use crate::SAMPLE_RATE;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

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

    pub fn stem(self) -> &'static str {
        match self {
            SoundCue::Join => "join",
            SoundCue::Leave => "leave",
            SoundCue::PeerJoin => "peer_join",
            SoundCue::PeerLeave => "peer_leave",
            SoundCue::Mute => "mute",
            SoundCue::Unmute => "unmute",
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
    /// Scans `./assets/sounds/`, app bundle paths, and user config `squad-air/sounds/` for `.wav` / `.mp3` cues.
    pub fn load_optional() -> Self {
        let mut dirs = vec![PathBuf::from("assets/sounds")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                dirs.push(exe_dir.join("assets/sounds"));
                dirs.push(exe_dir.join("../Resources/assets/sounds"));
            }
        }
        if let Some(mut cfg_dir) = dirs::config_dir() {
            cfg_dir.push("squad-air");
            cfg_dir.push("sounds");
            dirs.push(cfg_dir);
        }
        Self::load_from_dirs(&dirs)
    }

    pub fn load_from_dirs(dirs: &[PathBuf]) -> Self {
        let mut cues = HashMap::new();

        for cue in SoundCue::all() {
            let wav_name = format!("{}.wav", cue.stem());
            let mp3_name = format!("{}.mp3", cue.stem());
            for dir in dirs {
                let wav_path = dir.join(&wav_name);
                if wav_path.exists() {
                    if let Some(samples) = Self::decode_wav_to_48k_mono(&wav_path)
                        .or_else(|| Self::decode_symphonia_to_48k_mono(&wav_path))
                    {
                        cues.insert(cue, Arc::new(samples));
                        break;
                    }
                }
                let mp3_path = dir.join(&mp3_name);
                if mp3_path.exists() {
                    if let Some(samples) = Self::decode_symphonia_to_48k_mono(&mp3_path) {
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

    fn resample_to_48k(mono: Vec<f32>, src_rate: u32) -> Vec<f32> {
        if src_rate == SAMPLE_RATE || src_rate == 0 || mono.is_empty() {
            return mono;
        }
        let ratio = src_rate as f64 / SAMPLE_RATE as f64;
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
        resampled
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

        let mut mono = Vec::with_capacity(raw_samples.len() / channels);
        for chunk in raw_samples.chunks(channels) {
            let sum: f32 = chunk.iter().copied().sum();
            mono.push(sum / (chunk.len() as f32));
        }

        Some(Self::resample_to_48k(mono, spec.sample_rate))
    }

    fn decode_symphonia_to_48k_mono(path: &Path) -> Option<Vec<f32>> {
        let file = std::fs::File::open(path).ok()?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .ok()?;
        let mut format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)?;
        let track_id = track.id;
        let mut src_rate = track.codec_params.sample_rate.unwrap_or(SAMPLE_RATE);
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .ok()?;

        let mut mono = Vec::new();
        loop {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::ResetRequired) => {
                    decoder.reset();
                    continue;
                }
                Err(_) => break,
            };
            if packet.track_id() != track_id {
                continue;
            }
            match decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    src_rate = spec.rate;
                    let channels = spec.channels.count().max(1);
                    let mut sample_buf =
                        SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    sample_buf.copy_interleaved_ref(decoded);
                    for chunk in sample_buf.samples().chunks(channels) {
                        let sum: f32 = chunk.iter().copied().sum();
                        mono.push(sum / (chunk.len() as f32));
                    }
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => break,
            }
        }

        if mono.is_empty() {
            None
        } else {
            Some(Self::resample_to_48k(mono, src_rate))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_wav_cues_load_for_all_events() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/sounds");
        let bank = SoundCueBank::load_from_dirs(&[dir]);
        for cue in SoundCue::all() {
            let samples = bank
                .get(cue)
                .unwrap_or_else(|| panic!("missing cue {:?}", cue));
            assert!(!samples.is_empty(), "empty cue {:?}", cue);
        }
    }

    #[test]
    fn symphonia_decodes_and_resamples_stereo_wav_and_skips_corrupt_mp3() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "squad_dsp_cues_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let wav_path = tmp_dir.join("join.wav");
        let bad_mp3_path = tmp_dir.join("leave.mp3");

        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
        for i in 0..240 {
            let val = ((i as f32 * 0.1).sin() * 12000.0) as i16;
            writer.write_sample(val).unwrap();
            writer.write_sample(-val).unwrap();
        }
        writer.finalize().unwrap();
        std::fs::write(&bad_mp3_path, b"not-a-valid-mp3").unwrap();

        let decoded = SoundCueBank::decode_symphonia_to_48k_mono(&wav_path)
            .expect("symphonia should decode 24kHz stereo WAV");
        assert!((decoded.len() as i32 - 480).abs() <= 2);

        let bank = SoundCueBank::load_from_dirs(std::slice::from_ref(&tmp_dir));
        assert!(bank.get(SoundCue::Join).is_some());
        assert!(bank.get(SoundCue::Leave).is_none());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
