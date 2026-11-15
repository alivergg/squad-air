use crate::cues::{SoundCue, SoundCueBank};
use crate::jitter::{JitterBuffer, JitterSnapshot};
use crate::FRAME_SAMPLES;
use squad_proto::PeerId;
use std::collections::HashMap;
use std::sync::Arc;

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
    cue_bank: SoundCueBank,
    active_cues: Vec<ActiveCueCursor>,
    peer_scratch: [f32; FRAME_SAMPLES],
}

impl AudioMixer {
    pub fn new(cue_bank: SoundCueBank) -> Self {
        Self {
            peers: HashMap::with_capacity(10),
            cue_bank,
            active_cues: Vec::with_capacity(4),
            peer_scratch: [0.0; FRAME_SAMPLES],
        }
    }

    pub fn push_peer_opus(&mut self, peer_id: PeerId, seq_num: u64, opus_payload: Vec<u8>) {
        let channel = self.peers.entry(peer_id).or_insert_with(|| PeerChannel {
            jitter: JitterBuffer::new(),
            gain: 1.0,
        });
        channel.jitter.push_packet(seq_num, opus_payload);
    }

    /// Устанавливает индивидуальную громкость участника (`0.0` = 0%, `1.0` = 100%, `2.0` = 200%).
    pub fn set_peer_gain(&mut self, peer_id: PeerId, gain: f32) {
        let clamped = gain.clamp(0.0, 2.0);
        let channel = self.peers.entry(peer_id).or_insert_with(|| PeerChannel {
            jitter: JitterBuffer::new(),
            gain: clamped,
        });
        channel.gain = clamped;
    }

    pub fn remove_peer(&mut self, peer_id: PeerId) {
        self.peers.remove(&peer_id);
    }

    pub fn clear_peers(&mut self) {
        self.peers.clear();
    }

    pub fn trigger_cue(&mut self, cue: SoundCue) {
        if let Some(samples) = self.cue_bank.get(cue) {
            self.active_cues.push(ActiveCueCursor { samples, pos: 0 });
        }
    }

    /// Сводит один 10-миллисекундный кадр (480 сэмплов) со всех говорящих пиров и активных WAV-уведомлений.
    pub fn mix_frame(&mut self, deafened: bool, out_frame: &mut [f32; FRAME_SAMPLES]) {
        out_frame.fill(0.0);

        for channel in self.peers.values_mut() {
            if channel.jitter.pop_pcm_frame(&mut self.peer_scratch) && !deafened {
                let g = channel.gain;
                if g > 0.0001 {
                    for (dst, &src) in out_frame.iter_mut().zip(self.peer_scratch.iter()) {
                        *dst += src * g;
                    }
                }
            }
        }

        // Подмешиваем звуковые уведомления (даже если включен Deafen, чтобы слышать сам факт клика)
        self.active_cues.retain_mut(|cursor| {
            let remaining = cursor.samples.len().saturating_sub(cursor.pos);
            let count = remaining.min(FRAME_SAMPLES);
            for i in 0..count {
                out_frame[i] += cursor.samples[cursor.pos + i] * 0.5;
            }
            cursor.pos += count;
            cursor.pos < cursor.samples.len()
        });

        // Soft-clipping (защита ушей от перегруза при одновременной речи нескольких человек)
        for sample in out_frame.iter_mut() {
            *sample = soft_clip(*sample);
        }
    }

    pub fn peer_snapshots(&self) -> Vec<(PeerId, JitterSnapshot)> {
        self.peers
            .iter()
            .map(|(&peer_id, ch)| (peer_id, ch.jitter.snapshot()))
            .collect()
    }
}

#[inline]
fn soft_clip(x: f32) -> f32 {
    if x.abs() <= 0.85 {
        x
    } else {
        x.tanh()
    }
}