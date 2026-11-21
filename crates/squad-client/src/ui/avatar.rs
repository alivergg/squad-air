use iced::widget::image as iced_image;
use squad_proto::{ClientUuid, RoomId};
use std::collections::HashMap;
use std::sync::Mutex;

pub use squad_core::avatar_core::*;

const MAX_AVATAR_CACHE_ENTRIES: usize = 256;

fn insert_bounded_handle<K: Eq + std::hash::Hash>(
    map: &mut HashMap<K, iced_image::Handle>,
    key: K,
    handle: iced_image::Handle,
) {
    if !map.contains_key(&key) && map.len() >= MAX_AVATAR_CACHE_ENTRIES {
        map.clear();
    }
    map.insert(key, handle);
}

pub struct AvatarCache {
    handles: Mutex<HashMap<ClientUuid, iced_image::Handle>>,
    room_handles: Mutex<HashMap<RoomId, iced_image::Handle>>,
}

impl AvatarCache {
    pub fn new() -> Self {
        prune_expired_avatars();
        Self {
            handles: Mutex::new(HashMap::new()),
            room_handles: Mutex::new(HashMap::new()),
        }
    }

    pub fn handle_for(&self, client_uuid: &ClientUuid) -> iced_image::Handle {
        if let Ok(map) = self.handles.lock() {
            if let Some(existing) = map.get(client_uuid) {
                return existing.clone();
            }
        }
        let rgba = generate_identicon_rgba(client_uuid);
        let handle = iced_image::Handle::from_rgba(AVATAR_SIZE, AVATAR_SIZE, rgba);
        if let Ok(mut map) = self.handles.lock() {
            insert_bounded_handle(&mut map, *client_uuid, handle.clone());
        }
        handle
    }

    pub fn handle_for_pub_key(&self, pub_key: &[u8; 32]) -> iced_image::Handle {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"squad-air-friend-pubkey-v1");
        hasher.update(pub_key);
        let digest = hasher.finalize();
        let mut key = [0u8; 16];
        key.copy_from_slice(&digest.as_bytes()[..16]);
        self.handle_for(&key)
    }

    pub fn room_handle_for(&self, room_id: &RoomId) -> iced_image::Handle {
        if let Ok(map) = self.room_handles.lock() {
            if let Some(existing) = map.get(room_id) {
                return existing.clone();
            }
        }
        let rgba = generate_room_avatar_rgba(room_id);
        let handle = iced_image::Handle::from_rgba(AVATAR_SIZE, AVATAR_SIZE, rgba);
        if let Ok(mut map) = self.room_handles.lock() {
            insert_bounded_handle(&mut map, *room_id, handle.clone());
        }
        handle
    }

    pub fn set_custom_rgba(&self, client_uuid: ClientUuid, rgba: Vec<u8>) {
        let handle = iced_image::Handle::from_rgba(AVATAR_SIZE, AVATAR_SIZE, rgba);
        if let Ok(mut map) = self.handles.lock() {
            insert_bounded_handle(&mut map, client_uuid, handle);
        }
    }

    pub fn set_from_jpeg_option(&self, client_uuid: ClientUuid, jpeg_bytes: Option<&[u8]>) {
        if let Some(jpeg) = jpeg_bytes {
            if let Ok(rgba) = decode_avatar_jpeg_to_circular_rgba(jpeg) {
                self.set_custom_rgba(client_uuid, rgba);
                return;
            }
        }
        self.reset_to_identicon(client_uuid);
    }

    pub fn reset_to_identicon(&self, client_uuid: ClientUuid) {
        let rgba = generate_identicon_rgba(&client_uuid);
        let handle = iced_image::Handle::from_rgba(AVATAR_SIZE, AVATAR_SIZE, rgba);
        if let Ok(mut map) = self.handles.lock() {
            insert_bounded_handle(&mut map, client_uuid, handle);
        }
    }

    pub fn clear_memory_cache(&self) {
        if let Ok(mut map) = self.handles.lock() {
            map.clear();
            map.shrink_to_fit();
        }
        if let Ok(mut map) = self.room_handles.lock() {
            map.clear();
            map.shrink_to_fit();
        }
    }
}

impl Default for AvatarCache {
    fn default() -> Self {
        Self::new()
    }
}
