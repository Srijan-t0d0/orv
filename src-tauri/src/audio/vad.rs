//! Utterance segmentation with Silero VAD.
//!
//! This is the real latency knob of the app: the sooner we decide the user
//! stopped talking, the sooner the request goes out. We run Silero on 32 ms
//! frames (512 samples @ 16 kHz) and close an utterance after a short
//! silence hangover.

use voice_activity_detector::VoiceActivityDetector;

use super::TARGET_SAMPLE_RATE;

/// Silero requires exactly 512-sample chunks at 16 kHz.
pub const FRAME_SAMPLES: usize = 512;

const SPEECH_THRESHOLD: f32 = 0.5;
/// Frames of leading audio kept before speech onset (~192 ms) so we don't
/// clip the first phoneme.
const PREROLL_FRAMES: usize = 6;
/// Silence frames required to close an utterance (~350 ms).
const HANGOVER_FRAMES: usize = 11;
/// Hard cap per segment so long monologues still produce partial results
/// (~15 s of audio).
const MAX_UTTERANCE_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 15;
/// Ignore blips shorter than this (~200 ms) — coughs, key clicks.
const MIN_UTTERANCE_SAMPLES: usize = TARGET_SAMPLE_RATE as usize / 5;

pub enum VadEvent {
    /// User started talking.
    SpeechStart,
    /// A complete utterance, 16 kHz mono, ready to transcribe.
    Utterance(Vec<f32>),
}

pub struct Segmenter {
    vad: VoiceActivityDetector,
    frame_buf: Vec<f32>,
    preroll: Vec<Vec<f32>>,
    utterance: Vec<f32>,
    in_speech: bool,
    silence_frames: usize,
}

impl Segmenter {
    pub fn new() -> Result<Self, String> {
        let vad = VoiceActivityDetector::builder()
            .sample_rate(TARGET_SAMPLE_RATE as i64)
            .chunk_size(FRAME_SAMPLES)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            vad,
            frame_buf: Vec::with_capacity(FRAME_SAMPLES * 2),
            preroll: Vec::new(),
            utterance: Vec::new(),
            in_speech: false,
            silence_frames: 0,
        })
    }

    /// Feed 16 kHz mono samples; returns zero or more events.
    pub fn push(&mut self, samples: &[f32]) -> Vec<VadEvent> {
        let mut events = Vec::new();
        self.frame_buf.extend_from_slice(samples);

        while self.frame_buf.len() >= FRAME_SAMPLES {
            let frame: Vec<f32> = self.frame_buf.drain(..FRAME_SAMPLES).collect();
            let prob = self.vad.predict(frame.iter().copied());
            let is_speech = prob > SPEECH_THRESHOLD;

            if self.in_speech {
                self.utterance.extend_from_slice(&frame);

                if is_speech {
                    self.silence_frames = 0;
                } else {
                    self.silence_frames += 1;
                }

                let closed_by_silence = self.silence_frames >= HANGOVER_FRAMES;
                let closed_by_length = self.utterance.len() >= MAX_UTTERANCE_SAMPLES;
                if closed_by_silence || closed_by_length {
                    let audio = std::mem::take(&mut self.utterance);
                    self.in_speech = closed_by_length; // keep going after a forced split
                    self.silence_frames = 0;
                    if audio.len() >= MIN_UTTERANCE_SAMPLES {
                        events.push(VadEvent::Utterance(audio));
                    }
                }
            } else if is_speech {
                self.in_speech = true;
                self.silence_frames = 0;
                // Include pre-roll so the utterance doesn't start mid-word.
                for pre in self.preroll.drain(..) {
                    self.utterance.extend_from_slice(&pre);
                }
                self.utterance.extend_from_slice(&frame);
                events.push(VadEvent::SpeechStart);
            } else {
                self.preroll.push(frame);
                if self.preroll.len() > PREROLL_FRAMES {
                    self.preroll.remove(0);
                }
            }
        }

        events
    }

    /// Flush any in-progress utterance (e.g. when the user hits stop).
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        self.in_speech = false;
        self.silence_frames = 0;
        let audio = std::mem::take(&mut self.utterance);
        (audio.len() >= MIN_UTTERANCE_SAMPLES).then_some(audio)
    }
}
