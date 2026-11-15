use crate::SAMPLE_RATE;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SoundCue {
    Join,
    Leave,
    PeerJoin,
    PeerLeave,
    Mute,
    Unmute,
}

impl SoundCue {
    pub fn filename(self) -> &'static str {
        match self {
            SoundCue::Join => "join.wav",
            SoundCue::Leave => "leave.wav",
            SoundCue::PeerJoin => "peer_join.wav",
            SoundCue::PeerLeave => "peer_leave.wav",
            SoundCue::Mute => "mute.wav",
            SoundCue::Unmute => "unmute.wav",
        }
    }

    pub fn all() -> [SoundCue; 6] {
        [
            SoundCue::Join,
            SoundCue::Leave,
            SoundCue::PeerJoin,
            SoundCue::PeerLeave,
            SoundCue::Mute,
            SoundCue::Unmute,
        ]
    }
}

#[derive(Clone, Default)]
pub struct SoundCueBank {
    cues: HashMap<SoundCue, Arc<Vec<f32>>>,
}

impl SoundCueBank {
    /// Сканирует `./assets/sounds/` и пользовательскую папку конфига `squad-air/sounds/`.
    /// Отсутствующие или поврежденные файлы молча игнорируются.
    pub fn load_optional() -> Self {
        let mut cues = HashMap::new();
        let mut dirs = vec![PathBuf::from("assets/sounds")];
        if let Some(mut cfg_dir) = dirs::config_dir() {
            cfg_dir.push("squad-air");
            cfg_dir.push("sounds");
            dirs.push(cfg_dir);
        }

        for cue in SoundCue::all() {
            for dir in &dirs {
                let file_path = dir.join(cue.filename());
                if file_path.exists() {
                    if let Some(samples) = Self::decode_wav_to_48k_mono(&file_path) {
                        cues.insert(cue, Arc::new(samples));
                        break;
                    }
                }
            }
        }

        Self { cues }
    }

    pub fn get(&self, cue: SoundCue) -> Option<Arc<Vec<f32>>> {
        self.cues.get(&cue).cloned()
    }

    fn decode_wav_to_48k_mono(path: &Path) -> Option<Vec<f32>> {
        let mut reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let channels = (spec.channels as usize).max(1);

        let raw_samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
            hound::SampleFormat::Int => {
                let max_val = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
                reader
                    .samples::<i32>()
                    .filter_map(Result::ok)
                    .map(|s| (s as f32 / max_val).clamp(-1.0, 1.0))
                    .collect()
            }
        };

        if raw_samples.is_empty() {
            return None;
        }

        // Downmix to mono
        let mut mono = Vec::with_capacity(raw_samples.len() / channels);
        for chunk in raw_samples.chunks(channels) {
            let sum: f32 = chunk.iter().copied().sum();
            mono.push(sum / (chunk.len() as f32));
        }

        // Линейная интерполяция к 48 кГц, если исходный WAV имеет другую частоту
        if spec.sample_rate == SAMPLE_RATE {
            return Some(mono);
        }

        let ratio = spec.sample_rate as f64 / SAMPLE_RATE as f64;
        let out_len = ((mono.len() as f64) / ratio) as usize;
        let mut resampled = Vec::with_capacity(out_len);
        for i in 0..out_len {
            let src_pos = i as f64 * ratio;
            let idx = src_pos as usize;
            let frac = (src_pos - idx as f64) as f32;
            let s0 = mono.get(idx).copied().unwrap_or(0.0);
            let s1 = mono.get(idx + 1).copied().unwrap_or(s0);
            resampled.push(s0 + (s1 - s0) * frac);
        }
        Some(resampled)
    }
}