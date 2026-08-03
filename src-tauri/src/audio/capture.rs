//! Microphone capture via cpal.
//!
//! The cpal callback runs on a realtime audio thread, so it does the absolute
//! minimum: downmix to mono f32 and push into a lock-free channel. Everything
//! else (resampling, VAD, encoding) happens on ordinary threads.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use crossbeam_channel::Sender;

pub struct CaptureHandle {
    // Held so the stream stays alive; dropped to stop capture.
    _stream: Stream,
    pub sample_rate: u32,
    pub device_name: String,
}

// cpal::Stream is !Send on some platforms, but we only ever create and drop
// the handle from the pipeline thread that owns it. The handle is moved into
// the pipeline thread right after creation and never shared.
unsafe impl Send for CaptureHandle {}

/// List available input device names (default device first).
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok());

    let mut names: Vec<String> = Vec::new();
    if let Some(ref n) = default_name {
        names.push(n.clone());
    }
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                if Some(&name) != default_name.as_ref() {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// Start capturing from the named device (or the default input device).
/// Mono f32 chunks at the device's native sample rate are sent to `tx`.
pub fn start(device_name: Option<&str>, tx: Sender<Vec<f32>>) -> Result<CaptureHandle, String> {
    let host = cpal::default_host();

    let device = match device_name {
        Some(wanted) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().map(|n| n == wanted).unwrap_or(false))
            .ok_or_else(|| format!("input device not found: {wanted}"))?,
        None => host
            .default_input_device()
            .ok_or("no default input device")?,
    };

    let resolved_name = device.name().unwrap_or_else(|_| "unknown".into());
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let err_fn = |e| eprintln!("[capture] stream error: {e}");

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| push_mono(data, channels, &tx),
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                let floats: Vec<f32> =
                    data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                push_mono(&floats, channels, &tx)
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| {
                let floats: Vec<f32> = data
                    .iter()
                    .map(|&s| (s as f32 - 32_768.0) / 32_768.0)
                    .collect();
                push_mono(&floats, channels, &tx)
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported sample format: {other:?}")),
    }
    .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;

    Ok(CaptureHandle {
        _stream: stream,
        sample_rate,
        device_name: resolved_name,
    })
}

/// Downmix interleaved frames to mono and send. Uses try_send so the audio
/// thread never blocks; if the consumer stalls we drop audio rather than
/// glitch the device.
fn push_mono(data: &[f32], channels: usize, tx: &Sender<Vec<f32>>) {
    let mono: Vec<f32> = if channels == 1 {
        data.to_vec()
    } else {
        data.chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };
    let _ = tx.try_send(mono);
}
