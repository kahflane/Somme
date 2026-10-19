//! Voice typing: talk, and the words go into the command box. Whisper turns
//! the speech into text on this computer; nothing is sent anywhere. The model
//! is downloaded once, when the user asks, and checked against its known hash.

mod listen;
mod whisper;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext as _, Context, Entity, EntityId, Global, Subscription, Task};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::download::{self, Pinned};
use crate::runtime;
use crate::settings::Settings;

/// A loaded model is let go after this long unused, to free its memory.
const KEEP_LOADED: Duration = Duration::from_secs(10 * 60);
const DOWNLOAD_REFRESH: Duration = Duration::from_millis(250);

/// Which Whisper model to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum VoiceModel {
    /// large-v3-turbo, stored at 5 bits: the most accurate, quick on a GPU.
    #[default]
    Best,
    /// small, for older computers.
    Small,
}

/// A file to download, pinned to one version so its hash never changes.
struct ModelFile {
    name: &'static str,
    pinned: Pinned,
}

const BEST: ModelFile = ModelFile {
    name: "ggml-large-v3-turbo-q5_0.bin",
    pinned: Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q5_0.bin",
        size: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
};

const SMALL: ModelFile = ModelFile {
    name: "ggml-small-q5_1.bin",
    pinned: Pinned {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small-q5_1.bin",
        size: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    },
};

/// Silero, which finds where in a recording someone speaks.
const VAD: ModelFile = ModelFile {
    name: "ggml-silero-v5.1.2.bin",
    pinned: Pinned {
        url: "https://huggingface.co/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v5.1.2.bin",
        size: 885_098,
        sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
    },
};

impl VoiceModel {
    fn file(self) -> &'static ModelFile {
        match self {
            Self::Best => &BEST,
            Self::Small => &SMALL,
        }
    }

    /// The download size, like "574 MB".
    pub fn size(self) -> String {
        megabytes(self.file().pinned.size + VAD.pinned.size)
    }
}

pub use crate::download::megabytes;

/// The languages offered, by Whisper's code. Whisper knows about 100; these
/// are the common ones. "auto" lets it guess from the speech.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("auto", "Detect from speech"),
    ("en", "English"),
    ("ms", "Malay"),
    ("zh", "Chinese"),
    ("ta", "Tamil"),
    ("id", "Indonesian"),
    ("hi", "Hindi"),
    ("bn", "Bengali"),
    ("ar", "Arabic"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("pt", "Portuguese"),
    ("ru", "Russian"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Recording for the terminal `owner`.
    Listening { owner: EntityId },
    /// Turning the recording into text for `owner`.
    Writing { owner: EntityId },
}

pub struct Download {
    pub done: u64,
    pub total: u64,
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    _task: Task<()>,
}

pub struct Voice {
    dir: PathBuf,
    phase: Phase,
    recording: Option<listen::Recording>,
    download: Option<Download>,
    /// What went wrong with the last download.
    pub download_error: Option<String>,
    engine: Arc<Mutex<Option<(VoiceModel, whisper::Engine)>>>,
    used: Instant,
    /// How loud the microphone looks, falling back slowly so the wave moves smoothly.
    level: f32,
    /// Redraws the wave while listening.
    meter: Option<Task<()>>,
    _unload: Task<()>,
    _quit: Subscription,
}

struct GlobalVoice(Entity<Voice>);

impl Global for GlobalVoice {}

impl Voice {
    /// Models are kept in `voice` under the app's data folder.
    pub fn init(data_dir: &Path, cx: &mut App) {
        let dir = data_dir.join("voice");
        let voice = cx.new(|cx: &mut Context<Self>| {
            let unload = cx.spawn(async move |voice, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(60)).await;
                    let Ok(engine) = voice.update(cx, |voice, _| {
                        (voice.phase == Phase::Idle && voice.used.elapsed() > KEEP_LOADED).then(|| voice.engine.clone())
                    }) else {
                        return;
                    };
                    if let Some(engine) = engine {
                        cx.background_spawn(async move { drop(engine.lock().take()) }).await;
                    }
                }
            });
            // whisper.cpp stops the app with an error if the model is still on
            // the graphics card when the app ends.
            let quit = cx.on_app_quit(|voice: &mut Self, _| {
                voice.recording = None;
                if let Some(mut engine) = voice.engine.try_lock() {
                    *engine = None;
                }
                async {}
            });
            Self {
                dir,
                phase: Phase::Idle,
                recording: None,
                download: None,
                download_error: None,
                engine: Arc::default(),
                used: Instant::now(),
                level: 0.,
                meter: None,
                _unload: unload,
                _quit: quit,
            }
        });
        cx.set_global(GlobalVoice(voice));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalVoice>().0.clone()
    }

    pub fn chosen(cx: &App) -> VoiceModel {
        Settings::get(cx).voice_model
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// How loud the microphone is while listening, from 0 to 1.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// How long it has been listening, which moves the wave along.
    pub fn listening_for(&self) -> Duration {
        self.used.elapsed()
    }

    pub fn download(&self) -> Option<&Download> {
        self.download.as_ref()
    }

    fn path(&self, file: &ModelFile) -> PathBuf {
        self.dir.join(file.name)
    }

    /// The model and the speech finder are downloaded whole.
    pub fn ready(&self, model: VoiceModel) -> bool {
        [model.file(), &VAD]
            .iter()
            .all(|file| std::fs::metadata(self.path(file)).is_ok_and(|meta| meta.len() == file.pinned.size))
    }

    /// Downloads `model` and the speech finder, checking each against its hash.
    pub fn start_download(&mut self, model: VoiceModel, cx: &mut Context<Self>) {
        if self.download.is_some() {
            return;
        }
        self.download_error = None;
        let files: Vec<(&'static ModelFile, PathBuf)> = [&VAD, model.file()]
            .into_iter()
            .filter(|file| !std::fs::metadata(self.path(file)).is_ok_and(|meta| meta.len() == file.pinned.size))
            .map(|file| (file, self.path(file)))
            .collect();
        let total = files.iter().map(|(file, _)| file.pinned.size).sum();
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (seen, stop) = (progress.clone(), cancel.clone());
        let dir = self.dir.clone();
        let fetch = runtime::io(async move { fetch_all(&dir, files, &seen, &stop).await });
        let task = cx.spawn(async move |voice, cx| {
            let mut fetch = std::pin::pin!(fetch);
            let result = loop {
                let tick = cx.background_executor().timer(DOWNLOAD_REFRESH);
                match futures::future::select(fetch.as_mut(), tick).await {
                    futures::future::Either::Left((result, _)) => break result,
                    futures::future::Either::Right(_) => {
                        let _ = voice.update(cx, |voice, cx| {
                            if let Some(download) = voice.download.as_mut() {
                                download.done = download.progress.load(Ordering::Relaxed);
                            }
                            cx.notify();
                        });
                    }
                }
            };
            let _ = voice.update(cx, |voice, cx| {
                voice.download = None;
                voice.download_error = result.err();
                cx.notify();
            });
        });
        self.download = Some(Download {
            done: 0,
            total,
            progress,
            cancel,
            _task: task,
        });
        cx.notify();
    }

    pub fn cancel_download(&mut self, cx: &mut Context<Self>) {
        if let Some(download) = &self.download {
            download.cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    /// Deletes `model` from disk. The speech finder stays: it is tiny.
    pub fn delete(&mut self, model: VoiceModel, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine.try_lock().as_mut()
            && engine.as_ref().is_some_and(|(loaded, _)| *loaded == model)
        {
            **engine = None;
        }
        let _ = std::fs::remove_file(self.path(model.file()));
        cx.notify();
    }

    /// Starts recording for the terminal `owner`, and loads the model meanwhile.
    pub fn listen(&mut self, owner: EntityId, cx: &mut Context<Self>) -> Result<(), String> {
        if self.phase != Phase::Idle {
            return Err("Voice typing is busy in another tab.".to_owned());
        }
        let model = Self::chosen(cx);
        if !self.ready(model) {
            return Err("The voice model isn't downloaded yet.".to_owned());
        }
        self.recording = Some(listen::start()?);
        self.phase = Phase::Listening { owner };
        self.used = Instant::now();
        self.level = 0.;
        self.meter = Some(cx.spawn(async move |voice, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(50)).await;
                let listening = voice.update(cx, |voice, cx| {
                    let heard = voice.recording.as_ref().map_or(0., listen::Recording::level);
                    voice.level = heard.max(voice.level * 0.8);
                    cx.notify();
                    matches!(voice.phase, Phase::Listening { .. })
                });
                if !listening.unwrap_or(false) {
                    return;
                }
            }
        }));
        let (engine, path) = (self.engine.clone(), self.path(model.file()));
        cx.background_spawn(async move {
            if let Err(err) = load(&engine, model, &path) {
                tracing::warn!("{err}");
            }
        })
        .detach();
        cx.notify();
        Ok(())
    }

    /// Stops recording and gives the words spoken.
    pub fn finish(&mut self, cx: &mut Context<Self>) -> Task<Result<String, String>> {
        let (Some(recording), Phase::Listening { owner }) = (self.recording.take(), self.phase) else {
            return Task::ready(Err("Voice typing wasn't listening.".to_owned()));
        };
        self.phase = Phase::Writing { owner };
        self.meter = None;
        cx.notify();
        let model = Self::chosen(cx);
        let language = Settings::get(cx).voice_language.clone();
        let (engine, path, vad) = (self.engine.clone(), self.path(model.file()), self.path(&VAD));
        let work = cx.background_spawn(async move {
            let samples = recording.finish()?;
            load(&engine, model, &path)?;
            let engine = engine.lock();
            let (_, loaded) = engine.as_ref().ok_or("The voice model isn't loaded.")?;
            loaded.transcribe(&samples, &language, &vad)
        });
        cx.spawn(async move |voice, cx| {
            let result = work.await;
            let _ = voice.update(cx, |voice, cx| {
                voice.phase = Phase::Idle;
                voice.used = Instant::now();
                cx.notify();
            });
            result
        })
    }

    /// Stops recording for `owner`, a terminal that has closed.
    pub fn release(&mut self, owner: EntityId, cx: &mut Context<Self>) {
        if self.phase == (Phase::Listening { owner }) {
            self.cancel(cx);
        }
    }

    /// Stops recording without writing anything.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(recording) = self.recording.take() {
            cx.background_spawn(async move { drop(recording.finish()) }).detach();
        }
        if matches!(self.phase, Phase::Listening { .. }) {
            self.phase = Phase::Idle;
        }
        self.meter = None;
        cx.notify();
    }
}

/// Loads `model` unless it is loaded already. Slow the first time.
fn load(engine: &Mutex<Option<(VoiceModel, whisper::Engine)>>, model: VoiceModel, path: &Path) -> Result<(), String> {
    let mut engine = engine.lock();
    if engine.as_ref().is_some_and(|(loaded, _)| *loaded == model) {
        return Ok(());
    }
    *engine = None;
    *engine = Some((model, whisper::Engine::load(path)?));
    Ok(())
}

async fn fetch_all(
    dir: &Path,
    files: Vec<(&'static ModelFile, PathBuf)>,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    tokio::fs::create_dir_all(dir).await.map_err(|err| format!("Could not make the voice folder: {err}"))?;
    let client = download::client()?;
    for (file, path) in files {
        download::fetch(&client, &file.pinned, &path, "the voice model", progress, cancel).await?;
    }
    Ok(())
}
