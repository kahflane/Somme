//! Downloads files pinned to a known size and hash, like voice models and
//! language servers. Each is written next to its place first, and moved in
//! only once both match.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest as _, Sha256};

/// A file at a fixed version, so its hash never changes.
pub struct Pinned {
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// A size like "54 MB".
pub fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1_000_000))
}

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("somme/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())
}

/// Downloads `file` to `path`. `what` names it in errors, like "the voice
/// model". Adds each chunk's size to `progress`, and stops when `cancel` is set.
pub async fn fetch(
    client: &reqwest::Client,
    file: &Pinned,
    path: &Path,
    what: &str,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;

    let partial = path.with_extension("part");
    let failed = |err: String| {
        let partial = partial.clone();
        async move {
            let _ = tokio::fs::remove_file(&partial).await;
            Err(err)
        }
    };
    let mut response = match client.get(file.url).send().await.and_then(reqwest::Response::error_for_status) {
        Ok(response) => response,
        Err(err) => return failed(format!("Could not download {what}: {err}")).await,
    };
    let mut out = match tokio::fs::File::create(&partial).await {
        Ok(out) => out,
        Err(err) => return failed(format!("Could not save {what}: {err}")).await,
    };
    let mut hash = Sha256::new();
    let mut got = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return failed("Download stopped.".to_owned()).await;
        }
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(err) => return failed(format!("The download broke off: {err}")).await,
        };
        got += chunk.len() as u64;
        if got > file.size {
            return failed(format!("{} is bigger than it should be.", capitalized(what))).await;
        }
        hash.update(&chunk);
        if let Err(err) = out.write_all(&chunk).await {
            return failed(format!("Could not save {what}: {err}")).await;
        }
        progress.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    if let Err(err) = out.flush().await {
        return failed(format!("Could not save {what}: {err}")).await;
    }
    drop(out);
    let digest: String = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if got != file.size || digest != file.sha256 {
        return failed(format!("{} didn't match its checksum, so it was thrown away.", capitalized(what))).await;
    }
    tokio::fs::rename(&partial, path).await.map_err(|err| format!("Could not save {what}: {err}"))
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_start_with_a_capital() {
        assert_eq!(capitalized("the voice model"), "The voice model");
        assert_eq!(capitalized(""), "");
    }
}
