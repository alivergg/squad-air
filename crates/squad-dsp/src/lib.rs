pub mod aec;
#[cfg(target_os = "android")]
mod android_audio;
pub mod codec;
pub mod cues;
pub mod denoise;
pub mod engine;
mod hotplug;
pub mod jitter;
#[cfg(target_os = "macos")]
mod macos_loopback;
pub mod mixer;
pub mod resampler;
#[cfg(target_os = "windows")]
mod win_loopback;

pub use aec::EchoCanceller;
pub use codec::{OpusCodecError, OpusDecoder10ms, OpusEncoder10ms};
pub use cues::{SoundCue, SoundCueBank};
pub use denoise::{NoiseSuppressionMode, NoiseSuppressor};
pub use engine::{
    list_input_devices, list_output_devices, AudioEngineConfig, AudioEngineHandle,
    MicActivationMode, OutboundAudioFrame, PeerAudioStats,
};
pub use jitter::{JitterBuffer, JitterProfile, JitterSnapshot};
pub use mixer::AudioMixer;
pub use resampler::{CaptureResampler, PlaybackResampler};

/// Строгий стандарт частоты дискретизации всего DSP-тракта (48 кГц).
pub const SAMPLE_RATE: u32 = 48_000;

/// Строгий размер кадра 10 мс при 48 кГц моно (480 сэмплов).
pub const FRAME_SAMPLES: usize = 480;
