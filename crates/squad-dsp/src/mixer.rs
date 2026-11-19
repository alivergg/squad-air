use crate::cues::{SoundCue, SoundCueBank};
use crate::jitter::{JitterBuffer, JitterProfile, JitterSnapshot};
use crate::FRAME_SAMPLES;
use squad_proto::PeerId;
use std::collections::HashMap;
use std::sync::Arc;

pub const VOICE_UNITY_SCALE: f32 = 1.9;
const LOOPBACK_GAIN: f32 = 1.25;
const CUE_UNITY_SCALE: f32 = 0.75;

pub struct SoftLimiter {
    envelope: f32,
}

impl SoftLimiter {
    const THRESHOLD: f32 = 0.70;
    const RELEASE_COEFF: f32 = 0.9992;
    const CEILING_KNEE: f32 = 0.78;
    const CEILING_HEADROOM: f32 = 0.18;

    pub fn new() -> Self {
        Self { envelope: 0.0 }
    }

    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        if !x.is_finite() {
            return 0.0;
        }
        if !self.envelope.is_finite() {
            self.envelope = 0.0;
        }
        let abs_x = x.abs().min(10.0);
        if abs_x > self.envelope {
            self.envelope = abs_x;
        } else {
            self.envelope *= Self::RELEASE_COEFF;
        }

        let leveled = if self.envelope > Self::THRESHOLD {
            let target = Self::THRESHOLD + 0.15 * ((self.envelope - Self::THRESHOLD) / 0.15).tanh();
            x * (target / self.envelope)
        } else {
            x
        };

        let abs_y = leveled.abs();
        if abs_y <= Self::CEILING_KNEE {
            leveled
        } else {
            leveled.signum()
                * (Self::CEILING_KNEE
                    + Self::CEILING_HEADROOM
                        * ((abs_y - Self::CEILING_KNEE) / Self::CEILING_HEADROOM).tanh())
        }
    }

    pub fn process_frame(&mut self, frame: &mut [f32; FRAME_SAMPLES]) {
        for sample in frame.iter_mut() {
            *sample = self.process_sample(*sample);
        }
    }
}

impl Default for SoftLimiter {
    fn default() -> Self {
        Self::new()
    }
}

struct PeerChannel {
    jitter: JitterBuffer,
    gain: f32,
}

struct ActiveCueCursor {
    samples: Arc<Vec<f32>>,
    pos: usize,
}

pub struct AudioMixer {
    peers: HashMap<PeerId, PeerChannel>,
    stream_peers: HashMap<PeerId, PeerChannel>,
    stream_gain: f32,
    jitter_profile: JitterProfile,
    cue_bank: SoundCueBank,
    cue_gain: f32,
    active_cues: Vec<ActiveCueCursor>,
    peer_scratch: [f32; FRAME_SAMPLES],
    loopback_frames: [[f32; FRAME_SAMPLES]; 2],
    loopback_read: usize,
    loopback_write: usize,
    loopback_len: usize,
    limiter: SoftLimiter,
}

impl AudioMixer {
    pub fn new(cue_bank: SoundCueBank, jitter_profile: JitterProfile) -> Self {
        Self {
            peers: HashMap::with_capacity(10),
            stream_peers: HashMap::with_capacity(4),
            stream_gain: 1.0,
            jitter_profile,
            cue_bank,
            cue_gain: 1.0,
            active_cues: Vec::with_capacity(4),
            peer_scratch: [0.0; FRAME_SAMPLES],
            loopback_frames: [[0.0; FRAME_SAMPLES]; 2],
            loopback_read: 0,
            loopback_write: 0,
            loopback_len: 0,
            limiter: SoftLimiter::new(),
        }
    }

    pub fn set_jitter_profile(&mut self, profile: JitterProfile) {
        self.jitter_profile = profile;
        for channel in self.peers.values_mut() {
            channel.jitter.set_profile(profile);
        }
        for channel in self.stream_peers.values_mut() {
            channel.jitter.set_profile(profile);
        }
    }

    pub fn push_loopback_frame(&mut self, pcm: &[f32; FRAME_SAMPLES]) {
        self.loopback_frames[self.loopback_write].copy_from_slice(pcm);
        self.loopback_write = (self.loopback_write + 1) % 2;
        if self.loopback_len == 2 {
            self.loopback_read = (self.loopback_read + 1) % 2;
        } else {
            self.loopback_len += 1;
        }
    }

    pub fn clear_loopback(&mut self) {
        self.loopback_read = 0;
        self.loopback_write = 0;
        self.loopback_len = 0;
    }

    pub fn has_loopback(&self) -> bool {
        self.loopback_len > 0
    }

    pub fn push_peer_opus(&mut self, peer_id: PeerId, seq_num: u64, opus_payload: Vec<u8>) {
        if !self.peers.contains_key(&peer_id) && self.peers.len() >= 16 {
            return;
        }
        let profile = self.jitter_profile;
        let channel = self.peers.entry(peer_id).or_insert_with(|| PeerChannel {
            jitter: JitterBuffer::with_profile(profile),
            gain: 1.0,
        });
        channel.jitter.push_packet(seq_num, opus_payload);
    }

    pub fn push_stream_opus(&mut self, peer_id: PeerId, seq_num: u64, opus_payload: Vec<u8>) {
        if !self.stream_peers.contains_key(&peer_id) && self.stream_peers.len() >= 8 {
            return;
        }
        let profile = self.jitter_profile;
        let channel = self
            .stream_peers
            .entry(peer_id)
            .or_insert_with(|| PeerChannel {
                jitter: JitterBuffer::with_profile(profile),
                gain: 1.0,
            });
        channel.jitter.push_packet(seq_num, opus_payload);
    }

    pub fn set_stream_gain(&mut self, gain: f32) {
        let clamped = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            1.0
        };
        self.stream_gain = clamped;
    }

    pub fn clear_stream_audio(&mut self) {
        self.stream_peers.clear();
    }

    /// Устанавливает индивидуальную громкость участника (`0.0` = 0%, `1.0` = 100%, `2.0` = 200%).
    pub fn set_peer_gain(&mut self, peer_id: PeerId, gain: f32) {
        if !self.peers.contains_key(&peer_id) && self.peers.len() >= 16 {
            return;
        }
        let clamped = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            1.0
        };
        let profile = self.jitter_profile;
        let channel = self.peers.entry(peer_id).or_insert_with(|| PeerChannel {
            jitter: JitterBuffer::with_profile(profile),
            gain: clamped,
        });
        channel.gain = clamped;
    }

    pub fn remove_peer(&mut self, peer_id: PeerId) {
        self.peers.remove(&peer_id);
        self.stream_peers.remove(&peer_id);
    }

    pub fn clear_peers(&mut self) {
        self.peers.clear();
        self.stream_peers.clear();
    }

    pub fn set_cue_gain(&mut self, gain: f32) {
        let clamped = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            1.0
        };
        self.cue_gain = clamped;
        if self.cue_gain <= 0.0001 {
            self.active_cues.clear();
        }
    }

    pub fn cue_gain(&self) -> f32 {
        self.cue_gain
    }

    pub fn trigger_cue(&mut self, cue: SoundCue) {
        if self.cue_gain <= 0.0001 {
            return;
        }
        if let Some(samples) = self.cue_bank.get(cue) {
            self.active_cues.clear();
            self.active_cues.push(ActiveCueCursor { samples, pos: 0 });
        }
    }

    pub fn has_active_cues(&self) -> bool {
        !self.active_cues.is_empty()
    }

    pub fn has_peers(&self) -> bool {
        !self.peers.is_empty() || !self.stream_peers.is_empty()
    }

    /// Mixes one 10ms frame (480 samples) from active peers, stream audio, loopback, and sound cues.
    /// Returns `true` if any non-silent audio was mixed into `out_frame`.
    pub fn mix_frame(&mut self, deafened: bool, out_frame: &mut [f32; FRAME_SAMPLES]) -> bool {
        out_frame.fill(0.0);
        let mut had_audio = false;

        for channel in self.peers.values_mut() {
            if channel.jitter.pop_pcm_frame(&mut self.peer_scratch) && !deafened {
                let g = channel.gain;
                if g > 0.0001 {
                    had_audio = true;
                    let scaled_gain = g * VOICE_UNITY_SCALE;
                    for (dst, &src) in out_frame.iter_mut().zip(self.peer_scratch.iter()) {
                        *dst += src * scaled_gain;
                    }
                }
            }
        }

        if !deafened && self.stream_gain > 0.0001 {
            for channel in self.stream_peers.values_mut() {
                if channel.jitter.pop_pcm_frame(&mut self.peer_scratch) {
                    had_audio = true;
                    for (dst, &src) in out_frame.iter_mut().zip(self.peer_scratch.iter()) {
                        *dst += src * self.stream_gain;
                    }
                }
            }
        }

        if !deafened && self.loopback_len > 0 {
            had_audio = true;
            let frame = &self.loopback_frames[self.loopback_read];
            for (dst, &src) in out_frame.iter_mut().zip(frame.iter()) {
                *dst += src * LOOPBACK_GAIN;
            }
            self.loopback_read = (self.loopback_read + 1) % 2;
            self.loopback_len -= 1;
        }

        let cue_scale = self.cue_gain * CUE_UNITY_SCALE;
        if cue_scale <= 0.0001 {
            self.active_cues.clear();
        } else {
            self.active_cues.retain_mut(|cursor| {
                let remaining = cursor.samples.len().saturating_sub(cursor.pos);
                let count = remaining.min(FRAME_SAMPLES);
                if count > 0 {
                    had_audio = true;
                }
                for i in 0..count {
                    out_frame[i] += cursor.samples[cursor.pos + i] * cue_scale;
                }
                cursor.pos += count;
                cursor.pos < cursor.samples.len()
            });
        }

        if !had_audio {
            return false;
        }

        self.limiter.process_frame(out_frame);
        true
    }

    pub fn peer_snapshots(&self) -> Vec<(PeerId, JitterSnapshot)> {
        self.peers
            .iter()
            .map(|(&peer_id, ch)| (peer_id, ch.jitter.snapshot()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::OpusEncoder10ms;

    #[test]
    fn test_soft_limiter_untouched_below_threshold() {
        let mut limiter = SoftLimiter::new();
        let mut input = [0.0f32; FRAME_SAMPLES];
        for (i, sample) in input.iter_mut().enumerate() {
            let t = i as f32 / 48_000.0;
            *sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
        }
        let original = input;
        limiter.process_frame(&mut input);

        for (i, (&out_s, &in_s)) in input.iter().zip(original.iter()).enumerate() {
            assert_eq!(
                out_s.to_bits(),
                in_s.to_bits(),
                "Sample {i} <= 0.5 was modified: in={in_s}, out={out_s}"
            );
        }
    }

    #[test]
    fn test_soft_limiter_loud_signals_stay_below_ceiling() {
        let mut limiter = SoftLimiter::new();
        let mut input = [0.0f32; FRAME_SAMPLES];
        for (i, sample) in input.iter_mut().enumerate() {
            let t = i as f32 / 48_000.0;
            *sample = (2.0 * std::f32::consts::PI * 300.0 * t).sin() * 2.2;
        }
        limiter.process_frame(&mut input);

        let max_abs = input.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(
            max_abs < 0.96,
            "Expected loud signal peak strictly < 0.96, got {max_abs}"
        );
        assert!(
            max_abs > 0.70,
            "Expected limiter to preserve strong signal presence > 0.70, got {max_abs}"
        );
    }

    #[test]
    fn test_voice_unity_scale_boosts_unity_gain() {
        let cue_bank = SoundCueBank::load_optional();
        let mut mixer = AudioMixer::new(cue_bank, JitterProfile::LanUltraLow);
        let mut encoder = OpusEncoder10ms::new(64_000).unwrap();

        let mut pcm = [0.0f32; FRAME_SAMPLES];
        for (i, sample) in pcm.iter_mut().enumerate() {
            let t = i as f32 / 48_000.0;
            *sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.15;
        }
        let input_rms = (pcm.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();

        for seq in 1..=5u64 {
            let pkt = encoder.encode_speech_frame(&pcm).unwrap();
            mixer.push_peer_opus(1, seq, pkt);
        }

        let mut out = [0.0f32; FRAME_SAMPLES];
        let mut max_out_rms = 0.0f32;
        for _ in 0..5 {
            if mixer.mix_frame(false, &mut out) {
                let rms = (out.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
                max_out_rms = max_out_rms.max(rms);
            }
        }

        assert!(
            max_out_rms > input_rms * 1.5,
            "Expected VOICE_UNITY_SCALE ({VOICE_UNITY_SCALE}) to boost gain=1.0: input_rms={input_rms}, out_rms={max_out_rms}"
        );
    }

    #[test]
    fn test_loopback_ring_buffer() {
        let cue_bank = SoundCueBank::load_optional();
        let mut mixer = AudioMixer::new(cue_bank, JitterProfile::Balanced);
        assert!(!mixer.has_loopback());

        let f1 = [0.1f32; FRAME_SAMPLES];
        let f2 = [0.2f32; FRAME_SAMPLES];
        let f3 = [0.3f32; FRAME_SAMPLES];
        mixer.push_loopback_frame(&f1);
        mixer.push_loopback_frame(&f2);
        mixer.push_loopback_frame(&f3);
        assert!(mixer.has_loopback());

        let mut out = [0.0f32; FRAME_SAMPLES];
        assert!(mixer.mix_frame(false, &mut out));
        assert!((out[0] - 0.2 * 1.25).abs() < 1e-5);
        assert!(mixer.mix_frame(false, &mut out));
        assert!((out[0] - 0.3 * 1.25).abs() < 1e-5);
        assert!(!mixer.has_loopback());
        assert!(!mixer.mix_frame(false, &mut out));
    }

    #[test]
    fn test_soft_limiter_recovers_from_non_finite_samples() {
        let mut limiter = SoftLimiter::new();
        assert!((limiter.process_sample(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(limiter.process_sample(f32::INFINITY), 0.0);
        assert_eq!(limiter.process_sample(f32::NEG_INFINITY), 0.0);
        assert_eq!(limiter.process_sample(f32::NAN), 0.0);

        let recovered = limiter.process_sample(0.25);
        assert!(recovered.is_finite());
        assert!((recovered - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_cue_gain_scaling_and_mute() {
        let cue_bank = SoundCueBank::load_optional();
        let mut mixer = AudioMixer::new(cue_bank, JitterProfile::Balanced);
        assert!((mixer.cue_gain() - 1.0).abs() < 1e-6);

        mixer.set_cue_gain(-1.0);
        assert_eq!(mixer.cue_gain(), 0.0);
        mixer.set_cue_gain(5.0);
        assert_eq!(mixer.cue_gain(), 2.0);
        mixer.set_cue_gain(f32::NAN);
        assert_eq!(mixer.cue_gain(), 1.0);

        mixer.set_cue_gain(0.0);
        mixer.trigger_cue(SoundCue::Join);
        assert!(!mixer.has_active_cues());
        let mut muted_frame = [1.0f32; FRAME_SAMPLES];
        assert!(!mixer.mix_frame(false, &mut muted_frame));
        assert!(muted_frame.iter().all(|&x| x == 0.0));

        let rms_for_gain = |gain: f32| -> f32 {
            let bank = SoundCueBank::load_optional();
            let mut m = AudioMixer::new(bank, JitterProfile::Balanced);
            m.set_cue_gain(gain);
            m.trigger_cue(SoundCue::Join);
            assert!(m.has_active_cues());
            let mut frame = [0.0f32; FRAME_SAMPLES];
            let mut sum_sq = 0.0f64;
            let mut count = 0usize;
            while m.mix_frame(false, &mut frame) {
                for &s in &frame {
                    sum_sq += f64::from(s) * f64::from(s);
                    count += 1;
                }
            }
            (sum_sq / count.max(1) as f64).sqrt() as f32
        };

        let rms_half = rms_for_gain(0.5);
        let rms_unity = rms_for_gain(1.0);
        let rms_double = rms_for_gain(2.0);
        assert!(rms_unity > 0.001, "Expected audible cue at unity gain");
        let ratio = rms_half / rms_unity;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "Expected ~0.5 RMS ratio for 50% vs 100% cue gain, got {ratio}"
        );
        assert!(
            rms_double > rms_unity,
            "Expected 200% cue gain RMS ({rms_double}) > 100% ({rms_unity})"
        );

        mixer.set_cue_gain(1.0);
        mixer.trigger_cue(SoundCue::Join);
        assert!(mixer.has_active_cues());
        mixer.set_cue_gain(0.0);
        assert!(!mixer.has_active_cues());
    }
}
