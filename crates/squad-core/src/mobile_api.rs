use crate::android_video::spawn_android_surface_render_loop;
use crate::avatar_core::{
    decode_avatar_jpeg_to_circular_rgba, generate_friend_identicon_rgba, generate_identicon_rgba,
};
use crate::config::AppConfig;
use crate::crypto_identity::{
    format_friend_code, load_friends, parse_friend_code, save_friends, FriendContact,
};
use crate::net::lan::DiscoveredLanRoom;
use crate::net::nat::set_platform_lan_ipv4_override;
use crate::net::{
    spawn_mesh_worker, BlobTransferState, ChatContentUi, ChatItemUi, DecodedVideoFrame,
    FriendPresenceUi, MeshCommand, MeshEvent, PeerConnStatus, PeerRouteKind, PeerUiSnapshot,
    RemoteStreamInfo,
};
use crate::storage_paths::{init_storage_paths, StoragePaths};
use squad_dsp::{
    AudioEngineConfig, AudioEngineHandle, MicActivationMode, NoiseSuppressionMode, SoundCue,
};
use squad_proto::{sanitize_display_str, ClientUuid, InviteLink, PeerId, MAX_NICKNAME_LEN};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, Notify};

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum MobileCoreError {
    #[error("{message}")]
    InvalidArgument { message: String },
    #[error("{message}")]
    AudioInitFailed { message: String },
    #[error("{message}")]
    Internal { message: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileProfileSnapshot {
    pub nickname: String,
    pub client_uuid_hex: String,
    pub my_friend_code: String,
    pub rendezvous_server: String,
    pub custom_rendezvous_server: String,
    pub mic_muted: bool,
    pub deafened: bool,
    pub aec_enabled: bool,
    pub ns_mode: String,
    pub vad_threshold_db: f32,
    pub input_gain: f32,
    pub stream_volume: f32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileFriendItem {
    pub pub_key_hex: String,
    pub nickname: String,
    pub online: bool,
    pub in_room: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileSavedChannel {
    pub name: String,
    pub invite_uri: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileAudioTelemetry {
    pub local_mic_level_db: f32,
    pub is_local_speaking: bool,
    pub jitter_target_ms: u32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileRemoteStreamInfo {
    pub codec: String,
    pub has_audio: bool,
    pub width: u16,
    pub height: u16,
    pub fps: u8,
    pub source_title: String,
    pub encoder_backend: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobilePeerSnapshot {
    pub peer_id: u16,
    pub client_uuid_hex: String,
    pub pub_key_hex: Option<String>,
    pub nickname: String,
    pub status: String,
    pub route: String,
    pub rtt_ms: u32,
    pub jitter_ms: u32,
    pub loss_pct: f32,
    pub is_speaking: bool,
    pub volume_gain: f32,
    pub stream_info: Option<MobileRemoteStreamInfo>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileLanRoomItem {
    pub room_id_hex: String,
    pub room_name: String,
    pub host_nickname: String,
    pub peer_count: u8,
    pub max_peers: u8,
    pub requires_password: bool,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum MobileBlobTransferState {
    Available,
    Downloading {
        received_chunks: u32,
        total_chunks: u32,
    },
    Completed {
        saved_path: Option<String>,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum MobileChatContent {
    Text {
        text: String,
    },
    InlineImage {
        blob_id_hex: String,
        filename: String,
        total_size: u64,
        bytes: Option<Vec<u8>>,
        state: MobileBlobTransferState,
    },
    File {
        blob_id_hex: String,
        filename: String,
        total_size: u64,
        state: MobileBlobTransferState,
    },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileChatItem {
    pub msg_id: u64,
    pub sender_uuid_hex: String,
    pub sender_nickname: String,
    pub timestamp_ms: u64,
    pub is_self: bool,
    pub content: MobileChatContent,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MobileIncomingCall {
    pub sender_pub_key_hex: String,
    pub sender_nickname: String,
    pub invite_uri: String,
    pub is_party_invite: bool,
    pub is_known_friend: bool,
}

#[uniffi::export(callback_interface)]
pub trait MobileEventListener: Send + Sync {
    fn on_joined_room(&self, local_peer_id: u16, invite_uri: String, is_lan_locked: bool);
    fn on_lan_auth_failed(&self, message: String);
    fn on_left_room(&self);
    fn on_peers_updated(&self, peers: Vec<MobilePeerSnapshot>);
    fn on_chat_item_received(&self, item: MobileChatItem);
    fn on_blob_state_changed(
        &self,
        blob_id_hex: String,
        state: MobileBlobTransferState,
        inline_bytes: Option<Vec<u8>>,
    );
    fn on_peer_avatar_loaded(
        &self,
        client_uuid_hex: String,
        avatar_hash_hex: String,
        rgba_128: Vec<u8>,
    );
    fn on_remote_stream_updated(&self, peer_id: u16, stream: Option<MobileRemoteStreamInfo>);
    fn on_lan_rooms_updated(&self, rooms: Vec<MobileLanRoomItem>);
    fn on_friends_status_updated(&self, friends: Vec<MobileFriendItem>);
    fn on_incoming_friend_call(&self, call: MobileIncomingCall);
    fn on_short_code_generated(&self, code: String, expires_in_secs: u16);
    fn on_short_code_cleared(&self);
    fn on_friend_invite_sent(&self, pub_key_hex: String);
    fn on_error(&self, message: String);
}

struct SharedState {
    config: AppConfig,
    friends: Vec<FriendContact>,
    online_friends: HashMap<[u8; 32], FriendPresenceUi>,
    discovered_lan_rooms: HashMap<[u8; 16], DiscoveredLanRoom>,
    peer_id_by_uuid: HashMap<ClientUuid, PeerId>,
    in_room: bool,
    active_invite_uri: Option<String>,
    pending_incoming_call_invite: Option<String>,
}

#[derive(uniffi::Object)]
pub struct MobileCoreController {
    runtime: Runtime,
    audio_engine: AudioEngineHandle,
    mesh_tx: mpsc::Sender<MeshCommand>,
    video_notify: Arc<Notify>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<SharedState>>,
}

#[uniffi::export]
impl MobileCoreController {
    #[uniffi::constructor]
    pub fn new(
        files_dir: String,
        cache_dir: String,
        downloads_dir: String,
        event_listener: Box<dyn MobileEventListener>,
    ) -> Result<Arc<Self>, MobileCoreError> {
        if files_dir.trim().is_empty()
            || cache_dir.trim().is_empty()
            || downloads_dir.trim().is_empty()
        {
            return Err(MobileCoreError::InvalidArgument {
                message: "Storage directory paths must be non-empty".to_string(),
            });
        }

        init_storage_paths(StoragePaths {
            config_dir: PathBuf::from(files_dir),
            cache_dir: PathBuf::from(cache_dir),
            downloads_dir: PathBuf::from(downloads_dir),
        });

        let config = AppConfig::load_or_create();
        let friends = load_friends();

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .map_err(|e| MobileCoreError::Internal {
                message: format!("Failed to create Tokio runtime: {e}"),
            })?;

        let (audio_out_tx, audio_out_rx) = mpsc::channel(64);
        let audio_cfg = AudioEngineConfig {
            input_device_name: None,
            output_device_name: None,
            aec_enabled: config.aec_enabled,
            ns_mode: config.ns_mode,
            activation_mode: MicActivationMode::ToggleMute,
            input_gain: config.input_gain,
            cue_gain: config.cue_volume,
            vad_threshold_db: config.vad_threshold_db,
            bitrate_bps: config.opus_bitrate_bps,
            jitter_profile: config.jitter_profile,
        };

        let _guard = runtime.enter();
        let audio_engine = AudioEngineHandle::start(audio_cfg, audio_out_tx).map_err(|e| {
            MobileCoreError::AudioInitFailed {
                message: format!("Failed to start audio engine: {e}"),
            }
        })?;
        audio_engine.set_stream_gain(config.stream_volume);

        let video_frame_slot: Arc<Mutex<Option<DecodedVideoFrame>>> = Arc::new(Mutex::new(None));
        let mesh_notify = Arc::new(Notify::new());
        let video_notify = Arc::new(Notify::new());
        let running = Arc::new(AtomicBool::new(true));

        let (mesh_tx, mut evt_rx) = spawn_mesh_worker(
            audio_engine.clone(),
            audio_out_rx,
            Arc::clone(&video_frame_slot),
            Arc::clone(&mesh_notify),
            Arc::clone(&video_notify),
        );

        spawn_android_surface_render_loop(
            runtime.handle().clone(),
            video_frame_slot,
            Arc::clone(&video_notify),
            Arc::clone(&running),
        );

        let init_presence_cmd = MeshCommand::UpdatePresenceConfig {
            rendezvous_server: config.rendezvous_server.clone(),
            nickname: config.nickname.clone(),
            client_uuid: config.client_uuid(),
            identity_secret: config.identity_secret().to_bytes(),
            friend_pub_keys: friends.iter().map(|f| f.pub_key).collect(),
        };
        let _ = mesh_tx.try_send(init_presence_cmd);

        let state = Arc::new(Mutex::new(SharedState {
            config,
            friends,
            online_friends: HashMap::new(),
            discovered_lan_rooms: HashMap::new(),
            peer_id_by_uuid: HashMap::new(),
            in_room: false,
            active_invite_uri: None,
            pending_incoming_call_invite: None,
        }));

        let state_evt = Arc::clone(&state);
        let audio_evt = audio_engine.clone();
        let listener: Arc<dyn MobileEventListener> = Arc::from(event_listener);
        runtime.spawn(async move {
            while let Some(ev) = evt_rx.recv().await {
                dispatch_mesh_event(ev, &state_evt, &audio_evt, &listener);
            }
        });

        drop(_guard);

        Ok(Arc::new(Self {
            runtime,
            audio_engine,
            mesh_tx,
            video_notify,
            running,
            state,
        }))
    }

    pub fn get_profile_snapshot(&self) -> MobileProfileSnapshot {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = &guard.config;
        let pub_key = cfg.identity_pub_key();
        MobileProfileSnapshot {
            nickname: cfg.nickname.clone(),
            client_uuid_hex: bytes_to_hex(&cfg.client_uuid),
            my_friend_code: format_friend_code(&pub_key, &cfg.nickname),
            rendezvous_server: cfg.rendezvous_server.clone(),
            custom_rendezvous_server: cfg.custom_rendezvous_server.clone(),
            mic_muted: self.audio_engine.is_mic_muted(),
            deafened: self.audio_engine.is_deafened(),
            aec_enabled: cfg.aec_enabled,
            ns_mode: ns_mode_label(cfg.ns_mode).to_string(),
            vad_threshold_db: cfg.vad_threshold_db,
            input_gain: cfg.input_gain,
            stream_volume: cfg.stream_volume,
        }
    }

    pub fn set_nickname(&self, nickname: String) -> Result<(), MobileCoreError> {
        let clean = sanitize_display_str(nickname.trim(), MAX_NICKNAME_LEN);
        if clean.is_empty() {
            return Err(MobileCoreError::InvalidArgument {
                message: "Nickname cannot be empty".to_string(),
            });
        }
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.nickname = clean;
        guard.config.save().map_err(|e| MobileCoreError::Internal {
            message: format!("Failed to save config: {e}"),
        })?;
        self.send_presence_config_locked(&guard);
        Ok(())
    }

    pub fn set_rendezvous_server(&self, server: String) -> Result<(), MobileCoreError> {
        let trimmed = server.trim().to_string();
        if trimmed.is_empty() {
            return Err(MobileCoreError::InvalidArgument {
                message: "Rendezvous server cannot be empty".to_string(),
            });
        }
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.rendezvous_server = trimmed.clone();
        if crate::config::matching_rendezvous_preset(&trimmed).is_none() {
            guard.config.custom_rendezvous_server = trimmed;
        }
        guard.config.save().map_err(|e| MobileCoreError::Internal {
            message: format!("Failed to save config: {e}"),
        })?;
        self.send_presence_config_locked(&guard);
        Ok(())
    }

    pub fn get_friends(&self) -> Vec<MobileFriendItem> {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        build_friend_items(&guard.friends, &guard.online_friends)
    }

    pub fn add_friend_by_code(
        &self,
        friend_code: String,
    ) -> Result<MobileFriendItem, MobileCoreError> {
        let (pub_key, maybe_nick) = parse_friend_code(&friend_code)
            .map_err(|message| MobileCoreError::InvalidArgument { message })?;

        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if pub_key == guard.config.identity_pub_key() {
            return Err(MobileCoreError::InvalidArgument {
                message: "Cannot add yourself as a friend".to_string(),
            });
        }

        let nickname = maybe_nick.unwrap_or_else(|| "Friend".to_string());
        if let Some(existing) = guard.friends.iter_mut().find(|f| f.pub_key == pub_key) {
            if existing.nickname == "Friend" && nickname != "Friend" {
                existing.nickname = nickname;
            }
        } else {
            guard.friends.push(FriendContact {
                pub_key,
                nickname,
                added_at_ms: now_millis(),
            });
        }

        save_friends(&guard.friends);
        self.send_presence_config_locked(&guard);

        let items = build_friend_items(&guard.friends, &guard.online_friends);
        let pub_hex = bytes_to_hex(&pub_key);
        Ok(items
            .into_iter()
            .find(|item| item.pub_key_hex == pub_hex)
            .unwrap_or(MobileFriendItem {
                pub_key_hex: pub_hex,
                nickname: "Friend".to_string(),
                online: false,
                in_room: false,
            }))
    }

    pub fn remove_friend(&self, pub_key_hex: String) -> Result<(), MobileCoreError> {
        let pub_key =
            decode_hex_32(&pub_key_hex).ok_or_else(|| MobileCoreError::InvalidArgument {
                message: "Invalid friend public key hex".to_string(),
            })?;
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let before = guard.friends.len();
        guard.friends.retain(|f| f.pub_key != pub_key);
        if guard.friends.len() != before {
            guard.online_friends.remove(&pub_key);
            save_friends(&guard.friends);
            self.send_presence_config_locked(&guard);
        }
        Ok(())
    }

    pub fn call_or_invite_friend(&self, pub_key_hex: String) -> Result<(), MobileCoreError> {
        let target_pub_key =
            decode_hex_32(&pub_key_hex).ok_or_else(|| MobileCoreError::InvalidArgument {
                message: "Invalid friend public key hex".to_string(),
            })?;
        self.send_mesh(MeshCommand::CallOrInviteFriend { target_pub_key });
        Ok(())
    }

    pub fn accept_incoming_call(&self, invite_uri: String) -> Result<(), MobileCoreError> {
        {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.pending_incoming_call_invite = None;
        }
        self.join_room_by_invite(invite_uri)
    }

    pub fn decline_incoming_call(&self) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.pending_incoming_call_invite = None;
    }

    pub fn create_and_join_room(&self) -> Result<(), MobileCoreError> {
        let invite = InviteLink::generate();
        let (rendezvous_server, nickname, client_uuid, initial_volumes) = {
            let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                guard.config.rendezvous_server.clone(),
                guard.config.nickname.clone(),
                guard.config.client_uuid(),
                collect_initial_volumes(&guard.config),
            )
        };
        self.audio_engine.set_streams_active(true);
        self.send_mesh(MeshCommand::JoinRoom {
            rendezvous_server,
            invite,
            nickname,
            client_uuid,
            initial_volumes,
            is_lan_room: false,
            room_name: None,
        });
        Ok(())
    }

    pub fn join_room_by_invite(&self, invite_uri: String) -> Result<(), MobileCoreError> {
        let invite =
            InviteLink::parse(&invite_uri).map_err(|e| MobileCoreError::InvalidArgument {
                message: format!("Invalid invite link: {e}"),
            })?;
        let (rendezvous_server, nickname, client_uuid, initial_volumes) = {
            let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                guard.config.rendezvous_server.clone(),
                guard.config.nickname.clone(),
                guard.config.client_uuid(),
                collect_initial_volumes(&guard.config),
            )
        };
        self.audio_engine.set_streams_active(true);
        self.send_mesh(MeshCommand::JoinRoom {
            rendezvous_server,
            invite,
            nickname,
            client_uuid,
            initial_volumes,
            is_lan_room: false,
            room_name: None,
        });
        Ok(())
    }

    pub fn join_room_by_short_code(&self, code_6_digits: String) -> Result<(), MobileCoreError> {
        let code = parse_six_digit_code(&code_6_digits)?;
        let (rendezvous_server, nickname, client_uuid) = {
            let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                guard.config.rendezvous_server.clone(),
                guard.config.nickname.clone(),
                guard.config.client_uuid(),
            )
        };
        self.audio_engine.set_streams_active(true);
        self.send_mesh(MeshCommand::JoinByShortCode {
            rendezvous_server,
            code,
            nickname,
            client_uuid,
        });
        Ok(())
    }

    pub fn generate_short_code(&self) {
        self.send_mesh(MeshCommand::GenerateShortCode);
    }

    pub fn join_lan_room(
        &self,
        room_id_hex: String,
        password: Option<String>,
    ) -> Result<(), MobileCoreError> {
        let room_id =
            decode_hex_16(&room_id_hex).ok_or_else(|| MobileCoreError::InvalidArgument {
                message: "Invalid LAN room identifier".to_string(),
            })?;
        let (room, nickname, client_uuid, initial_volumes) = {
            let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let room = guard
                .discovered_lan_rooms
                .get(&room_id)
                .cloned()
                .ok_or_else(|| MobileCoreError::InvalidArgument {
                    message: "LAN room is no longer available".to_string(),
                })?;
            (
                room,
                guard.config.nickname.clone(),
                guard.config.client_uuid(),
                collect_initial_volumes(&guard.config),
            )
        };

        if let Some(secret) = room.secret {
            let invite = InviteLink {
                room_id: room.room_id,
                secret,
            };
            self.audio_engine.set_streams_active(true);
            self.send_mesh(MeshCommand::JoinRoom {
                rendezvous_server: room.host_addr.to_string(),
                invite,
                nickname,
                client_uuid,
                initial_volumes,
                is_lan_room: true,
                room_name: Some(room.room_name),
            });
            return Ok(());
        }

        let clean_pw = password
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .ok_or_else(|| MobileCoreError::InvalidArgument {
                message: "Password is required for locked LAN room".to_string(),
            })?;

        self.audio_engine.set_streams_active(true);
        self.send_mesh(MeshCommand::JoinLockedLanRoom {
            room,
            password: clean_pw,
            nickname,
            client_uuid,
        });
        Ok(())
    }

    pub fn leave_room(&self) {
        self.audio_engine.set_streams_active(false);
        self.audio_engine.clear_peers();
        self.send_mesh(MeshCommand::LeaveRoom);
    }

    pub fn get_saved_channels(&self) -> Vec<MobileSavedChannel> {
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .config
            .saved_channels
            .iter()
            .map(|c| MobileSavedChannel {
                name: c.name.clone(),
                invite_uri: c.invite_uri.clone(),
            })
            .collect()
    }

    pub fn save_channel(&self, name: String, invite_uri: String) -> Result<(), MobileCoreError> {
        InviteLink::parse(&invite_uri).map_err(|e| MobileCoreError::InvalidArgument {
            message: format!("Invalid invite link: {e}"),
        })?;
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.add_saved_channel(name, invite_uri);
        Ok(())
    }

    pub fn remove_saved_channel(&self, invite_uri: String) {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.remove_saved_channel(&invite_uri);
    }

    pub fn set_mic_muted(&self, muted: bool) {
        let prev = self.audio_engine.is_mic_muted();
        self.audio_engine.set_mic_muted(muted);
        if prev != muted {
            self.audio_engine.play_cue(if muted {
                SoundCue::Mute
            } else {
                SoundCue::Unmute
            });
        }
    }

    pub fn toggle_mic_mute(&self) -> bool {
        let next = !self.audio_engine.is_mic_muted();
        self.audio_engine.set_mic_muted(next);
        self.audio_engine.play_cue(if next {
            SoundCue::Mute
        } else {
            SoundCue::Unmute
        });
        next
    }

    pub fn set_deafened(&self, deafened: bool) {
        let prev = self.audio_engine.is_deafened();
        self.audio_engine.set_deafened(deafened);
        if prev != deafened {
            self.audio_engine.play_cue(if deafened {
                SoundCue::Mute
            } else {
                SoundCue::Unmute
            });
        }
    }

    pub fn toggle_deafen(&self) -> bool {
        let next = !self.audio_engine.is_deafened();
        self.audio_engine.set_deafened(next);
        self.audio_engine.play_cue(if next {
            SoundCue::Mute
        } else {
            SoundCue::Unmute
        });
        next
    }

    pub fn set_aec_enabled(&self, enabled: bool) {
        self.audio_engine.set_aec_enabled(enabled);
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.aec_enabled = enabled;
        let _ = guard.config.save();
    }

    pub fn set_rnnoise_enabled(&self, enabled: bool) {
        let mode = if enabled {
            NoiseSuppressionMode::Rnnoise
        } else {
            NoiseSuppressionMode::Off
        };
        self.audio_engine.set_ns_mode(mode);
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.ns_mode = mode;
        let _ = guard.config.save();
    }

    pub fn set_input_gain(&self, gain: f32) {
        let clamped = if gain.is_finite() {
            gain.clamp(0.0, 2.5)
        } else {
            1.0
        };
        self.audio_engine.set_input_gain(clamped);
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.input_gain = clamped;
        let _ = guard.config.save();
    }

    pub fn set_vad_threshold_db(&self, db: f32) {
        let clamped = if db.is_finite() {
            db.clamp(-60.0, 0.0)
        } else {
            -45.0
        };
        self.audio_engine.set_vad_threshold_db(clamped);
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.vad_threshold_db = clamped;
        let _ = guard.config.save();
    }

    pub fn set_peer_volume(
        &self,
        client_uuid_hex: String,
        gain: f32,
    ) -> Result<(), MobileCoreError> {
        let uuid =
            decode_hex_16(&client_uuid_hex).ok_or_else(|| MobileCoreError::InvalidArgument {
                message: "Invalid peer client UUID".to_string(),
            })?;
        let clamped = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            1.0
        };
        let peer_id_opt = {
            let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
            guard.config.set_peer_volume(&uuid, clamped);
            guard.peer_id_by_uuid.get(&uuid).copied()
        };
        if let Some(peer_id) = peer_id_opt {
            self.audio_engine.set_peer_gain(peer_id, clamped);
        }
        Ok(())
    }

    pub fn set_stream_volume(&self, gain: f32) {
        let clamped = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            1.0
        };
        self.audio_engine.set_stream_gain(clamped);
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        guard.config.stream_volume = clamped;
        let _ = guard.config.save();
    }

    pub fn notify_android_audio_route_changed(&self) {
        self.audio_engine.set_audio_devices(None, None);
    }

    pub fn set_lan_ipv4_override(&self, ipv4_list: Vec<String>) {
        set_platform_lan_ipv4_override(parse_ipv4_list(&ipv4_list));
    }

    pub fn poll_fast_audio_telemetry(&self) -> MobileAudioTelemetry {
        MobileAudioTelemetry {
            local_mic_level_db: self.audio_engine.local_mic_level_db(),
            is_local_speaking: self.audio_engine.is_local_speaking(),
            jitter_target_ms: self.audio_engine.jitter_target_ms(),
        }
    }

    pub fn watch_peer_stream(&self, peer_id: Option<u16>) {
        self.send_mesh(MeshCommand::WatchPeerStream { peer_id });
    }

    pub fn send_chat_text(&self, text: String) {
        self.send_mesh(MeshCommand::SendChatText { text });
    }

    pub fn send_inline_image(&self, filename: String, bytes: Vec<u8>) {
        self.send_mesh(MeshCommand::SendInlineImage { filename, bytes });
    }

    pub fn offer_file(&self, file_path: String) {
        self.send_mesh(MeshCommand::OfferFile {
            path: PathBuf::from(file_path),
        });
    }

    pub fn download_blob(&self, blob_id_hex: String) -> Result<(), MobileCoreError> {
        let blob_id =
            decode_hex_16(&blob_id_hex).ok_or_else(|| MobileCoreError::InvalidArgument {
                message: "Invalid blob identifier".to_string(),
            })?;
        self.send_mesh(MeshCommand::DownloadBlob { blob_id });
        Ok(())
    }

    pub fn get_identicon_rgba(&self, client_uuid_hex: String) -> Vec<u8> {
        let uuid = decode_hex_16(&client_uuid_hex).unwrap_or([0u8; 16]);
        generate_identicon_rgba(&uuid)
    }

    pub fn get_friend_identicon_rgba(&self, pub_key_hex: String) -> Vec<u8> {
        let key = decode_hex_32(&pub_key_hex).unwrap_or([0u8; 32]);
        generate_friend_identicon_rgba(&key)
    }

    pub fn shutdown(&self) {
        self.running.store(false, Ordering::Release);
        self.video_notify.notify_waiters();
        let _ = self.mesh_tx.try_send(MeshCommand::Shutdown);
        self.audio_engine.shutdown();
    }
}

impl MobileCoreController {
    fn send_mesh(&self, cmd: MeshCommand) {
        if self.mesh_tx.try_send(cmd.clone()).is_err() {
            let tx = self.mesh_tx.clone();
            self.runtime.spawn(async move {
                let _ = tx.send(cmd).await;
            });
        }
    }

    fn send_presence_config_locked(&self, guard: &SharedState) {
        let cmd = MeshCommand::UpdatePresenceConfig {
            rendezvous_server: guard.config.rendezvous_server.clone(),
            nickname: guard.config.nickname.clone(),
            client_uuid: guard.config.client_uuid(),
            identity_secret: guard.config.identity_secret().to_bytes(),
            friend_pub_keys: guard.friends.iter().map(|f| f.pub_key).collect(),
        };
        self.send_mesh(cmd);
    }
}

impl Drop for MobileCoreController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn dispatch_mesh_event(
    ev: MeshEvent,
    state: &Arc<Mutex<SharedState>>,
    audio_engine: &AudioEngineHandle,
    listener: &Arc<dyn MobileEventListener>,
) {
    match ev {
        MeshEvent::JoinedRoom {
            local_peer_id,
            invite_uri,
            is_lan_locked,
        } => {
            let first_join = {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                let was_in_room = guard.in_room;
                guard.in_room = true;
                guard.active_invite_uri = Some(invite_uri.clone());
                !was_in_room
            };
            if first_join {
                audio_engine.play_cue(SoundCue::Join);
            }
            listener.on_joined_room(local_peer_id, invite_uri, is_lan_locked);
        }
        MeshEvent::LanAuthFailed(msg) => {
            let in_room = state.lock().unwrap_or_else(|e| e.into_inner()).in_room;
            if !in_room {
                audio_engine.set_streams_active(false);
            }
            listener.on_lan_auth_failed(msg);
        }
        MeshEvent::LeftRoom => {
            {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard.in_room = false;
                guard.active_invite_uri = None;
                guard.peer_id_by_uuid.clear();
            }
            audio_engine.set_streams_active(false);
            listener.on_left_room();
        }
        MeshEvent::PeersUpdated(snaps) => {
            let mapped = {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard.peer_id_by_uuid.clear();
                for snap in &snaps {
                    guard.peer_id_by_uuid.insert(snap.client_uuid, snap.peer_id);
                    let saved = guard.config.peer_volume(&snap.client_uuid);
                    audio_engine.set_peer_gain(snap.peer_id, saved);
                }
                snaps
                    .into_iter()
                    .map(|snap| {
                        let gain = guard.config.peer_volume(&snap.client_uuid);
                        convert_peer_snapshot(snap, gain)
                    })
                    .collect()
            };
            listener.on_peers_updated(mapped);
        }
        MeshEvent::ChatItemReceived(item) => {
            listener.on_chat_item_received(convert_chat_item(item));
        }
        MeshEvent::BlobStateChanged {
            blob_id,
            state,
            inline_bytes,
        } => {
            listener.on_blob_state_changed(
                bytes_to_hex(&blob_id),
                convert_blob_state(state),
                inline_bytes,
            );
        }
        MeshEvent::PeerAvatarLoaded {
            client_uuid,
            avatar_hash,
            jpeg_bytes,
        } => {
            let rgba = if avatar_hash == [0u8; 32] {
                generate_identicon_rgba(&client_uuid)
            } else if let Some(raw) = jpeg_bytes.as_deref() {
                decode_avatar_jpeg_to_circular_rgba(raw)
                    .unwrap_or_else(|_| generate_identicon_rgba(&client_uuid))
            } else {
                generate_identicon_rgba(&client_uuid)
            };
            listener.on_peer_avatar_loaded(
                bytes_to_hex(&client_uuid),
                bytes_to_hex(&avatar_hash),
                rgba,
            );
        }
        MeshEvent::RemoteStreamUpdated { peer_id, stream } => {
            listener.on_remote_stream_updated(peer_id, stream.map(convert_stream_info));
        }
        MeshEvent::LocalStreamStatus { .. } | MeshEvent::StreamCodecFallbackToH264 { .. } => {}
        MeshEvent::LanRoomsUpdated(rooms) => {
            let mapped = {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard.discovered_lan_rooms.clear();
                let mut out = Vec::with_capacity(rooms.len());
                for room in rooms {
                    out.push(MobileLanRoomItem {
                        room_id_hex: bytes_to_hex(&room.room_id),
                        room_name: room.room_name.clone(),
                        host_nickname: room.host_nickname.clone(),
                        peer_count: room.peer_count,
                        max_peers: room.max_peers,
                        requires_password: room.requires_password(),
                    });
                    guard.discovered_lan_rooms.insert(room.room_id, room);
                }
                out
            };
            listener.on_lan_rooms_updated(mapped);
        }
        MeshEvent::FriendsStatusUpdated(online_list) => {
            let merged = {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                guard.online_friends = online_list
                    .into_iter()
                    .filter(|f| f.online)
                    .map(|f| (f.pub_key, f))
                    .collect();
                build_friend_items(&guard.friends, &guard.online_friends)
            };
            listener.on_friends_status_updated(merged);
        }
        MeshEvent::IncomingFriendCall(call) => {
            let invite_uri = call.invite.to_uri();
            let should_deliver = {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                let same_room = guard.in_room
                    && guard
                        .active_invite_uri
                        .as_deref()
                        .and_then(|u| InviteLink::parse(u).ok())
                        .is_some_and(|inv| inv.room_id == call.invite.room_id);
                if !same_room {
                    guard.pending_incoming_call_invite = Some(invite_uri.clone());
                    true
                } else {
                    false
                }
            };
            if should_deliver {
                audio_engine.play_cue(SoundCue::IncomingCall);
                listener.on_incoming_friend_call(MobileIncomingCall {
                    sender_pub_key_hex: bytes_to_hex(&call.sender_pub_key),
                    sender_nickname: call.sender_nickname,
                    invite_uri,
                    is_party_invite: call.is_party_invite,
                    is_known_friend: call.is_known_friend,
                });
            }
        }
        MeshEvent::ShortCodeGenerated {
            code,
            expires_in_secs,
        } => {
            listener.on_short_code_generated(format!("{code:06}"), expires_in_secs);
        }
        MeshEvent::ShortCodeCleared => {
            listener.on_short_code_cleared();
        }
        MeshEvent::FriendInviteSent { target_pub_key } => {
            listener.on_friend_invite_sent(bytes_to_hex(&target_pub_key));
        }
        MeshEvent::Error(msg) => {
            let in_room = state.lock().unwrap_or_else(|e| e.into_inner()).in_room;
            if !in_room {
                audio_engine.set_streams_active(false);
            }
            listener.on_error(msg);
        }
    }
}

fn collect_initial_volumes(cfg: &AppConfig) -> HashMap<ClientUuid, f32> {
    let mut out = HashMap::new();
    for (k, &v) in &cfg.peer_volumes {
        if let Ok(parsed) = uuid::Uuid::parse_str(k) {
            out.insert(*parsed.as_bytes(), v.clamp(0.0, 2.0));
        }
    }
    out
}

fn build_friend_items(
    friends: &[FriendContact],
    online: &HashMap<[u8; 32], FriendPresenceUi>,
) -> Vec<MobileFriendItem> {
    let mut items: Vec<MobileFriendItem> = friends
        .iter()
        .map(|f| {
            let presence = online.get(&f.pub_key);
            MobileFriendItem {
                pub_key_hex: bytes_to_hex(&f.pub_key),
                nickname: presence
                    .map(|p| p.nickname.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| f.nickname.clone()),
                online: presence.is_some_and(|p| p.online),
                in_room: presence.is_some_and(|p| p.in_room),
            }
        })
        .collect();
    items.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| b.in_room.cmp(&a.in_room))
            .then_with(|| a.nickname.to_lowercase().cmp(&b.nickname.to_lowercase()))
    });
    items
}

fn convert_stream_info(s: RemoteStreamInfo) -> MobileRemoteStreamInfo {
    MobileRemoteStreamInfo {
        codec: s.codec.label().to_string(),
        has_audio: s.has_audio,
        width: s.width,
        height: s.height,
        fps: s.fps,
        source_title: s.source_title,
        encoder_backend: s.encoder_backend,
    }
}

fn convert_peer_snapshot(snap: PeerUiSnapshot, volume_gain: f32) -> MobilePeerSnapshot {
    MobilePeerSnapshot {
        peer_id: snap.peer_id,
        client_uuid_hex: bytes_to_hex(&snap.client_uuid),
        pub_key_hex: snap.pub_key.map(|k| bytes_to_hex(&k)),
        nickname: snap.nickname,
        status: match snap.status {
            PeerConnStatus::Punching => "Punching".to_string(),
            PeerConnStatus::Connected => "Connected".to_string(),
            PeerConnStatus::MeshRelayed => "MeshRelayed".to_string(),
            PeerConnStatus::Relayed => "Relayed".to_string(),
            PeerConnStatus::NatFailed => "NatFailed".to_string(),
        },
        route: match snap.route {
            PeerRouteKind::Connecting => "CONNECTING".to_string(),
            PeerRouteKind::LanDirect => "LAN".to_string(),
            PeerRouteKind::P2pDirect => "P2P".to_string(),
            PeerRouteKind::MeshRelayed => "MESH".to_string(),
            PeerRouteKind::Relayed => "RELAY".to_string(),
            PeerRouteKind::NatFailed => "NAT_FAILED".to_string(),
        },
        rtt_ms: snap.rtt_ms,
        jitter_ms: snap.jitter_ms,
        loss_pct: snap.loss_pct,
        is_speaking: snap.is_speaking,
        volume_gain,
        stream_info: snap.stream_info.map(convert_stream_info),
    }
}

fn convert_blob_state(state: BlobTransferState) -> MobileBlobTransferState {
    match state {
        BlobTransferState::Available => MobileBlobTransferState::Available,
        BlobTransferState::Downloading {
            received_chunks,
            total_chunks,
        } => MobileBlobTransferState::Downloading {
            received_chunks,
            total_chunks,
        },
        BlobTransferState::Completed { saved_path } => MobileBlobTransferState::Completed {
            saved_path: saved_path.map(|p| p.to_string_lossy().into_owned()),
        },
        BlobTransferState::Failed(reason) => MobileBlobTransferState::Failed { reason },
    }
}

fn convert_chat_item(item: ChatItemUi) -> MobileChatItem {
    let content = match item.content {
        ChatContentUi::Text(text) => MobileChatContent::Text { text },
        ChatContentUi::InlineImage {
            blob_id,
            filename,
            total_size,
            bytes,
            state,
        } => MobileChatContent::InlineImage {
            blob_id_hex: bytes_to_hex(&blob_id),
            filename,
            total_size,
            bytes,
            state: convert_blob_state(state),
        },
        ChatContentUi::File {
            blob_id,
            filename,
            total_size,
            state,
        } => MobileChatContent::File {
            blob_id_hex: bytes_to_hex(&blob_id),
            filename,
            total_size,
            state: convert_blob_state(state),
        },
    };
    MobileChatItem {
        msg_id: item.msg_id,
        sender_uuid_hex: bytes_to_hex(&item.sender_uuid),
        sender_nickname: item.sender_nickname,
        timestamp_ms: item.timestamp_ms,
        is_self: item.is_self,
        content,
    }
}

fn ns_mode_label(mode: NoiseSuppressionMode) -> &'static str {
    match mode {
        NoiseSuppressionMode::Off => "Off",
        NoiseSuppressionMode::Rnnoise => "Rnnoise",
        NoiseSuppressionMode::DeepFilterNet3 => "DeepFilterNet3",
    }
}

fn parse_six_digit_code(raw: &str) -> Result<u32, MobileCoreError> {
    let digits: String = raw.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(MobileCoreError::InvalidArgument {
            message: "Room code must be exactly 6 digits".to_string(),
        });
    }
    digits
        .parse::<u32>()
        .map_err(|_| MobileCoreError::InvalidArgument {
            message: "Invalid 6-digit room code".to_string(),
        })
}

fn parse_ipv4_list(raw: &[String]) -> Vec<Ipv4Addr> {
    raw.iter()
        .filter_map(|s| s.trim().parse::<Ipv4Addr>().ok())
        .collect()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn decode_hex_16(hex: &str) -> Option<[u8; 16]> {
    let trimmed = hex.trim();
    if trimmed.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        let pair = trimmed.get(i * 2..i * 2 + 2)?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

fn decode_hex_32(hex: &str) -> Option<[u8; 32]> {
    let trimmed = hex.trim();
    if trimmed.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let pair = trimmed.get(i * 2..i * 2 + 2)?;
        *slot = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_digit_code_validation_handles_whitespace_and_errors() {
        assert_eq!(parse_six_digit_code("042 910").unwrap(), 42_910);
        assert_eq!(parse_six_digit_code("999999").unwrap(), 999_999);
        assert!(parse_six_digit_code("12345").is_err());
        assert!(parse_six_digit_code("12345a").is_err());
        assert!(parse_six_digit_code("1234567").is_err());
    }

    #[test]
    fn hex_and_ipv4_helpers_roundtrip() {
        let id = [0xABu8; 16];
        let hex16 = bytes_to_hex(&id);
        assert_eq!(decode_hex_16(&hex16), Some(id));
        assert_eq!(decode_hex_16("abcd"), None);

        let key = [0x4Cu8; 32];
        let hex32 = bytes_to_hex(&key);
        assert_eq!(decode_hex_32(&hex32), Some(key));
        assert_eq!(decode_hex_32("zz"), None);

        let ips = parse_ipv4_list(&[
            "192.168.1.15".to_string(),
            "not-an-ip".to_string(),
            "10.0.0.5".to_string(),
        ]);
        assert_eq!(
            ips,
            vec![Ipv4Addr::new(192, 168, 1, 15), Ipv4Addr::new(10, 0, 0, 5)]
        );
    }

    #[test]
    fn friend_items_sort_online_and_in_room_first() {
        let friends = vec![
            FriendContact {
                pub_key: [0x01; 32],
                nickname: "Zoe".to_string(),
                added_at_ms: 1,
            },
            FriendContact {
                pub_key: [0x02; 32],
                nickname: "Alice".to_string(),
                added_at_ms: 2,
            },
            FriendContact {
                pub_key: [0x03; 32],
                nickname: "Bob".to_string(),
                added_at_ms: 3,
            },
        ];
        let mut online = HashMap::new();
        online.insert(
            [0x01; 32],
            FriendPresenceUi {
                pub_key: [0x01; 32],
                nickname: "Zoe".to_string(),
                online: true,
                in_room: true,
            },
        );
        online.insert(
            [0x03; 32],
            FriendPresenceUi {
                pub_key: [0x03; 32],
                nickname: "Bob".to_string(),
                online: true,
                in_room: false,
            },
        );
        let items = build_friend_items(&friends, &online);
        assert_eq!(items[0].nickname, "Zoe");
        assert_eq!(items[1].nickname, "Bob");
        assert_eq!(items[2].nickname, "Alice");
    }
}
