use crate::config::write_file_private;
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    ChaCha20Poly1305, Nonce,
};
use rand::{rngs::OsRng, RngCore};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use squad_proto::{sanitize_display_str, InviteLink, MAX_NICKNAME_LEN};
use std::path::PathBuf;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

pub const FRIEND_CODE_PREFIX: &str = "sqf_";
const FRIEND_INVITE_CONTEXT: &str = "squad-air friend invite x25519 v1";
const SPAKE2_INVITE_CONTEXT: &str = "squad-air spake2 room invite v1";
const SPAKE2_ID_JOINER: &[u8] = b"squad-air-spake2-joiner-v1";
const SPAKE2_ID_HOST: &[u8] = b"squad-air-spake2-host-v1";
const MAX_FRIENDS_STORED: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendContact {
    pub pub_key: [u8; 32],
    pub nickname: String,
    pub added_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptedFriendInvite {
    pub invite: InviteLink,
    pub sender_nickname: String,
    pub is_party_invite: bool,
}

pub fn friends_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|mut dir| {
        dir.push("squad-air");
        dir.push("friends.json");
        dir
    })
}

pub fn load_friends() -> Vec<FriendContact> {
    let Some(path) = friends_file_path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_friends_json(&raw)
}

pub fn save_friends(friends: &[FriendContact]) {
    let Some(path) = friends_file_path() else {
        return;
    };
    let json = serialize_friends_json(friends);
    let _ = write_file_private(&path, json.as_bytes());
}

pub fn format_friend_code(pub_key: &[u8; 32], nickname: &str) -> String {
    let clean_nick = sanitize_display_str(nickname, MAX_NICKNAME_LEN);
    let b58 = bs58::encode(pub_key).into_string();
    if clean_nick.is_empty() {
        format!("{FRIEND_CODE_PREFIX}{b58}")
    } else {
        let encoded_nick = percent_encode_component(&clean_nick);
        format!("{FRIEND_CODE_PREFIX}{b58}#{encoded_nick}")
    }
}

pub fn parse_friend_code(input: &str) -> Result<([u8; 32], Option<String>), String> {
    let trimmed = input.trim();
    if trimmed.len() > 256 {
        return Err("Friend code exceeds maximum length".to_string());
    }
    let Some(body) = trimmed.strip_prefix(FRIEND_CODE_PREFIX) else {
        return Err("Friend code must start with sqf_".to_string());
    };
    let (key_part, nick_part) = match body.split_once('#') {
        Some((k, n)) => (k.trim(), Some(n.trim())),
        None => (body.trim(), None),
    };
    if key_part.is_empty() || key_part.len() > 64 {
        return Err("Invalid friend public key length".to_string());
    }
    let raw_key = bs58::decode(key_part)
        .into_vec()
        .map_err(|_| "Invalid base58 friend public key".to_string())?;
    let pub_key: [u8; 32] = raw_key
        .try_into()
        .map_err(|_| "Friend public key must be 32 bytes".to_string())?;
    if pub_key == [0u8; 32] {
        return Err("Invalid all-zero friend public key".to_string());
    }

    let nick = nick_part.and_then(|n| {
        let decoded = percent_decode_component(n)?;
        let clean = sanitize_display_str(&decoded, MAX_NICKNAME_LEN);
        if clean.is_empty() {
            None
        } else {
            Some(clean)
        }
    });
    Ok((pub_key, nick))
}

pub type SealedFriendInvite = ([u8; 32], [u8; 12], Vec<u8>);
pub type Spake2SealedInvite = (Vec<u8>, [u8; 12], Vec<u8>);

pub fn seal_friend_invite(
    my_secret: &StaticSecret,
    friend_pub: &[u8; 32],
    invite: &InviteLink,
    my_nick: &str,
    is_party_invite: bool,
) -> Result<SealedFriendInvite, String> {
    if *friend_pub == [0u8; 32] {
        return Err("Invalid target public key".to_string());
    }
    let my_pub = PublicKey::from(my_secret).to_bytes();
    let target_pk = PublicKey::from(*friend_pub);
    let eph_sk = EphemeralSecret::random_from_rng(OsRng);
    let eph_pub = PublicKey::from(&eph_sk).to_bytes();

    let ss_es = eph_sk.diffie_hellman(&target_pk);
    let ss_ss = my_secret.diffie_hellman(&target_pk);
    if ss_es.as_bytes().iter().all(|&b| b == 0) || ss_ss.as_bytes().iter().all(|&b| b == 0) {
        return Err("Weak X25519 shared secret rejected".to_string());
    }

    let key = derive_friend_invite_key(ss_es.as_bytes(), ss_ss.as_bytes());
    let cipher = ChaCha20Poly1305::new((&key).into());

    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);

    let clean_nick = sanitize_display_str(my_nick, MAX_NICKNAME_LEN);
    let nick_bytes = clean_nick.as_bytes();
    let mut plaintext = Vec::with_capacity(50 + nick_bytes.len());
    plaintext.extend_from_slice(&invite.room_id);
    plaintext.extend_from_slice(&invite.secret);
    plaintext.push(u8::from(is_party_invite));
    plaintext.push(nick_bytes.len() as u8);
    plaintext.extend_from_slice(nick_bytes);

    let aad = build_friend_invite_aad(&my_pub, friend_pub, &eph_pub);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| "Failed to encrypt friend invite".to_string())?;

    Ok((eph_pub, nonce, ciphertext))
}

pub fn open_friend_invite(
    my_secret: &StaticSecret,
    sender_pub: &[u8; 32],
    eph_pub: &[u8; 32],
    nonce: &[u8; 12],
    ciphertext: &[u8],
) -> Result<DecryptedFriendInvite, String> {
    if *sender_pub == [0u8; 32] || *eph_pub == [0u8; 32] {
        return Err("Invalid sender or ephemeral X25519 key".to_string());
    }
    let my_pub = PublicKey::from(my_secret).to_bytes();
    let sender_pk = PublicKey::from(*sender_pub);
    let eph_pk = PublicKey::from(*eph_pub);

    let ss_es = my_secret.diffie_hellman(&eph_pk);
    let ss_ss = my_secret.diffie_hellman(&sender_pk);
    if ss_es.as_bytes().iter().all(|&b| b == 0) || ss_ss.as_bytes().iter().all(|&b| b == 0) {
        return Err("Weak X25519 shared secret rejected".to_string());
    }

    let key = derive_friend_invite_key(ss_es.as_bytes(), ss_ss.as_bytes());
    let cipher = ChaCha20Poly1305::new((&key).into());
    let aad = build_friend_invite_aad(sender_pub, &my_pub, eph_pub);

    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| "Friend invite authentication failed".to_string())?;

    if plaintext.len() < 50 {
        return Err("Truncated friend invite payload".to_string());
    }
    let mut room_id = [0u8; 16];
    room_id.copy_from_slice(&plaintext[0..16]);
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&plaintext[16..48]);
    let is_party_invite = plaintext[48] != 0;
    let nick_len = plaintext[49] as usize;
    if nick_len > MAX_NICKNAME_LEN || plaintext.len() != 50 + nick_len {
        return Err("Malformed friend invite nickname length".to_string());
    }
    let nick_raw = std::str::from_utf8(&plaintext[50..50 + nick_len])
        .map_err(|_| "Invalid UTF-8 nickname in invite".to_string())?;
    let sender_nickname = sanitize_display_str(nick_raw, MAX_NICKNAME_LEN);

    Ok(DecryptedFriendInvite {
        invite: InviteLink { room_id, secret },
        sender_nickname: if sender_nickname.is_empty() {
            "Friend".to_string()
        } else {
            sender_nickname
        },
        is_party_invite,
    })
}

pub fn spake2_start_joiner(code: u32) -> (Spake2<Ed25519Group>, Vec<u8>) {
    let pw = format!("{code:06}");
    Spake2::<Ed25519Group>::start_a(
        &Password::new(pw.as_bytes()),
        &Identity::new(SPAKE2_ID_JOINER),
        &Identity::new(SPAKE2_ID_HOST),
    )
}

pub fn spake2_host_seal_invite(
    code: u32,
    msg_a: &[u8],
    invite: &InviteLink,
) -> Result<Spake2SealedInvite, String> {
    let pw = format!("{code:06}");
    let (state_b, msg_b) = Spake2::<Ed25519Group>::start_b(
        &Password::new(pw.as_bytes()),
        &Identity::new(SPAKE2_ID_JOINER),
        &Identity::new(SPAKE2_ID_HOST),
    );
    let shared_key = state_b
        .finish(msg_a)
        .map_err(|_| "SPAKE2 host verification failed".to_string())?;
    let aead_key = blake3::derive_key(SPAKE2_INVITE_CONTEXT, &shared_key);
    let cipher = ChaCha20Poly1305::new((&aead_key).into());

    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut plaintext = [0u8; 48];
    plaintext[0..16].copy_from_slice(&invite.room_id);
    plaintext[16..48].copy_from_slice(&invite.secret);
    let aad = code.to_be_bytes();

    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| "Failed to encrypt SPAKE2 room invite".to_string())?;
    Ok((msg_b, nonce, encrypted))
}

pub fn spake2_joiner_open_invite(
    state_a: Spake2<Ed25519Group>,
    code: u32,
    msg_b: &[u8],
    nonce: &[u8; 12],
    encrypted_invite: &[u8],
) -> Result<InviteLink, String> {
    let shared_key = state_a
        .finish(msg_b)
        .map_err(|_| "SPAKE2 exchange failed".to_string())?;
    let aead_key = blake3::derive_key(SPAKE2_INVITE_CONTEXT, &shared_key);
    let cipher = ChaCha20Poly1305::new((&aead_key).into());
    let aad = code.to_be_bytes();

    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: encrypted_invite,
                aad: &aad,
            },
        )
        .map_err(|_| "Invalid 6-digit code or corrupted handshake".to_string())?;

    if plaintext.len() != 48 {
        return Err("Invalid room invite payload length".to_string());
    }
    let mut room_id = [0u8; 16];
    room_id.copy_from_slice(&plaintext[0..16]);
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&plaintext[16..48]);
    Ok(InviteLink { room_id, secret })
}

fn derive_friend_invite_key(ss_es: &[u8], ss_ss: &[u8]) -> [u8; 32] {
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(ss_es);
    ikm[32..].copy_from_slice(ss_ss);
    blake3::derive_key(FRIEND_INVITE_CONTEXT, &ikm)
}

fn build_friend_invite_aad(sender: &[u8; 32], recipient: &[u8; 32], eph: &[u8; 32]) -> [u8; 96] {
    let mut aad = [0u8; 96];
    aad[..32].copy_from_slice(sender);
    aad[32..64].copy_from_slice(recipient);
    aad[64..].copy_from_slice(eph);
    aad
}

fn percent_encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        let safe = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(*byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn percent_decode_component(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let val = u8::from_str_radix(hex, 16).ok()?;
            out.push(val);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn serialize_friends_json(friends: &[FriendContact]) -> String {
    let mut out = String::from("[\n");
    for (idx, friend) in friends.iter().take(MAX_FRIENDS_STORED).enumerate() {
        if idx > 0 {
            out.push_str(",\n");
        }
        let pk_b58 = bs58::encode(friend.pub_key).into_string();
        let nick_escaped = escape_json_str(&friend.nickname);
        use std::fmt::Write;
        let _ = write!(
            out,
            "  {{\"pub_key_b58\":\"{pk_b58}\",\"nickname\":\"{nick_escaped}\",\"added_at_ms\":{}}}",
            friend.added_at_ms
        );
    }
    out.push_str("\n]\n");
    out
}

pub fn parse_friends_json(raw: &str) -> Vec<FriendContact> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(obj_start) = rest.find('{') {
        rest = &rest[obj_start + 1..];
        let Some(obj_end) = rest.find('}') else {
            break;
        };
        let obj = &rest[..obj_end];
        rest = &rest[obj_end + 1..];

        let Some(pk_b58) = extract_json_string_field(obj, "pub_key_b58") else {
            continue;
        };
        let Some(pub_vec) = bs58::decode(pk_b58.trim()).into_vec().ok() else {
            continue;
        };
        let Ok(pub_key) = <[u8; 32]>::try_from(pub_vec) else {
            continue;
        };
        if pub_key == [0u8; 32] {
            continue;
        }

        let nick_raw = extract_json_string_field(obj, "nickname").unwrap_or_default();
        let clean_nick = sanitize_display_str(&nick_raw, MAX_NICKNAME_LEN);
        let nickname = if clean_nick.is_empty() {
            "Friend".to_string()
        } else {
            clean_nick
        };
        let added_at_ms = extract_json_u64_field(obj, "added_at_ms").unwrap_or(0);

        if !out.iter().any(|c: &FriendContact| c.pub_key == pub_key) {
            out.push(FriendContact {
                pub_key,
                nickname,
                added_at_ms,
            });
            if out.len() >= MAX_FRIENDS_STORED {
                break;
            }
        }
    }
    out
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn extract_json_string_field(obj: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let key_pos = obj.find(&needle)?;
    let after_key = obj[key_pos + needle.len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let mut chars = after_colon.chars();
    if chars.next()? != '"' {
        return None;
    }
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => {
                let esc = chars.next()?;
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    other => out.push(other),
                }
            }
            c => out.push(c),
        }
    }
    None
}

fn extract_json_u64_field(obj: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"");
    let key_pos = obj.find(&needle)?;
    let after_key = obj[key_pos + needle.len()..].trim_start();
    let after_colon = after_key.strip_prefix(':')?.trim_start();
    let digits_end = after_colon
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_colon.len());
    after_colon[..digits_end].parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_friend_code_roundtrip() {
        let secret = StaticSecret::random_from_rng(OsRng);
        let pub_bytes = PublicKey::from(&secret).to_bytes();
        let code = format_friend_code(&pub_bytes, "Swift Falcon #1");
        assert!(code.starts_with(FRIEND_CODE_PREFIX));

        let (decoded_pk, decoded_nick) = parse_friend_code(&code).unwrap();
        assert_eq!(decoded_pk, pub_bytes);
        assert_eq!(decoded_nick.as_deref(), Some("Swift Falcon #1"));

        assert!(parse_friend_code("invalid_code").is_err());
        assert!(parse_friend_code("sqf_111").is_err());
    }

    #[test]
    fn test_x25519_friend_invite_seal_open_and_tamper_rejection() {
        let alice_sk = StaticSecret::random_from_rng(OsRng);
        let alice_pk = PublicKey::from(&alice_sk).to_bytes();
        let bob_sk = StaticSecret::random_from_rng(OsRng);
        let bob_pk = PublicKey::from(&bob_sk).to_bytes();
        let mallory_sk = StaticSecret::random_from_rng(OsRng);
        let mallory_pk = PublicKey::from(&mallory_sk).to_bytes();

        let invite = InviteLink::generate();
        let (eph_pk, nonce, mut ct) =
            seal_friend_invite(&alice_sk, &bob_pk, &invite, "Alice", true).unwrap();

        let opened = open_friend_invite(&bob_sk, &alice_pk, &eph_pk, &nonce, &ct).unwrap();
        assert_eq!(opened.invite, invite);
        assert_eq!(opened.sender_nickname, "Alice");
        assert!(opened.is_party_invite);

        // Wrong sender identity must fail AEAD authentication
        assert!(open_friend_invite(&bob_sk, &mallory_pk, &eph_pk, &nonce, &ct).is_err());
        // Wrong recipient secret must fail AEAD authentication
        assert!(open_friend_invite(&mallory_sk, &alice_pk, &eph_pk, &nonce, &ct).is_err());
        // Ciphertext tampering must fail
        ct[0] ^= 0xFF;
        assert!(open_friend_invite(&bob_sk, &alice_pk, &eph_pk, &nonce, &ct).is_err());
    }

    #[test]
    fn test_spake2_short_code_exchange_matches_and_rejects_wrong_code() {
        let invite = InviteLink::generate();
        let (joiner_state, msg_a) = spake2_start_joiner(482_910);
        let (msg_b, nonce, enc) = spake2_host_seal_invite(482_910, &msg_a, &invite).unwrap();
        let opened =
            spake2_joiner_open_invite(joiner_state, 482_910, &msg_b, &nonce, &enc).unwrap();
        assert_eq!(opened, invite);

        // Wrong 6-digit code fails SPAKE2 decryption cleanly without disclosing secret
        let (wrong_state, wrong_a) = spake2_start_joiner(482_911);
        let (host_b, wrong_nonce, wrong_enc) =
            spake2_host_seal_invite(482_910, &wrong_a, &invite).unwrap();
        assert!(
            spake2_joiner_open_invite(wrong_state, 482_911, &host_b, &wrong_nonce, &wrong_enc)
                .is_err()
        );
    }

    #[test]
    fn test_friends_json_roundtrip() {
        let contacts = vec![
            FriendContact {
                pub_key: [0x11; 32],
                nickname: "Alice \"Ace\"".to_string(),
                added_at_ms: 1_700_000_000_000,
            },
            FriendContact {
                pub_key: [0x22; 32],
                nickname: "Боб".to_string(),
                added_at_ms: 1_700_000_000_500,
            },
        ];
        let json = serialize_friends_json(&contacts);
        let parsed = parse_friends_json(&json);
        assert_eq!(parsed, contacts);
    }
}
