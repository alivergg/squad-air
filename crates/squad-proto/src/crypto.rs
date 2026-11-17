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
        let derived = blake3::derive_key(KDF_CONTEXT, secret);
        let key = Key::from_slice(&derived);
        Self {
            cipher: ChaCha20Poly1305::new(key),
        }
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
}
