//! Turns recorded speech into text with a Whisper model, on this computer.

use std::path::Path;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadContext, WhisperVadContextParams,
    WhisperVadParams,
};

use super::listen;

/// Quieter than this is taken as silence, where Whisper tends to make words up.
const SILENCE: f32 = 0.003;
/// Shorter than this is too short to say anything.
const SHORTEST: usize = listen::RATE as usize / 4;
/// A part Whisper is less sure than this holds speech is left out.
const LIKELY_NOISE: f32 = 0.6;
/// What Whisper often "hears" in silence or noise. Dropped when it is all it heard.
const MADE_UP: &[&str] = &[
    "thank you",
    "thank you very much",
    "thank you so much",
    "thanks",
    "thanks for watching",
    "thank you for watching",
    "you",
    "bye",
];
const NOTHING_HEARD: &str = "Didn't hear anything. Check the microphone, then try again.";

/// A loaded model. Loading takes a few seconds, so it is kept between uses.
pub struct Engine {
    context: WhisperContext,
}

impl Engine {
    pub fn load(model: &Path) -> Result<Self, String> {
        whisper_rs::install_logging_hooks();
        let path = model.to_str().ok_or("The model's folder name can't be read.")?;
        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|err| format!("Could not load the voice model: {err}"))?;
        Ok(Self { context })
    }

    /// The text spoken in `samples`, which are at [`listen::RATE`]. `language`
    /// is a code like "en", or "auto". `vad` finds where speech is, so silence
    /// doesn't turn into made-up words.
    pub fn transcribe(&self, samples: &[f32], language: &str, vad: &Path) -> Result<String, String> {
        let vad = vad.to_str().filter(|_| vad.is_file());
        if samples.len() < SHORTEST || listen::loudness(samples) < SILENCE || vad.is_some_and(|vad| !has_speech(samples, vad)) {
            return Err(NOTHING_HEARD.to_owned());
        }
        let mut state = self.context.create_state().map_err(|err| format!("Could not start the voice model: {err}"))?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
        params.set_n_threads(threads as i32);
        params.set_language(Some(language));
        params.set_no_context(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if let Some(vad) = vad {
            params.set_vad_model_path(Some(vad));
            params.enable_vad(true);
        }
        state.full(params, samples).map_err(|err| format!("Could not turn the speech into text: {err}"))?;
        let parts: Vec<String> = state
            .as_iter()
            .filter(|segment| segment.no_speech_probability() < LIKELY_NOISE)
            .filter_map(|segment| segment.to_str_lossy().ok().map(|text| text.into_owned()))
            .collect();
        let text = tidy(&parts);
        if made_up(&text) {
            return Err(NOTHING_HEARD.to_owned());
        }
        if text.is_empty() {
            return Err("Didn't catch any words. Try again, a little closer to the microphone.".to_owned());
        }
        Ok(text)
    }
}

/// Whether the speech model `vad` finds any speech in `samples`. If it
/// can't run, the samples are given the benefit of the doubt.
fn has_speech(samples: &[f32], vad: &str) -> bool {
    let Ok(mut context) = WhisperVadContext::new(vad, WhisperVadContextParams::default()) else {
        return true;
    };
    context.segments_from_samples(WhisperVadParams::default(), samples).map_or(true, |segments| segments.num_segments() > 0)
}

/// Whether `text` is only one of the phrases Whisper makes up in silence.
fn made_up(text: &str) -> bool {
    let plain: String = text.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().to_lowercase();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    MADE_UP.contains(&plain.as_str())
}

/// Joins what Whisper heard into one line for the command box. Notes like
/// "[BLANK_AUDIO]" or "(music)" are dropped, and so is a last full stop,
/// which a command never ends with.
fn tidy(parts: &[String]) -> String {
    let words: Vec<&str> = parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| {
            !part.is_empty()
                && !(part.starts_with('[') && part.ends_with(']'))
                && !(part.starts_with('(') && part.ends_with(')'))
                && !(part.starts_with('*') && part.ends_with('*'))
        })
        .flat_map(str::split_whitespace)
        .collect();
    let line = words.join(" ");
    match line.strip_suffix(['.', '。']) {
        Some(rest) if !rest.ends_with('.') => rest.to_owned(),
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_the_last_full_stop_go() {
        let parts = |list: &[&str]| list.iter().map(|part| (*part).to_owned()).collect::<Vec<_>>();
        assert_eq!(tidy(&parts(&[" List all files", "  in this folder."])), "List all files in this folder");
        assert_eq!(tidy(&parts(&["[BLANK_AUDIO]"])), "");
        assert_eq!(tidy(&parts(&[" (music)", " git status."])), "git status");
        assert_eq!(tidy(&parts(&["Wait..."])), "Wait...");
        assert_eq!(tidy(&parts(&["Senaraikan fail。"])), "Senaraikan fail");
    }

    #[test]
    fn phrases_made_up_in_silence_are_dropped() {
        assert!(made_up("Thank you"));
        assert!(made_up(" Thank you for watching!"));
        assert!(made_up("you"));
        assert!(!made_up("thank you git status"));
        assert!(!made_up("git status"));
        assert!(!made_up(""));
    }
}
