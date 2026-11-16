use crate::codec::OpusDecoder10ms;
use crate::FRAME_SAMPLES;
use std::collections::BTreeMap;

const TARGET_BUFFER_FRAMES: usize = 3; // 30 ms (leaves 20 ms steady jitter cushion)
const MAX_BUFFER_FRAMES: usize = 10; // 100 ms max burst tolerance
const MAX_PLC_CONSECUTIVE: u32 = 3; // 30 ms max PLC before fade-out
const SPEAKING_RMS_THRESHOLD: f32 = 0.0055; // ~ -45 dBFS

#[derive(Debug, Clone, Copy, Default)]
pub struct JitterSnapshot {
    pub loss_pct: f32,
    pub is_speaking: bool,
}

pub struct JitterBuffer {
    decoder: OpusDecoder10ms,
    packets: BTreeMap<u64, Vec<u8>>,
    next_expected_seq: Option<u64>,
    primed: bool,
    consecutive_missing: u32,
    received_window: u32,
    lost_window: u32,
    loss_pct_ewma: f32,
    speaking_hold_frames: u8,
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self {
            decoder: OpusDecoder10ms::new().expect("OpusDecoder10ms init"),
            packets: BTreeMap::new(),
            next_expected_seq: None,
            primed: false,
            consecutive_missing: 0,
            received_window: 0,
            lost_window: 0,
            loss_pct_ewma: 0.0,
            speaking_hold_frames: 0,
        }
    }

    pub fn push_packet(&mut self, seq_num: u64, opus_payload: Vec<u8>) {
        if let Some(expected) = self.next_expected_seq {
            if seq_num < expected {
                // Пакет опоздал сильнее текущего окна воспроизведения — отбрасываем
                return;
            }
            // Если после долгого молчания (DTX) seq_num прыгнул далеко вперед — перепривязываем окно
            if seq_num.saturating_sub(expected) > 25 {
                self.packets.clear();
                self.next_expected_seq = Some(seq_num);
                self.primed = false;
            }
        }

        self.packets.insert(seq_num, opus_payload);

        // Ограничиваем верхний предел буфера, чтобы задержка никогда не копилась
        while self.packets.len() > MAX_BUFFER_FRAMES {
            if let Some((&oldest_seq, _)) = self.packets.iter().next() {
                self.packets.remove(&oldest_seq);
                self.next_expected_seq = Some(oldest_seq + 1);
            }
        }

        if self.packets.len() >= TARGET_BUFFER_FRAMES {
            self.primed = true;
        }
    }

    /// Вызывается микшером каждые 10 мс. Возвращает `true`, если кадр содержит активный звук.
    pub fn pop_pcm_frame(&mut self, out_pcm: &mut [f32; FRAME_SAMPLES]) -> bool {
        if !self.primed {
            out_pcm.fill(0.0);
            if self.speaking_hold_frames > 0 {
                self.speaking_hold_frames -= 1;
            }
            return false;
        }

        let seq = match self.next_expected_seq {
            Some(s) => s,
            None => {
                if let Some(&first_seq) = self.packets.keys().next() {
                    self.next_expected_seq = Some(first_seq);
                    first_seq
                } else {
                    self.primed = false;
                    out_pcm.fill(0.0);
                    return false;
                }
            }
        };

        if let Some(opus_bytes) = self.packets.remove(&seq) {
            self.next_expected_seq = Some(seq + 1);
            self.consecutive_missing = 0;
            self.received_window += 1;
            self.update_loss_stats();

            if self.decoder.decode_frame(&opus_bytes, out_pcm).is_err() {
                out_pcm.fill(0.0);
                return false;
            }
            self.update_speaking_flag(out_pcm);
            true
        } else {
            self.consecutive_missing += 1;
            if !self.packets.is_empty() {
                self.next_expected_seq = Some(seq + 1);
                self.lost_window += 1;
                self.update_loss_stats();
                let next_pkt = self.packets.get(&(seq + 1)).map(|v| v.as_slice());
                if self.decoder.decode_missing(next_pkt, out_pcm).is_err() {
                    out_pcm.fill(0.0);
                    return false;
                }
                self.update_speaking_flag(out_pcm);
                true
            } else if self.consecutive_missing <= MAX_PLC_CONSECUTIVE {
                self.next_expected_seq = Some(seq);
                if self.decoder.decode_missing(None, out_pcm).is_err() {
                    out_pcm.fill(0.0);
                    return false;
                }
                self.update_speaking_flag(out_pcm);
                true
            } else {
                self.primed = false;
                self.next_expected_seq = None;
                out_pcm.fill(0.0);
                if self.speaking_hold_frames > 0 {
                    self.speaking_hold_frames -= 1;
                }
                false
            }
        }
    }

    fn update_loss_stats(&mut self) {
        let total = self.received_window + self.lost_window;
        if total >= 50 {
            let instant_loss = (self.lost_window as f32 / total as f32) * 100.0;
            self.loss_pct_ewma = self.loss_pct_ewma * 0.7 + instant_loss * 0.3;
            self.received_window = 0;
            self.lost_window = 0;
        }
    }

    fn update_speaking_flag(&mut self, pcm: &[f32; FRAME_SAMPLES]) {
        let rms = (pcm.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
        if rms >= SPEAKING_RMS_THRESHOLD {
            self.speaking_hold_frames = 15; // 150 мс удержания кольца говорящего в UI
        } else if self.speaking_hold_frames > 0 {
            self.speaking_hold_frames -= 1;
        }
    }

    pub fn snapshot(&self) -> JitterSnapshot {
        JitterSnapshot {
            loss_pct: self.loss_pct_ewma,
            is_speaking: self.speaking_hold_frames > 0,
        }
    }
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::OpusEncoder10ms;

    #[test]
    fn test_jitter_buffer_late_packet_and_short_pause_no_drop() {
        let mut encoder = OpusEncoder10ms::new(40_000).unwrap();
        let mut pcm = [0.0f32; FRAME_SAMPLES];
        for (i, sample) in pcm.iter_mut().enumerate() {
            let t = i as f32 / 48_000.0;
            *sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
        }

        let mut packets = Vec::new();
        for _ in 0..6 {
            packets.push(encoder.encode_speech_frame(&pcm).unwrap());
        }

        let mut jb = JitterBuffer::new();
        jb.push_packet(1, packets[0].clone());
        jb.push_packet(2, packets[1].clone());
        jb.push_packet(3, packets[2].clone());

        let mut out = [0.0f32; FRAME_SAMPLES];
        assert!(jb.pop_pcm_frame(&mut out));
        assert!(jb.pop_pcm_frame(&mut out));
        assert!(jb.pop_pcm_frame(&mut out));

        // Buffer is now empty and expects seq 4; one empty tick triggers PLC without skipping seq 4.
        assert!(jb.pop_pcm_frame(&mut out));
        assert_eq!(jb.next_expected_seq, Some(4));

        // Delayed seq 4 arrives and must be accepted rather than dropped as stale.
        jb.push_packet(4, packets[3].clone());
        assert!(jb.pop_pcm_frame(&mut out));
        assert_eq!(jb.next_expected_seq, Some(5));
        let rms = (out.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
        assert!(rms > 0.05, "Expected decoded audio for late seq 4, got rms={rms}");
    }
}