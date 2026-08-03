pub mod capture;
pub mod encode;
pub mod vad;

/// Target sample rate for the whole pipeline. 16 kHz mono is what STT models
/// are trained on; anything higher only inflates upload time.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
