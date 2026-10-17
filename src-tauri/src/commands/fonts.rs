use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use fontdb::{Database, FaceInfo, Source, Style};
use serde::Serialize;
use tauri::ipc::Response;

use crate::error::{ApiError, ApiResult};

const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
// Past this gap the web view does better faking the weight from the regular face.
const MAX_WEIGHT_GAP: u16 = 150;

static FONTS: Mutex<Option<Arc<Database>>> = Mutex::new(None);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemFont {
    family: String,
    monospace: bool,
}

fn database(refresh: bool) -> Arc<Database> {
    let mut slot = FONTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match slot.as_ref() {
        Some(db) if !refresh => Arc::clone(db),
        _ => {
            let mut db = Database::new();
            db.load_system_fonts();
            let db = Arc::new(db);
            *slot = Some(Arc::clone(&db));
            db
        }
    }
}

async fn load(refresh: bool) -> ApiResult<Arc<Database>> {
    tokio::task::spawn_blocking(move || database(refresh))
        .await
        .map_err(|err| ApiError::io(err.to_string()))
}

fn family_of(face: &FaceInfo) -> Option<&str> {
    face.families
        .first()
        .map(|(name, _)| name.as_str())
        .filter(|name| !name.starts_with('.'))
}

// Only plain font files can be handed to the web view; collections hold many
// faces and the web view would always pick the first one.
fn single_face_path(face: &FaceInfo) -> Option<&Path> {
    let path = match &face.source {
        Source::File(path) | Source::SharedFile(path, _) => path.as_path(),
        Source::Binary(_) => return None,
    };
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    (face.index == 0 && matches!(extension.as_str(), "ttf" | "otf")).then_some(path)
}

#[tauri::command]
pub async fn system_fonts(refresh: Option<bool>) -> ApiResult<Vec<SystemFont>> {
    let db = load(refresh.unwrap_or(false)).await?;
    let mut families: BTreeMap<String, bool> = BTreeMap::new();
    for face in db.faces() {
        if let Some(family) = family_of(face) {
            *families.entry(family.to_owned()).or_insert(false) |= face.monospaced;
        }
    }
    let mut fonts: Vec<SystemFont> = families
        .into_iter()
        .map(|(family, monospace)| SystemFont { family, monospace })
        .collect();
    fonts.sort_by_key(|font| font.family.to_lowercase());
    Ok(fonts)
}

#[tauri::command]
pub async fn system_font_data(family: String, weight: u16, italic: bool) -> ApiResult<Response> {
    let db = load(false).await?;
    let path = db
        .faces()
        .filter(|face| family_of(face) == Some(family.as_str()))
        .filter(|face| (face.style != Style::Normal) == italic)
        .filter_map(|face| single_face_path(face).map(|path| (face.weight.0, path.to_path_buf())))
        .filter(|(face_weight, _)| face_weight.abs_diff(weight) <= MAX_WEIGHT_GAP)
        .min_by_key(|(face_weight, _)| face_weight.abs_diff(weight))
        .map(|(_, path)| path)
        .ok_or_else(|| ApiError::io(format!("no loadable font file for {family}")))?;
    let size = tokio::fs::metadata(&path).await?.len();
    if size > MAX_FONT_BYTES {
        return Err(ApiError::io(format!("font file for {family} is too large")));
    }
    Ok(Response::new(tokio::fs::read(&path).await?))
}
