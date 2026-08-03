//! Resampling and WAV encoding.
//!
//! Devices capture at 44.1/48 kHz; STT models want 16 kHz mono. Downsampling
//! before upload cuts the payload ~3x, which directly cuts request latency.

use std::io::Cursor;

use rubato::{FftFixedIn, Resampler};

use super::TARGET_SAMPLE_RATE;

/// Streaming resampler from a device rate to 16 kHz mono.
pub struct StreamResampler {
    inner: Option<FftFixedIn<f32>>,
    input_rate: u32,
    pending: Vec<f32>,
    chunk: usize,
}

impl StreamResampler {
    pub fn new(input_rate: u32) -> Result<Self, String> {
        if input_rate == TARGET_SAMPLE_RATE {
            return Ok(Self {
                inner: None,
                input_rate,
                pending: Vec::new(),
                chunk: 0,
            });
        }
        // ~64ms input chunks: small enough for low latency, large enough to
        // keep FFT resampling efficient.
        let chunk = (input_rate as usize / 16).max(256);
        let inner = FftFixedIn::<f32>::new(
            input_rate as usize,
            TARGET_SAMPLE_RATE as usize,
            chunk,
            2,
            1,
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            inner: Some(inner),
            input_rate,
            pending: Vec::new(),
            chunk,
        })
    }

    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// Feed device-rate samples, get back any 16 kHz samples ready so far.
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        let Some(resampler) = self.inner.as_mut() else {
            return samples.to_vec(); // already 16 kHz
        };

        self.pending.extend_from_slice(samples);
        let mut out: Vec<f32> = Vec::new();
        while self.pending.len() >= self.chunk {
            let frame: Vec<f32> = self.pending.drain(..self.chunk).collect();
            if let Ok(mut chunks) = resampler.process(&[frame], None) {
                if let Some(ch) = chunks.pop() {
                    out.extend(ch);
                }
            }
        }
        out
    }
}

/// Encode 16 kHz mono f32 samples as an in-memory 16-bit PCM WAV file.
pub fn wav_encode_16k(samples: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer =
            hound::WavWriter::new(&mut cursor, spec).expect("wav writer on Vec cannot fail");
        for &s in samples {
            let clamped = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(clamped)
                .expect("writing to Vec cannot fail");
        }
        writer.finalize().expect("finalizing Vec-backed wav");
    }
    cursor.into_inner()
}
