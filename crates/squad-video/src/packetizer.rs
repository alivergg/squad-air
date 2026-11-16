use squad_proto::{MAX_VIDEO_FRAG_BYTES, VIDEO_FLAG_KEYFRAME, VIDEO_FLAG_XOR_FEC};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const VIDEO_FEC_GROUP_SIZE: usize = 5;
pub const VIDEO_DATA_SHARD_SIZE: usize = MAX_VIDEO_FRAG_BYTES - 2; // 1098B leaves 2B length metadata in parity
const FRAME_TIMEOUT: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPacketOut {
    pub frame_id: u32,
    pub frag_idx: u16,
    pub frag_count: u16,
    pub flags: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReassembledFrame {
    pub frame_id: u32,
    pub is_keyframe: bool,
    pub annex_b_bytes: Vec<u8>,
}

struct PendingVideoFrame {
    frag_count: usize,
    is_keyframe: bool,
    data_shards: Vec<Option<Vec<u8>>>,
    parity_shards: Vec<Option<Vec<u8>>>,
    created_at: Instant,
}

impl PendingVideoFrame {
    fn new(frag_count: usize, is_keyframe: bool) -> Self {
        let num_groups = frag_count.div_ceil(VIDEO_FEC_GROUP_SIZE);
        Self {
            frag_count,
            is_keyframe,
            data_shards: vec![None; frag_count],
            parity_shards: vec![None; num_groups],
            created_at: Instant::now(),
        }
    }

    fn try_recover_fec(&mut self) {
        for (g_idx, parity_opt) in self.parity_shards.iter().enumerate() {
            let Some(parity) = parity_opt else {
                continue;
            };
            if parity.len() < 2 {
                continue;
            }
            let start = g_idx * VIDEO_FEC_GROUP_SIZE;
            let end = (start + VIDEO_FEC_GROUP_SIZE).min(self.frag_count);
            let mut missing_idx = None;
            let mut missing_count = 0usize;

            for (idx, shard) in self.data_shards.iter().enumerate().take(end).skip(start) {
                if shard.is_none() {
                    missing_idx = Some(idx);
                    missing_count += 1;
                    if missing_count > 1 {
                        break;
                    }
                }
            }

            if missing_count != 1 {
                continue;
            }
            let Some(target_idx) = missing_idx else {
                continue;
            };

            let parity_len_xor = u16::from_be_bytes([parity[0], parity[1]]);
            let mut known_len_xor = 0u16;
            for shard in self.data_shards.iter().take(end).skip(start).flatten() {
                known_len_xor ^= shard.len() as u16;
            }

            let recovered_len = (parity_len_xor ^ known_len_xor) as usize;
            if recovered_len > VIDEO_DATA_SHARD_SIZE {
                continue;
            }

            let parity_payload = &parity[2..];
            let mut recovered = vec![0u8; recovered_len];
            let copy_len = recovered_len.min(parity_payload.len());
            recovered[..copy_len].copy_from_slice(&parity_payload[..copy_len]);

            for shard in self.data_shards.iter().take(end).skip(start).flatten() {
                let overlap = recovered_len.min(shard.len());
                for i in 0..overlap {
                    recovered[i] ^= shard[i];
                }
            }

            self.data_shards[target_idx] = Some(recovered);
        }
    }

    fn is_complete(&self) -> bool {
        self.data_shards.iter().all(Option::is_some)
    }

    fn assemble(self, frame_id: u32) -> ReassembledFrame {
        let total_len: usize = self
            .data_shards
            .iter()
            .filter_map(|s| s.as_ref().map(Vec::len))
            .sum();
        let mut out = Vec::with_capacity(total_len);
        for shard in self.data_shards.into_iter().flatten() {
            out.extend_from_slice(&shard);
        }
        ReassembledFrame {
            frame_id,
            is_keyframe: self.is_keyframe,
            annex_b_bytes: out,
        }
    }
}

pub fn fragment_frame(frame_id: u32, is_keyframe: bool, annex_b_bytes: &[u8]) -> Vec<VideoPacketOut> {
    if annex_b_bytes.is_empty() {
        return Vec::new();
    }

    let chunks: Vec<&[u8]> = annex_b_bytes.chunks(VIDEO_DATA_SHARD_SIZE).collect();
    let total_data = chunks.len();
    if total_data > u16::MAX as usize {
        return Vec::new();
    }

    let key_flag = if is_keyframe { VIDEO_FLAG_KEYFRAME } else { 0 };
    let num_groups = total_data.div_ceil(VIDEO_FEC_GROUP_SIZE);
    let mut packets = Vec::with_capacity(total_data + num_groups);

    for (idx, chunk) in chunks.iter().enumerate() {
        packets.push(VideoPacketOut {
            frame_id,
            frag_idx: idx as u16,
            frag_count: total_data as u16,
            flags: key_flag,
            data: chunk.to_vec(),
        });
    }

    for g_idx in 0..num_groups {
        let start = g_idx * VIDEO_FEC_GROUP_SIZE;
        let end = (start + VIDEO_FEC_GROUP_SIZE).min(total_data);
        let mut len_xor = 0u16;
        let mut max_len = 0usize;

        for chunk in &chunks[start..end] {
            len_xor ^= chunk.len() as u16;
            max_len = max_len.max(chunk.len());
        }

        let mut parity = vec![0u8; 2 + max_len];
        parity[0..2].copy_from_slice(&len_xor.to_be_bytes());
        for chunk in &chunks[start..end] {
            for (i, &b) in chunk.iter().enumerate() {
                parity[2 + i] ^= b;
            }
        }

        packets.push(VideoPacketOut {
            frame_id,
            frag_idx: g_idx as u16,
            frag_count: total_data as u16,
            flags: key_flag | VIDEO_FLAG_XOR_FEC,
            data: parity,
        });
    }

    packets
}

pub struct FrameReassembler {
    pending: BTreeMap<u32, PendingVideoFrame>,
    last_completed_frame_id: Option<u32>,
    awaiting_idr: bool,
}

impl FrameReassembler {
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            last_completed_frame_id: None,
            awaiting_idr: true,
        }
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.last_completed_frame_id = None;
        self.awaiting_idr = true;
    }

    pub fn reset_and_shrink(&mut self) {
        self.pending = BTreeMap::new();
        self.last_completed_frame_id = None;
        self.awaiting_idr = true;
    }

    /// Feeds one `VideoFrameFrag` packet and returns `(Some(frame), need_pli)`.
    pub fn push_fragment(
        &mut self,
        frame_id: u32,
        frag_idx: u16,
        frag_count: u16,
        flags: u8,
        data: Vec<u8>,
    ) -> (Option<ReassembledFrame>, bool) {
        if frag_count == 0 || frag_count as usize > 2048 || data.len() > MAX_VIDEO_FRAG_BYTES {
            return (None, false);
        }
        if self
            .last_completed_frame_id
            .is_some_and(|done| frame_id <= done)
        {
            return (None, false);
        }

        let is_fec = (flags & VIDEO_FLAG_XOR_FEC) != 0;
        let is_keyframe = (flags & VIDEO_FLAG_KEYFRAME) != 0;
        let count = frag_count as usize;
        let idx = frag_idx as usize;

        let entry = self
            .pending
            .entry(frame_id)
            .or_insert_with(|| PendingVideoFrame::new(count, is_keyframe));

        if entry.frag_count != count {
            return (None, false);
        }
        entry.is_keyframe |= is_keyframe;

        if is_fec {
            if idx < entry.parity_shards.len() && entry.parity_shards[idx].is_none() {
                entry.parity_shards[idx] = Some(data);
            }
        } else if idx < entry.data_shards.len() && entry.data_shards[idx].is_none() {
            entry.data_shards[idx] = Some(data);
        }

        entry.try_recover_fec();

        if entry.is_complete() {
            let completed = self.pending.remove(&frame_id).unwrap().assemble(frame_id);
            let stale_keys: Vec<u32> = self
                .pending
                .keys()
                .copied()
                .filter(|&k| k < frame_id)
                .collect();
            let dropped_older = !stale_keys.is_empty();
            for k in stale_keys {
                self.pending.remove(&k);
            }
            let gap = self
                .last_completed_frame_id
                .is_some_and(|prev| frame_id > prev + 1);
            self.last_completed_frame_id = Some(frame_id);

            if completed.is_keyframe {
                self.awaiting_idr = false;
                return (Some(completed), false);
            }

            if self.awaiting_idr || dropped_older || gap {
                self.awaiting_idr = true;
                return (None, true);
            }

            return (Some(completed), false);
        }

        let mut need_pli = false;
        let now = Instant::now();
        let expired: Vec<u32> = self
            .pending
            .iter()
            .filter_map(|(&fid, st)| {
                if fid < frame_id && now.duration_since(st.created_at) >= FRAME_TIMEOUT {
                    Some(fid)
                } else {
                    None
                }
            })
            .collect();
        for fid in expired {
            self.pending.remove(&fid);
            self.awaiting_idr = true;
            need_pli = true;
        }

        while self.pending.len() > 8 {
            if let Some((&oldest, _)) = self.pending.iter().next() {
                if oldest == frame_id {
                    break;
                }
                self.pending.remove(&oldest);
                self.awaiting_idr = true;
                need_pli = true;
            }
        }

        if self.awaiting_idr && !is_keyframe {
            need_pli = true;
        }

        (None, need_pli)
    }
}

impl Default for FrameReassembler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xor_fec_recovers_one_missing_fragment_per_five_group() {
        let payload: Vec<u8> = (0..5_123).map(|i| (i % 251) as u8).collect();
        let packets = fragment_frame(7, true, &payload);
        assert!(packets.iter().all(|p| p.data.len() <= MAX_VIDEO_FRAG_BYTES));

        let mut reassembler = FrameReassembler::new();
        let mut recovered = None;

        // Drop fragment #2 from first 5-packet group; keep parity and remaining shards.
        for pkt in packets {
            if pkt.flags & VIDEO_FLAG_XOR_FEC == 0 && pkt.frag_idx == 2 {
                continue;
            }
            let (frame_opt, need_pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(!need_pli);
            if let Some(frame) = frame_opt {
                recovered = Some(frame);
            }
        }

        let frame = recovered.expect("XOR-FEC should recover dropped shard #2");
        assert_eq!(frame.frame_id, 7);
        assert!(frame.is_keyframe);
        assert_eq!(frame.annex_b_bytes, payload);
    }

    #[test]
    fn test_xor_fec_recovers_short_final_shard() {
        let payload: Vec<u8> = (0..2_500).map(|i| ((i * 7) % 253) as u8).collect();
        let packets = fragment_frame(9, true, &payload);
        let total_data = packets[0].frag_count;

        let mut reassembler = FrameReassembler::new();
        let mut recovered = None;
        for pkt in packets {
            if pkt.flags & VIDEO_FLAG_XOR_FEC == 0 && pkt.frag_idx == total_data - 1 {
                continue;
            }
            let (frame_opt, _) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            if let Some(frame) = frame_opt {
                recovered = Some(frame);
            }
        }

        let frame = recovered.expect("XOR-FEC should recover short trailing shard");
        assert_eq!(frame.annex_b_bytes, payload);
    }

    #[test]
    fn test_strict_idr_gating_rejects_p_frames_until_keyframe() {
        let payload: Vec<u8> = (0..1_500).map(|i| (i % 199) as u8).collect();
        let mut reassembler = FrameReassembler::new();

        // P-frame arriving before any IDR must be dropped and trigger PLI.
        let p1 = fragment_frame(1, false, &payload);
        let mut got_pli = false;
        for pkt in p1 {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(out.is_none());
            got_pli |= pli;
        }
        assert!(got_pli);

        // IDR clears awaiting_idr gate.
        let idr = fragment_frame(2, true, &payload);
        let mut got_idr = false;
        for pkt in idr {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(!pli);
            if let Some(frame) = out {
                assert!(frame.is_keyframe);
                got_idr = true;
            }
        }
        assert!(got_idr);

        // Contiguous P-frame #3 passes through cleanly.
        let p3 = fragment_frame(3, false, &payload);
        let mut got_p3 = false;
        for pkt in p3 {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(!pli);
            if out.is_some() {
                got_p3 = true;
            }
        }
        assert!(got_p3);

        // Simulated dropped frame #4 followed by P-frame #5 must lock back into awaiting_idr + PLI.
        let p5 = fragment_frame(5, false, &payload);
        let mut p5_pli = false;
        for pkt in p5 {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(out.is_none());
            p5_pli |= pli;
        }
        assert!(p5_pli);
    }
}
