use squad_proto::{MAX_VIDEO_FRAG_BYTES, VIDEO_FLAG_KEYFRAME, VIDEO_FLAG_XOR_FEC};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const VIDEO_FEC_GROUP_SIZE: usize = 5;
pub const VIDEO_DATA_SHARD_SIZE: usize = MAX_VIDEO_FRAG_BYTES - 2; // 1098B leaves 2B length metadata in parity
pub const MAX_VIDEO_FRAGS: usize = 2048;
const FRAME_TIMEOUT: Duration = Duration::from_millis(350);

#[inline]
fn is_serial_older(candidate: u32, reference: u32) -> bool {
    let d = reference.wrapping_sub(candidate);
    d != 0 && d <= 0x8000_0000
}

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
    received_data_count: usize,
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
            received_data_count: 0,
            data_shards: vec![None; frag_count],
            parity_shards: vec![None; num_groups],
            created_at: Instant::now(),
        }
    }

    fn try_recover_fec_group(&mut self, g_idx: usize) {
        let Some(Some(parity)) = self.parity_shards.get(g_idx) else {
            return;
        };
        if parity.len() < 2 {
            return;
        }
        let start = g_idx * VIDEO_FEC_GROUP_SIZE;
        let end = (start + VIDEO_FEC_GROUP_SIZE).min(self.frag_count);
        if start >= end {
            return;
        }
        let mut missing_idx = None;
        let mut missing_count = 0usize;

        for (idx, shard) in self.data_shards.iter().enumerate().take(end).skip(start) {
            if shard.is_none() {
                missing_idx = Some(idx);
                missing_count += 1;
                if missing_count > 1 {
                    return;
                }
            }
        }

        if missing_count != 1 {
            return;
        }
        let Some(target_idx) = missing_idx else {
            return;
        };

        let parity_len_xor = u16::from_be_bytes([parity[0], parity[1]]);
        let mut known_len_xor = 0u16;
        for shard in self.data_shards.iter().take(end).skip(start).flatten() {
            known_len_xor ^= shard.len() as u16;
        }

        let recovered_len = (parity_len_xor ^ known_len_xor) as usize;
        if recovered_len > VIDEO_DATA_SHARD_SIZE {
            return;
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

        if self.data_shards[target_idx].is_none() {
            self.data_shards[target_idx] = Some(recovered);
            self.received_data_count += 1;
        }
    }

    #[inline]
    fn is_complete(&self) -> bool {
        self.received_data_count == self.frag_count
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

pub fn fragment_frame(
    frame_id: u32,
    is_keyframe: bool,
    annex_b_bytes: &[u8],
) -> Vec<VideoPacketOut> {
    if annex_b_bytes.is_empty() {
        return Vec::new();
    }

    let total_data = annex_b_bytes.len().div_ceil(VIDEO_DATA_SHARD_SIZE);
    if total_data > MAX_VIDEO_FRAGS {
        return Vec::new();
    }
    let chunks: Vec<&[u8]> = annex_b_bytes.chunks(VIDEO_DATA_SHARD_SIZE).collect();

    let key_flag = if is_keyframe { VIDEO_FLAG_KEYFRAME } else { 0 };
    let num_groups = total_data.div_ceil(VIDEO_FEC_GROUP_SIZE);
    let mut packets = Vec::with_capacity(total_data + num_groups);

    for offset in 0..VIDEO_FEC_GROUP_SIZE {
        for g_idx in 0..num_groups {
            let idx = g_idx * VIDEO_FEC_GROUP_SIZE + offset;
            if let Some(chunk) = chunks.get(idx) {
                packets.push(VideoPacketOut {
                    frame_id,
                    frag_idx: idx as u16,
                    frag_count: total_data as u16,
                    flags: key_flag,
                    data: chunk.to_vec(),
                });
            }
        }
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
        if frag_count == 0
            || frag_count as usize > MAX_VIDEO_FRAGS
            || data.len() > MAX_VIDEO_FRAG_BYTES
        {
            return (None, false);
        }
        if self.last_completed_frame_id.is_some_and(|done| {
            let d = frame_id.wrapping_sub(done);
            d == 0 || d > 0x8000_0000
        }) {
            return (None, false);
        }

        let is_fec = (flags & VIDEO_FLAG_XOR_FEC) != 0;
        let is_keyframe = (flags & VIDEO_FLAG_KEYFRAME) != 0;
        let count = frag_count as usize;
        let idx = frag_idx as usize;
        let num_groups = count.div_ceil(VIDEO_FEC_GROUP_SIZE);
        if (is_fec && idx >= num_groups) || (!is_fec && idx >= count) {
            return (None, false);
        }

        if let Some(existing) = self.pending.get(&frame_id) {
            if existing.frag_count != count || existing.is_keyframe != is_keyframe {
                return (None, false);
            }
        }

        if self.awaiting_idr && !is_keyframe {
            return (None, true);
        }

        let mut need_pli = false;
        if !self.pending.contains_key(&frame_id) && self.pending.len() >= 8 {
            if let Some(victim_fid) = self
                .pending
                .iter()
                .min_by_key(|(_, st)| (st.is_keyframe, st.created_at))
                .map(|(&fid, _)| fid)
            {
                self.pending.remove(&victim_fid);
                if !is_keyframe {
                    self.awaiting_idr = true;
                    need_pli = true;
                }
            }
        }

        let entry = self
            .pending
            .entry(frame_id)
            .or_insert_with(|| PendingVideoFrame::new(count, is_keyframe));

        if entry.frag_count != count || entry.is_keyframe != is_keyframe {
            return (None, false);
        }

        if is_fec {
            if idx < entry.parity_shards.len() && entry.parity_shards[idx].is_none() {
                entry.parity_shards[idx] = Some(data);
                entry.try_recover_fec_group(idx);
            }
        } else if idx < entry.data_shards.len() && entry.data_shards[idx].is_none() {
            entry.data_shards[idx] = Some(data);
            entry.received_data_count += 1;
            entry.try_recover_fec_group(idx / VIDEO_FEC_GROUP_SIZE);
        }

        let now = Instant::now();

        if entry.is_complete() {
            let completed = self.pending.remove(&frame_id).unwrap().assemble(frame_id);

            if completed.is_keyframe {
                let stale_keys: Vec<u32> = self
                    .pending
                    .keys()
                    .copied()
                    .filter(|&k| is_serial_older(k, frame_id))
                    .collect();
                for k in stale_keys {
                    self.pending.remove(&k);
                }
                self.last_completed_frame_id = Some(frame_id);
                self.awaiting_idr = false;
                return (Some(completed), false);
            }

            let older_inflight_idr = self.pending.iter().any(|(&k, st)| {
                is_serial_older(k, frame_id)
                    && st.is_keyframe
                    && now.duration_since(st.created_at) < FRAME_TIMEOUT
            });
            if older_inflight_idr {
                return (None, false);
            }

            let stale_keys: Vec<u32> = self
                .pending
                .keys()
                .copied()
                .filter(|&k| is_serial_older(k, frame_id))
                .collect();
            let dropped_older = !stale_keys.is_empty();
            for k in stale_keys {
                self.pending.remove(&k);
            }
            let gap = self
                .last_completed_frame_id
                .is_some_and(|prev| frame_id.wrapping_sub(prev) > 1);

            if self.awaiting_idr || dropped_older || gap {
                self.awaiting_idr = true;
                return (None, true);
            }

            self.last_completed_frame_id = Some(frame_id);
            return (Some(completed), false);
        }

        let expired: Vec<u32> = self
            .pending
            .iter()
            .filter_map(|(&fid, st)| {
                if is_serial_older(fid, frame_id)
                    && now.duration_since(st.created_at) >= FRAME_TIMEOUT
                {
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

    #[test]
    fn test_decreasing_frame_id_pending_cap_evicts_oldest() {
        let mut reassembler = FrameReassembler::new();
        for fid in (80..=100).rev() {
            let (out, _) = reassembler.push_fragment(fid, 0, 2, VIDEO_FLAG_KEYFRAME, vec![1; 16]);
            assert!(out.is_none());
            assert!(reassembler.pending.len() <= 8);
        }
        assert_eq!(reassembler.pending.len(), 8);
    }

    #[test]
    fn test_invalid_frag_idx_rejected_without_pending() {
        let mut reassembler = FrameReassembler::new();
        assert_eq!(
            reassembler.push_fragment(1, 3, 3, VIDEO_FLAG_KEYFRAME, vec![1; 16]),
            (None, false)
        );
        assert!(reassembler.pending.is_empty());

        assert_eq!(
            reassembler.push_fragment(
                2,
                1,
                5,
                VIDEO_FLAG_KEYFRAME | VIDEO_FLAG_XOR_FEC,
                vec![1; 16]
            ),
            (None, false)
        );
        assert!(reassembler.pending.is_empty());

        assert_eq!(
            reassembler.push_fragment(
                3,
                0,
                (MAX_VIDEO_FRAGS + 1) as u16,
                VIDEO_FLAG_KEYFRAME,
                vec![1; 16]
            ),
            (None, false)
        );
        assert!(reassembler.pending.is_empty());

        let too_large = vec![0u8; MAX_VIDEO_FRAGS * VIDEO_DATA_SHARD_SIZE + 1];
        assert!(fragment_frame(4, true, &too_large).is_empty());

        assert_eq!(
            reassembler.push_fragment(5, 0, 2, VIDEO_FLAG_KEYFRAME, vec![1; 16]),
            (None, false)
        );
        assert_eq!(
            reassembler.push_fragment(5, 1, 2, 0, vec![2; 16]),
            (None, false)
        );
        assert_eq!(reassembler.pending.len(), 1);
    }

    #[test]
    fn test_u32_max_wraparound_continuity() {
        let payload: Vec<u8> = (0..1_500).map(|i| (i % 211) as u8).collect();
        let mut reassembler = FrameReassembler::new();

        for (fid, is_key) in [
            (u32::MAX - 1, true),
            (u32::MAX, false),
            (0, false),
            (1, false),
        ] {
            let mut completed = None;
            for pkt in fragment_frame(fid, is_key, &payload) {
                let (out, pli) = reassembler.push_fragment(
                    pkt.frame_id,
                    pkt.frag_idx,
                    pkt.frag_count,
                    pkt.flags,
                    pkt.data,
                );
                assert!(!pli);
                if let Some(frame) = out {
                    completed = Some(frame);
                }
            }
            let frame = completed.expect("contiguous frame across u32::MAX must assemble");
            assert_eq!(frame.frame_id, fid);
            assert_eq!(frame.annex_b_bytes, payload);
        }

        assert_eq!(
            reassembler.push_fragment(u32::MAX, 0, 1, 0, vec![0; 16]),
            (None, false)
        );

        let mut gap_pli = false;
        for pkt in fragment_frame(3, false, &payload) {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(out.is_none());
            gap_pli |= pli;
        }
        assert!(gap_pli);
    }

    #[test]
    fn test_p_frame_during_awaiting_idr_does_not_kill_inflight_idr() {
        let idr_payload: Vec<u8> = (0..12_000).map(|i| ((i * 13) % 251) as u8).collect();
        let p_payload: Vec<u8> = (0..600).map(|i| (i % 197) as u8).collect();
        let mut reassembler = FrameReassembler::new();

        let idr_packets = fragment_frame(10, true, &idr_payload);
        let split = idr_packets.len() / 2;

        for pkt in idr_packets.iter().take(split) {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data.clone(),
            );
            assert!(out.is_none());
            assert!(!pli);
        }

        // Complete P-frame #11 arrives before IDR #10 finishes: must NOT evict or poison IDR #10.
        for pkt in fragment_frame(11, false, &p_payload) {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(out.is_none());
            assert!(pli);
        }

        let mut completed_idr = None;
        for pkt in idr_packets.into_iter().skip(split) {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(!pli);
            if let Some(frame) = out {
                completed_idr = Some(frame);
            }
        }
        let frame = completed_idr.expect("In-flight IDR #10 must survive early P-frame #11");
        assert_eq!(frame.frame_id, 10);
        assert!(frame.is_keyframe);
        assert_eq!(frame.annex_b_bytes, idr_payload);

        // Also verify periodic GOP IDR #12 in-flight is not killed if P-frame #13 overtakes it.
        let gop_idr_packets = fragment_frame(12, true, &idr_payload);
        let gop_split = gop_idr_packets.len() / 2;
        for pkt in gop_idr_packets.iter().take(gop_split) {
            let (out, _) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data.clone(),
            );
            assert!(out.is_none());
        }
        for pkt in fragment_frame(13, false, &p_payload) {
            let (out, _) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(out.is_none());
        }
        let mut completed_gop_idr = None;
        for pkt in gop_idr_packets.into_iter().skip(gop_split) {
            let (out, pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(!pli);
            if let Some(f) = out {
                completed_gop_idr = Some(f);
            }
        }
        let gop_frame = completed_gop_idr.expect("In-flight GOP IDR #12 must complete");
        assert_eq!(gop_frame.frame_id, 12);
        assert!(gop_frame.is_keyframe);
        assert_eq!(gop_frame.annex_b_bytes, idr_payload);
    }

    #[test]
    fn test_interleaved_fec_recovers_consecutive_burst_loss() {
        let payload: Vec<u8> = (0..20_000).map(|i| ((i * 29) % 251) as u8).collect();
        let packets = fragment_frame(42, true, &payload);

        // Drop 3 consecutive packets in the transmitted stream (indices 3, 4, 5).
        // Because data shards are interleaved across 4 FEC groups, each dropped packet belongs
        // to a different FEC group and all 3 are recovered via XOR-FEC without PLI.
        let mut reassembler = FrameReassembler::new();
        let mut recovered = None;
        for (tx_idx, pkt) in packets.into_iter().enumerate() {
            if (3..6).contains(&tx_idx) {
                continue;
            }
            let (out, need_pli) = reassembler.push_fragment(
                pkt.frame_id,
                pkt.frag_idx,
                pkt.frag_count,
                pkt.flags,
                pkt.data,
            );
            assert!(!need_pli);
            if let Some(frame) = out {
                recovered = Some(frame);
            }
        }

        let frame = recovered.expect("Interleaved XOR-FEC must recover 3-packet consecutive burst");
        assert_eq!(frame.frame_id, 42);
        assert!(frame.is_keyframe);
        assert_eq!(frame.annex_b_bytes, payload);
    }
}
