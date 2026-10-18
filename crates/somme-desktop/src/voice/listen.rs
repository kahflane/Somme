//! Records the microphone until told to stop, then gives the sound as
//! Whisper wants it: one channel, 16,000 samples a second.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

/// Whisper's sample rate.
pub const RATE: u32 = 16_000;
/// Recording stops by itself after this long.
pub const MAX_LENGTH: Duration = Duration::from_secs(120);

/// A recording in progress. Dropping it stops the microphone.
pub struct Recording {
    stop: mpsc::Sender<()>,
    done: mpsc::Receiver<Result<Vec<f32>, String>>,
}

impl Recording {
    /// Stops recording. Slow: waits for the microphone to close.
    pub fn finish(self) -> Result<Vec<f32>, String> {
        let _ = self.stop.send(());
        self.done.recv().unwrap_or_else(|_| Err("The microphone stopped.".to_owned()))
    }
}

/// Starts recording from the default microphone.
pub fn start() -> Result<Recording, String> {
    #[cfg(feature = "snapshot")]
    if let Some(path) = std::env::var_os("SOMME_VOICE_WAV") {
        return from_wav(path.into());
    }
    let (stop, stopped) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let (started_tx, started) = mpsc::channel();
    // The stream can't move between threads on every system, so it lives on its own.
    std::thread::Builder::new()
        .name("microphone".into())
        .spawn(move || {
            let heard = Arc::new(Mutex::new(Vec::new()));
            let stream = match open(heard.clone()) {
                Ok((stream, rate)) => {
                    let _ = started_tx.send(Ok(()));
                    (stream, rate)
                }
                Err(err) => {
                    let _ = started_tx.send(Err(err));
                    return;
                }
            };
            let _ = stopped.recv_timeout(MAX_LENGTH);
            let (stream, rate) = stream;
            drop(stream);
            let samples = std::mem::take(&mut *heard.lock().unwrap_or_else(|e| e.into_inner()));
            let _ = done_tx.send(Ok(resample(&samples, rate)));
        })
        .map_err(|err| format!("Could not start recording: {err}"))?;
    started.recv().unwrap_or_else(|_| Err("Could not start recording.".to_owned()))?;
    Ok(Recording { stop, done })
}

/// Opens the default microphone, adding what it hears, as one channel, to `heard`.
fn open(heard: Arc<Mutex<Vec<f32>>>) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host().default_input_device().ok_or("No microphone found.")?;
    let config = device.default_input_config().map_err(|err| format!("Can't use the microphone: {err}"))?;
    let channels = usize::from(config.channels()).max(1);
    let rate = config.sample_rate();
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let stream = match format {
        SampleFormat::I8 => build::<i8>(&device, config, channels, heard),
        SampleFormat::I16 => build::<i16>(&device, config, channels, heard),
        SampleFormat::I32 => build::<i32>(&device, config, channels, heard),
        SampleFormat::U8 => build::<u8>(&device, config, channels, heard),
        SampleFormat::U16 => build::<u16>(&device, config, channels, heard),
        SampleFormat::F32 => build::<f32>(&device, config, channels, heard),
        SampleFormat::F64 => build::<f64>(&device, config, channels, heard),
        other => return Err(format!("The microphone gives sound as {other}, which Somme can't read.")),
    }
    .map_err(|err| format!("Can't use the microphone: {err}"))?;
    stream.play().map_err(|err| format!("Can't start the microphone: {err}"))?;
    Ok((stream, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    heard: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let limit = MAX_LENGTH.as_secs() as usize * config.sample_rate as usize;
    device.build_input_stream(
        config,
        move |data: &[T], _: &_| {
            let mut heard = heard.lock().unwrap_or_else(|e| e.into_inner());
            if heard.len() >= limit {
                return;
            }
            for frame in data.chunks(channels) {
                let sum: f32 = frame.iter().map(|&sample| f32::from_sample(sample)).sum();
                heard.push(sum / frame.len() as f32);
            }
        },
        |err| tracing::warn!(%err, "microphone"),
        None,
    )
}

/// Changes the sample rate to [`RATE`]. Sounds too high for the new rate are
/// filtered out first, so they don't fold back in as noise.
pub fn resample(input: &[f32], rate: u32) -> Vec<f32> {
    if rate == RATE || rate == 0 || input.is_empty() {
        return input.to_vec();
    }
    let step = f64::from(rate) / f64::from(RATE);
    // Cycles per input sample to keep: a bit under half the new rate.
    let cutoff = 0.5 * (1. / step).min(1.) * 0.9;
    let half = (8. * step.max(1.)).ceil() as i64;
    let out_len = (input.len() as f64 / step).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let at = i as f64 * step;
        let center = at.floor() as i64;
        let (mut sum, mut weights) = (0., 0.);
        for k in (center - half + 1)..=(center + half) {
            let Some(&sample) = usize::try_from(k).ok().and_then(|k| input.get(k)) else { continue };
            let x = at - k as f64;
            let window = 0.5 + 0.5 * (std::f64::consts::PI * x / half as f64).cos();
            let weight = sinc(2. * cutoff * x) * window;
            sum += f64::from(sample) * weight;
            weights += weight;
        }
        out.push(if weights.abs() > f64::EPSILON { (sum / weights) as f32 } else { 0. });
    }
    out
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.
    } else {
        let x = std::f64::consts::PI * x;
        x.sin() / x
    }
}

/// How loud the sound is, from 0 to 1.
pub fn loudness(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// For the UI tests: plays a WAV file instead of the microphone.
#[cfg(feature = "snapshot")]
fn from_wav(path: std::path::PathBuf) -> Result<Recording, String> {
    let (stop, stopped) = mpsc::channel::<()>();
    let (done_tx, done) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = stopped.recv();
        let _ = done_tx.send(read_wav(&path));
    });
    Ok(Recording { stop, done })
}

/// Reads a 16-bit PCM WAV file as one channel at [`RATE`].
#[cfg(feature = "snapshot")]
fn read_wav(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    let mut at = 12;
    let (mut channels, mut rate, mut samples) = (1usize, RATE, Vec::new());
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap_or_default()) as usize;
        let body = bytes.get(at + 8..at + 8 + size).ok_or("The WAV file is cut short.")?;
        match id {
            b"fmt " if body.len() >= 16 => {
                channels = usize::from(u16::from_le_bytes([body[2], body[3]])).max(1);
                rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            }
            b"data" => {
                let values: Vec<f32> =
                    body.chunks_exact(2).map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.).collect();
                samples = values.chunks(channels).map(|frame| frame.iter().sum::<f32>() / frame.len() as f32).collect();
            }
            _ => {}
        }
        at += 8 + size + size % 2;
    }
    Ok(resample(&samples, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, seconds: f64) -> Vec<f32> {
        let count = (f64::from(rate) * seconds) as usize;
        (0..count).map(|i| (2. * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin() as f32).collect()
    }

    #[test]
    fn speech_keeps_its_loudness_and_high_sounds_are_cut() {
        let voice = resample(&tone(440., 48_000, 1.), 48_000);
        assert_eq!(voice.len(), 16_000);
        // Away from the edges, a tone in the speech range comes through whole.
        let middle = &voice[1000..15_000];
        assert!((loudness(middle) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02, "{}", loudness(middle));
        // 12 kHz can't be shown at 16 kHz; it must not come back as a lower tone.
        let hiss = resample(&tone(12_000., 48_000, 1.), 48_000);
        assert!(loudness(&hiss[1000..15_000]) < 0.05, "{}", loudness(&hiss[1000..15_000]));
        let same = resample(&[0.5; 10], RATE);
        assert_eq!(same, [0.5; 10]);
        assert_eq!(resample(&tone(440., 44_100, 1.), 44_100).len(), 16_000);
    }
}
