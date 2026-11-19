use crate::packet::{P2pHeader, P2pPayload, ProtoError, P2P_HEADER_LEN};
use chacha20poly1305::{
    aead::{AeadInPlace, KeyInit},
    ChaCha20Poly1305, Key, Nonce, Tag,
};

pub const TAG_LEN: usize = 16;
const KDF_CONTEXT: &str = "squad-air-e2ee-chacha20poly1305-v1";

#[derive(Clone)]
pub struct RoomCipher {
    cipher: ChaCha20Poly1305,
}

impl RoomCipher {
    /// Derives a 256-bit AEAD key from the 32-byte room secret using BLAKE3.
    pub fn from_secret(secret: &[u8; 32]) -> Self {
        let mut derived = blake3::derive_key(KDF_CONTEXT, secret);
        let key = Key::from_slice(&derived);
        let cipher = ChaCha20Poly1305::new(key);
        for byte in &mut derived {
            unsafe {
                std::ptr::write_volatile(byte, 0);
            }
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
        Self { cipher }
    }

    /// Constructs the deterministic 96-bit nonce from the packet header:
    /// `[sender_peer_id: 2B BE][msg_type: 1B][seq_num: 8B BE][0x00: 1B]`
    #[inline]
    pub fn build_nonce(header: &P2pHeader) -> Nonce {
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[0..2].copy_from_slice(&header.sender_peer_id.to_be_bytes());
        nonce_bytes[2] = header.msg_type as u8;
        nonce_bytes[3..11].copy_from_slice(&header.seq_num.to_be_bytes());
        nonce_bytes[11] = 0x00;
        *Nonce::from_slice(&nonce_bytes)
    }

    /// Encrypts a `P2pPayload` with `header` as cleartext AAD into `out`.
    pub fn seal_packet(
        &self,
        header: &P2pHeader,
        payload: &P2pPayload,
        out: &mut Vec<u8>,
    ) -> Result<(), ProtoError> {
        out.clear();
        let aad = header.encode();
        out.extend_from_slice(&aad);
        payload.encode_body(out)?;

        let nonce = Self::build_nonce(header);
        let body_slice = &mut out[P2P_HEADER_LEN..];
        let tag = self
            .cipher
            .encrypt_in_place_detached(&nonce, &aad, body_slice)
            .map_err(|_| ProtoError::CryptoFailure)?;

        out.extend_from_slice(tag.as_slice());
        if out.len() > crate::packet::MAX_UDP_PAYLOAD {
            return Err(ProtoError::PacketTooLarge(out.len()));
        }
        Ok(())
    }

    /// Verifies and decrypts an incoming UDP datagram in-place, returning `(P2pHeader, P2pPayload)`.
    pub fn open_packet(&self, datagram: &mut [u8]) -> Result<(P2pHeader, P2pPayload), ProtoError> {
        if datagram.len() < P2P_HEADER_LEN + TAG_LEN {
            return Err(ProtoError::PacketTooShort {
                expected: P2P_HEADER_LEN + TAG_LEN,
                actual: datagram.len(),
            });
        }

        let header = P2pHeader::decode(&datagram[..P2P_HEADER_LEN])?;
        let nonce = Self::build_nonce(&header);

        let (aad_and_body, tag_bytes) = datagram.split_at_mut(datagram.len() - TAG_LEN);
        let (aad, ciphertext) = aad_and_body.split_at_mut(P2P_HEADER_LEN);
        let tag = Tag::from_slice(tag_bytes);

        self.cipher
            .decrypt_in_place_detached(&nonce, aad, ciphertext, tag)
            .map_err(|_| ProtoError::CryptoFailure)?;

        let payload = P2pPayload::decode_body(header.msg_type, ciphertext)?;
        Ok((header, payload))
    }
}

/// 128-packet sliding replay window using a bitmask anchored at `highest_seq`.
#[derive(Debug, Clone, Default)]
pub struct ReplayWindow {
    highest_seq: u64,
    bitmap: u128,
    initialized: bool,
}

impl ReplayWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.highest_seq = 0;
        self.bitmap = 0;
        self.initialized = false;
    }

    pub fn check_and_accept(&mut self, seq_num: u64) -> bool {
        if !self.initialized {
            self.initialized = true;
            self.highest_seq = seq_num;
            self.bitmap = 1;
            return true;
        }

        if seq_num > self.highest_seq {
            let shift = seq_num - self.highest_seq;
            if shift >= 128 {
                self.bitmap = 1;
            } else {
                self.bitmap = (self.bitmap << shift) | 1;
            }
            self.highest_seq = seq_num;
            return true;
        }

        let diff = self.highest_seq - seq_num;
        if diff >= 128 {
            return false;
        }

        let mask = 1u128 << diff;
        if (self.bitmap & mask) != 0 {
            return false;
        }
        self.bitmap |= mask;
        true
    }
}

const LARGE_REPLAY_WINDOW_BITS: u64 = 2048;

/// 2048-packet sliding replay window using a 32-word bitmask anchored at `highest_seq`.
#[derive(Debug, Clone, Default)]
pub struct LargeReplayWindow {
    highest_seq: u64,
    words: [u64; 32],
    initialized: bool,
}

impl LargeReplayWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.highest_seq = 0;
        self.words = [0; 32];
        self.initialized = false;
    }

    pub fn check_and_accept(&mut self, seq_num: u64) -> bool {
        if !self.initialized {
            self.initialized = true;
            self.highest_seq = seq_num;
            self.words = [0; 32];
            self.words[0] = 1;
            return true;
        }

        if seq_num > self.highest_seq {
            let shift = seq_num - self.highest_seq;
            if shift >= LARGE_REPLAY_WINDOW_BITS {
                self.words = [0; 32];
                self.words[0] = 1;
            } else {
                let word_shift = (shift / 64) as usize;
                let bit_shift = (shift % 64) as u32;
                for i in (0..32).rev() {
                    let mut val = 0u64;
                    if i >= word_shift {
                        let src = self.words[i - word_shift];
                        val |= src << bit_shift;
                        if bit_shift > 0 && i > word_shift {
                            val |= self.words[i - word_shift - 1] >> (64 - bit_shift);
                        }
                    }
                    self.words[i] = val;
                }
                self.words[0] |= 1;
            }
            self.highest_seq = seq_num;
            return true;
        }

        let diff = self.highest_seq - seq_num;
        if diff >= LARGE_REPLAY_WINDOW_BITS {
            return false;
        }

        let w = (diff / 64) as usize;
        let b = (diff % 64) as u32;
        let mask = 1u64 << b;
        if (self.words[w] & mask) != 0 {
            return false;
        }
        self.words[w] |= mask;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::P2pMsgType;

    #[test]
    fn test_aead_seal_open_and_tamper_detection() {
        let secret = [0x42u8; 32];
        let cipher = RoomCipher::from_secret(&secret);

        let header = P2pHeader {
            msg_type: P2pMsgType::AudioOpus,
            room_prefix: [1, 2, 3, 4],
            sender_peer_id: 3,
            seq_num: 987_654_321,
        };
        let payload = P2pPayload::AudioOpus {
            opus_data: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02],
        };

        let mut wire = Vec::new();
        cipher
            .seal_packet(&header, &payload, &mut wire)
            .expect("seal succeeds");

        // Valid open
        let mut wire_copy = wire.clone();
        let (dec_hdr, dec_payload) = cipher.open_packet(&mut wire_copy).expect("open succeeds");
        assert_eq!(dec_hdr, header);
        assert_eq!(dec_payload, payload);

        // Tamper with AAD header (sender_peer_id)
        let mut tampered_aad = wire.clone();
        tampered_aad[9] ^= 0xFF;
        assert!(matches!(
            cipher.open_packet(&mut tampered_aad),
            Err(ProtoError::CryptoFailure)
        ));

        // Tamper with ciphertext
        let mut tampered_ct = wire.clone();
        tampered_ct[P2P_HEADER_LEN + 2] ^= 0x55;
        assert!(matches!(
            cipher.open_packet(&mut tampered_ct),
            Err(ProtoError::CryptoFailure)
        ));
    }

    #[test]
    fn test_replay_window_ordering_duplicates_and_reset() {
        let mut window = ReplayWindow::new();

        // In-order acceptance starting at seq 0
        assert!(window.check_and_accept(0));
        assert!(window.check_and_accept(1));
        assert!(window.check_and_accept(2));

        // Duplicates rejected
        assert!(!window.check_and_accept(2));
        assert!(!window.check_and_accept(0));

        // Out-of-order within 128-packet window
        assert!(window.check_and_accept(10));
        assert!(window.check_and_accept(5));
        assert!(window.check_and_accept(9));
        assert!(!window.check_and_accept(5));
        assert!(!window.check_and_accept(10));

        // Boundary of 128-packet window: highest_seq = 130 => [3..=130]
        assert!(window.check_and_accept(130));
        assert!(window.check_and_accept(3));
        assert!(!window.check_and_accept(3));
        assert!(!window.check_and_accept(2));

        // Jump >= 128 clears prior window
        assert!(window.check_and_accept(300));
        assert!(!window.check_and_accept(172));
        assert!(window.check_and_accept(173));
        assert!(!window.check_and_accept(130));

        // Reset allows reusing sequence numbers
        window.reset();
        assert!(window.check_and_accept(0));
        assert!(window.check_and_accept(1));
        assert!(window.check_and_accept(130));
        assert!(!window.check_and_accept(130));
    }

    #[test]
    fn test_large_replay_window_in_order_and_duplicates() {
        let mut window = LargeReplayWindow::new();

        for seq in 0..=150 {
            assert!(window.check_and_accept(seq));
        }

        for dup in [0, 1, 63, 64, 65, 128, 150] {
            assert!(!window.check_and_accept(dup));
        }
    }

    #[test]
    fn test_large_replay_window_out_of_order_word_boundaries() {
        let mut window = LargeReplayWindow::new();
        assert!(window.check_and_accept(3000));

        for offset in [63, 64, 65, 128, 1024, 2047] {
            let seq = 3000 - offset;
            assert!(
                window.check_and_accept(seq),
                "expected offset {offset} to be accepted"
            );
            assert!(
                !window.check_and_accept(seq),
                "expected duplicate at offset {offset} to be rejected"
            );
        }
        assert!(!window.check_and_accept(3000 - 2048));

        // Verify bit_shift == 0 (shift by 64) and carry across word boundaries
        let mut shifted = LargeReplayWindow::new();
        assert!(shifted.check_and_accept(0));
        assert!(shifted.check_and_accept(64));
        assert!(shifted.check_and_accept(128));
        assert!(shifted.check_and_accept(129));
        assert!(!shifted.check_and_accept(0));
        assert!(!shifted.check_and_accept(64));
        assert!(!shifted.check_and_accept(128));
        assert!(shifted.check_and_accept(1));
        assert!(shifted.check_and_accept(63));
        assert!(shifted.check_and_accept(65));

        // Advancing by 1 evicts diff 2048 while preserving diff 2047
        let mut edge = LargeReplayWindow::new();
        assert!(edge.check_and_accept(0));
        assert!(edge.check_and_accept(2047));
        assert!(!edge.check_and_accept(0));
        assert!(edge.check_and_accept(2048));
        assert!(!edge.check_and_accept(0));
        assert!(edge.check_and_accept(1));
        assert!(!edge.check_and_accept(1));
    }

    #[test]
    fn test_large_replay_window_forward_jumps_and_reset() {
        let mut window = LargeReplayWindow::new();
        assert!(window.check_and_accept(5));
        assert!(window.check_and_accept(11));

        // Jump forward by 700 (spans 10 words + 60-bit shift with carry into word 11)
        assert!(window.check_and_accept(711));
        assert!(!window.check_and_accept(5));
        assert!(!window.check_and_accept(11));
        assert!(window.check_and_accept(6));
        assert!(!window.check_and_accept(6));
        assert!(window.check_and_accept(12));
        assert!(!window.check_and_accept(12));

        // Jump forward > 2048 clears prior window
        assert!(window.check_and_accept(3211));
        assert!(!window.check_and_accept(711));
        assert!(!window.check_and_accept(3211 - 2048));
        assert!(window.check_and_accept(3211 - 2047));
        assert!(!window.check_and_accept(3211 - 2047));

        // Exact 2048 jump also clears prior window
        assert!(window.check_and_accept(5259));
        assert!(!window.check_and_accept(3211));
        assert!(window.check_and_accept(3212));

        // Reset restores empty initial state
        window.reset();
        assert!(window.check_and_accept(0));
        assert!(window.check_and_accept(1));
        assert!(window.check_and_accept(5259));
        assert!(!window.check_and_accept(0));
        assert!(!window.check_and_accept(5259));
    }
}
