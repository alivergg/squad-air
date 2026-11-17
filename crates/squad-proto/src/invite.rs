use crate::packet::{ProtoError, RoomId};
use rand::RngCore;

pub const INVITE_SCHEME: &str = "squad://";
const LEGACY_SCHEME: &str = "voice://";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    pub room_id: RoomId,
    pub secret: [u8; 32],
}

impl InviteLink {
    /// Generates a cryptographically random 128-bit room_id and 256-bit room secret.
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let mut room_id = [0u8; 16];
        let mut secret = [0u8; 32];
        rng.fill_bytes(&mut room_id);
        rng.fill_bytes(&mut secret);
        Self { room_id, secret }
    }

    /// Formats the invite link as `squad://<base58_room_id>#<base58_secret>`.
    pub fn to_uri(&self) -> String {
        let room_b58 = bs58::encode(&self.room_id).into_string();
        let secret_b58 = bs58::encode(&self.secret).into_string();
        format!("{INVITE_SCHEME}{room_b58}#{secret_b58}")
    }

    /// Parses `squad://<base58_room_id>#<base58_secret>` (also accepts `voice://`).
    pub fn parse(input: &str) -> Result<Self, ProtoError> {
        let trimmed = input.trim();
        let body = if let Some(rest) = trimmed.strip_prefix(INVITE_SCHEME) {
            rest
        } else if let Some(rest) = trimmed.strip_prefix(LEGACY_SCHEME) {
            rest
        } else {
            return Err(ProtoError::InvalidInvite(
                "missing squad:// or voice:// scheme".into(),
            ));
        };

        let (room_str, secret_str) = body
            .split_once('#')
            .ok_or_else(|| ProtoError::InvalidInvite("missing '#' secret separator".into()))?;

        let room_vec = bs58::decode(room_str)
            .into_vec()
            .map_err(|e| ProtoError::InvalidInvite(format!("invalid room base58: {e}")))?;
        let secret_vec = bs58::decode(secret_str)
            .into_vec()
            .map_err(|e| ProtoError::InvalidInvite(format!("invalid secret base58: {e}")))?;

        let room_id: [u8; 16] = room_vec
            .try_into()
            .map_err(|_| ProtoError::InvalidInvite("room_id must be exactly 16 bytes".into()))?;
        let secret: [u8; 32] = secret_vec
            .try_into()
            .map_err(|_| ProtoError::InvalidInvite("secret must be exactly 32 bytes".into()))?;

        Ok(Self { room_id, secret })
    }

    /// Returns the 4-byte room prefix used in cleartext P2P headers for fast demuxing.
    pub fn room_prefix(&self) -> [u8; 4] {
        [
            self.room_id[0],
            self.room_id[1],
            self.room_id[2],
            self.room_id[3],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invite_roundtrip_and_legacy_scheme() {
        let invite = InviteLink::generate();
        let uri = invite.to_uri();
        assert!(uri.starts_with("squad://"));

        let parsed = InviteLink::parse(&uri).expect("should parse valid squad:// URI");
        assert_eq!(invite, parsed);

        let legacy_uri = uri.replacen("squad://", "voice://", 1);
        let parsed_legacy =
            InviteLink::parse(&legacy_uri).expect("should parse legacy voice:// URI");
        assert_eq!(invite, parsed_legacy);
    }

    #[test]
    fn test_invite_rejects_malformed() {
        assert!(InviteLink::parse("http://bad#link").is_err());
        assert!(InviteLink::parse("squad://no_hash_separator").is_err());
        assert!(InviteLink::parse("squad://1111#2222").is_err());
    }
}
