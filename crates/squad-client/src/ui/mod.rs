pub mod avatar;
pub mod chat;
pub mod components;
pub mod lobby;
pub mod room;
pub mod settings;
pub mod share_modal;
pub mod stream_view;
pub mod theme;

use crate::config::AppConfig;
use crate::hotkeys::{request_macos_permissions, HotkeyController};
use crate::net::{
    downloads_root_dir, spawn_mesh_worker, ChatContentUi, ChatItemUi, DecodedVideoFrame,
    MeshCommand, MeshEvent, PeerUiSnapshot,
};
use crate::tray::{SystemTray, TrayAction};
use crate::ui::avatar::{
    hash_to_hex, hex_to_hash, load_cached_avatar_jpeg, process_custom_avatar_image,
    save_cached_avatar_jpeg, AvatarCache,
};
use crate::ui::share_modal::ShareSourceTab;
use crate::ui::theme::{
    material_dark_theme, pill_danger_button, pill_primary_button, pill_tonal_button,
    root_container_style, M3_ON_SURFACE, M3_OUTLINE, M3_SURFACE_CONTAINER,
};
use iced::{
    alignment::Vertical,
    clipboard, event, keyboard, time,
    widget::{button, column, container, horizontal_space, image as iced_image, row, stack, text},
    Background, Border, Color, ContentFit, Element, Length, Subscription, Task, Theme,
};
use std::borrow::Cow;
use image::imageops::FilterType;
use squad_dsp::{
    list_input_devices, list_output_devices, AudioEngineConfig, AudioEngineHandle,
    MicActivationMode, NoiseSuppressionMode, SoundCue,
};
use squad_proto::{ClientUuid, InviteLink, PeerId, INLINE_IMAGE_MAX_BYTES};
use squad_video::{
    detect_hardware_encoder_backend, enumerate_capture_targets, request_screen_capture_permission,
    CaptureTarget, StreamConfig, StreamFps, StreamResolution,
};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub const SYSTEM_DEFAULT_DEVICE: &str = "System Default (OS)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Lobby,
    Room,
    Settings,
}

#[derive(Debug, Clone)]
pub enum ClipboardPayload {
    InlineImage { filename: String, bytes: Vec<u8> },
    TempFile(PathBuf),
    InviteUri(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    CreateRoom,
    InviteInputChanged(String),
    JoinRoomWithInput,
    LeaveRoom,
    CopyInviteUri,
    OpenSettings,
    CloseSettings,
    ToggleMicMute,
    ToggleDeafen,
    SetActivationMode(MicActivationMode),
    SetAecEnabled(bool),
    SetNsMode(NoiseSuppressionMode),
    InputGainChanged(f32),
    VadThresholdChanged(f32),
    NicknameChanged(String),
    RendezvousServerChanged(String),
    HotkeyPtmChanged(String),
    HotkeyPttChanged(String),
    HotkeyToggleChanged(String),
    RefreshAudioDevices,
    InputDeviceSelected(String),
    OutputDeviceSelected(String),
    RequestMacosPermissions,
    SaveChannelNameChanged(String),
    SaveChannelFromInput,
    SaveActiveChannel,
    JoinSavedChannel(String),
    RemoveSavedChannel(String),
    ChatInputChanged(String),
    SendChatText,
    PickFileToSend,
    FilePicked(Option<PathBuf>),
    PasteFromClipboard {
        text_input_focused: bool,
    },
    ClipboardProcessed(Option<ClipboardPayload>),
    CopyChatText(u64, String),
    CopyImageToClipboard([u8; 16]),
    OpenImageLightbox([u8; 16]),
    CloseImageLightbox,
    SaveImageAs([u8; 16]),
    ClipboardActionDone(Result<(), String>),
    EscapePressed,
    DownloadBlob([u8; 16]),
    OpenDownloadsFolder(Option<PathBuf>),
    PeerVolumeChanged {
        peer_id: PeerId,
        client_uuid: ClientUuid,
        gain: f32,
    },
    OpenShareModal,
    CloseShareModal,
    SelectShareTab(ShareSourceTab),
    RefreshCaptureTargets,
    SelectCaptureTarget(usize),
    SetShareResolution(StreamResolution),
    SetShareFps(StreamFps),
    SetShareBitrate(u32),
    StartScreenShare,
    StopScreenShare,
    WatchPeerStream(PeerId),
    StopWatchingStream,
    ToggleStreamExpanded,
    PickAvatarImage,
    AvatarImagePicked(Option<PathBuf>),
    ResetAvatarIdenticon,
}

pub struct SquadApp {
    pub config: AppConfig,
    pub screen: Screen,
    pub previous_screen: Screen,
    pub audio: AudioEngineHandle,
    pub mesh_tx: mpsc::Sender<MeshCommand>,
    pub mesh_rx: Arc<Mutex<mpsc::Receiver<MeshEvent>>>,
    pub hotkeys: HotkeyController,
    pub tray: SystemTray,
    pub avatars: AvatarCache,
    pub invite_input: String,
    pub save_channel_name_input: String,
    pub active_invite_uri: String,
    pub local_peer_id: PeerId,
    pub peers: Vec<PeerUiSnapshot>,
    pub chat_messages: Vec<ChatItemUi>,
    pub image_handles: HashMap<[u8; 16], iced_image::Handle>,
    pub inline_image_bytes: HashMap<[u8; 16], (String, Vec<u8>)>,
    pub copied_msg_id: Option<(u64, std::time::Instant)>,
    pub lightbox_blob_id: Option<[u8; 16]>,
    pub chat_input: String,
    pub status_banner: Option<String>,
    pub invite_copied_flash: bool,
    pub input_devices: Vec<String>,
    pub output_devices: Vec<String>,
    pub video_frame_slot: Arc<Mutex<Option<DecodedVideoFrame>>>,
    pub share_modal_open: bool,
    pub share_tab: ShareSourceTab,
    pub capture_targets: Vec<CaptureTarget>,
    pub selected_capture_target_idx: usize,
    pub share_resolution: StreamResolution,
    pub share_fps: StreamFps,
    pub share_bitrate_bps: u32,
    pub local_encoder_backend: String,
    pub is_local_sharing: bool,
    pub local_stream_subscribers: usize,
    pub watched_peer_id: Option<PeerId>,
    pub active_stream_frame: Option<iced_image::Handle>,
    pub stream_expanded: bool,
}

impl SquadApp {
    pub fn new() -> (Self, Task<Message>) {
        let config = AppConfig::load_or_create();
        let (audio_out_tx, audio_out_rx) = mpsc::channel(256);

        let engine_cfg = AudioEngineConfig {
            input_device_name: config.input_device.clone(),
            output_device_name: config.output_device.clone(),
            aec_enabled: config.aec_enabled,
            ns_mode: config.ns_mode,
            activation_mode: config.activation_mode,
            input_gain: config.input_gain,
            vad_threshold_db: config.vad_threshold_db,
            bitrate_bps: 40_000,
        };

        let audio = AudioEngineHandle::start(engine_cfg, audio_out_tx)
            .expect("Failed to initialize Squad-Air DSP audio engine");

        let video_frame_slot = Arc::new(Mutex::new(None));
        let (mesh_tx, mesh_rx) =
            spawn_mesh_worker(audio.clone(), audio_out_rx, Arc::clone(&video_frame_slot));
        let hotkeys = HotkeyController::new(
            &config.hotkey_toggle_mute,
            &config.hotkey_ptt,
            &config.hotkey_ptm,
        );
        let tray = SystemTray::new();
        let mut config_mut = config;
        let avatars = AvatarCache::new();
        if let Some(ref hex_hash) = config_mut.custom_avatar_hash {
            if let Some(hash) = hex_to_hash(hex_hash) {
                if let Some(jpeg) = load_cached_avatar_jpeg(&hash) {
                    avatars.set_from_jpeg_option(config_mut.client_uuid, Some(&jpeg));
                    let _ = mesh_tx.try_send(MeshCommand::SetLocalAvatar {
                        avatar: Some((hash, jpeg)),
                    });
                } else {
                    config_mut.custom_avatar_hash = None;
                    let _ = config_mut.save();
                }
            } else {
                config_mut.custom_avatar_hash = None;
                let _ = config_mut.save();
            }
        }

        let app = Self {
            config: config_mut,
            screen: Screen::Lobby,
            previous_screen: Screen::Lobby,
            audio,
            mesh_tx,
            mesh_rx: Arc::new(Mutex::new(mesh_rx)),
            hotkeys,
            tray,
            avatars,
            invite_input: String::new(),
            save_channel_name_input: String::new(),
            active_invite_uri: String::new(),
            local_peer_id: 0,
            peers: Vec::new(),
            chat_messages: Vec::new(),
            image_handles: HashMap::new(),
            inline_image_bytes: HashMap::new(),
            copied_msg_id: None,
            lightbox_blob_id: None,
            chat_input: String::new(),
            status_banner: None,
            invite_copied_flash: false,
            input_devices: list_input_devices(),
            output_devices: list_output_devices(),
            video_frame_slot,
            share_modal_open: false,
            share_tab: ShareSourceTab::Monitors,
            capture_targets: Vec::new(),
            selected_capture_target_idx: 0,
            share_resolution: StreamResolution::R1080p,
            share_fps: StreamFps::Fps60,
            share_bitrate_bps: 6_000_000,
            local_encoder_backend: detect_hardware_encoder_backend(),
            is_local_sharing: false,
            local_stream_subscribers: 0,
            watched_peer_id: None,
            active_stream_frame: None,
            stream_expanded: false,
        };

        (app, Task::none())
    }

    pub fn title(&self) -> String {
        match self.screen {
            Screen::Lobby => format!("Squad-Air — {}", self.config.nickname),
            Screen::Room => format!(
                "Squad-Air • Connected ({}/10)",
                self.peers.len() + 1
            ),
            Screen::Settings => "Squad-Air — Settings".to_string(),
        }
    }

    pub fn theme(&self) -> Theme {
        material_dark_theme()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let tick = time::every(Duration::from_millis(20)).map(|_| Message::Tick);
        let keys = event::listen_with(|evt, status, _win| match evt {
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) => {
                let is_v = matches!(key, keyboard::Key::Character(ref ch) if ch.eq_ignore_ascii_case("v"))
                    || matches!(
                        physical_key,
                        keyboard::key::Physical::Code(keyboard::key::Code::KeyV)
                    );
                if modifiers.command() && is_v {
                    return Some(Message::PasteFromClipboard {
                        text_input_focused: status == iced::event::Status::Captured,
                    });
                }
                if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
                    return Some(Message::EscapePressed);
                }
                None
            }
            _ => None,
        });
        Subscription::batch([tick, keys])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.hotkeys.poll_events(&self.audio);
                if let Some((_, ts)) = self.copied_msg_id {
                    if ts.elapsed() >= Duration::from_secs(2) {
                        self.copied_msg_id = None;
                    }
                }
                if let Some(TrayAction::Quit) = self.tray.poll_action(&self.audio) {
                    let _ = self.mesh_tx.try_send(MeshCommand::Shutdown);
                    self.audio.shutdown();
                    std::process::exit(0);
                }

                if let Ok(mut rx) = self.mesh_rx.try_lock() {
                    while let Ok(evt) = rx.try_recv() {
                        match evt {
                            MeshEvent::JoinedRoom {
                                local_peer_id,
                                invite_uri,
                            } => {
                                self.local_peer_id = local_peer_id;
                                self.active_invite_uri = invite_uri.clone();
                                self.save_channel_name_input = self
                                    .config
                                    .saved_channels
                                    .iter()
                                    .find(|c| c.invite_uri == invite_uri)
                                    .map(|c| c.name.clone())
                                    .unwrap_or_default();
                                self.screen = Screen::Room;
                                self.status_banner = None;
                            }
                            MeshEvent::LeftRoom => {
                                self.peers.clear();
                                self.chat_messages.clear();
                                self.image_handles.clear();
                                self.inline_image_bytes.clear();
                                self.copied_msg_id = None;
                                self.lightbox_blob_id = None;
                                self.chat_input.clear();
                                self.local_peer_id = 0;
                                self.share_modal_open = false;
                                self.is_local_sharing = false;
                                self.local_stream_subscribers = 0;
                                self.watched_peer_id = None;
                                self.active_stream_frame = None;
                                self.stream_expanded = false;
                                if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                                    *slot = None;
                                }
                                self.screen = Screen::Lobby;
                            }
                            MeshEvent::PeersUpdated(snapshots) => {
                                for snap in &snapshots {
                                    let saved = self.config.peer_volume(&snap.client_uuid);
                                    self.audio.set_peer_gain(snap.peer_id, saved);
                                }
                                self.peers = snapshots;
                            }
                            MeshEvent::ChatItemReceived(item) => {
                                if let ChatContentUi::InlineImage {
                                    blob_id,
                                    ref filename,
                                    bytes: Some(ref raw),
                                    ..
                                } = item.content
                                {
                                    self.image_handles
                                        .insert(blob_id, iced_image::Handle::from_bytes(raw.clone()));
                                    self.inline_image_bytes
                                        .insert(blob_id, (filename.clone(), raw.clone()));
                                }
                                let already = self.chat_messages.iter().any(|existing| {
                                    existing.sender_uuid == item.sender_uuid
                                        && existing.msg_id == item.msg_id
                                });
                                if !already {
                                    self.chat_messages.push(item);
                                    self.chat_messages
                                        .sort_by_key(|m| (m.timestamp_ms, m.msg_id));
                                    if self.chat_messages.len() > 30 {
                                        let drop_n = self.chat_messages.len() - 30;
                                        self.chat_messages.drain(0..drop_n);
                                    }
                                }
                            }
                            MeshEvent::BlobStateChanged {
                                blob_id,
                                state,
                                inline_bytes,
                            } => {
                                if let Some(ref raw) = inline_bytes {
                                    self.image_handles
                                        .insert(blob_id, iced_image::Handle::from_bytes(raw.clone()));
                                }
                                for item in &mut self.chat_messages {
                                    match &mut item.content {
                                        ChatContentUi::InlineImage {
                                            blob_id: bid,
                                            filename,
                                            state: st,
                                            bytes,
                                            ..
                                        } if *bid == blob_id => {
                                            *st = state.clone();
                                            if let Some(ref raw) = inline_bytes {
                                                *bytes = Some(raw.clone());
                                                self.inline_image_bytes
                                                    .insert(blob_id, (filename.clone(), raw.clone()));
                                            }
                                        }
                                        ChatContentUi::File {
                                            blob_id: bid,
                                            state: st,
                                            ..
                                        } if *bid == blob_id => {
                                            *st = state.clone();
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            MeshEvent::RemoteStreamUpdated { peer_id, stream } => {
                                if let Some(peer) =
                                    self.peers.iter_mut().find(|p| p.peer_id == peer_id)
                                {
                                    peer.stream_info = stream.clone();
                                }
                                if stream.is_none() && self.watched_peer_id == Some(peer_id) {
                                    self.watched_peer_id = None;
                                    self.active_stream_frame = None;
                                    self.stream_expanded = false;
                                }
                            }
                            MeshEvent::LocalStreamStatus {
                                is_sharing,
                                subscriber_count,
                            } => {
                                self.is_local_sharing = is_sharing;
                                self.local_stream_subscribers = subscriber_count;
                            }
                            MeshEvent::PeerAvatarLoaded {
                                client_uuid,
                                avatar_hash,
                                jpeg_bytes,
                            } => {
                                if avatar_hash == [0u8; 32] {
                                    self.avatars.reset_to_identicon(client_uuid);
                                } else {
                                    self.avatars
                                        .set_from_jpeg_option(client_uuid, jpeg_bytes.as_deref());
                                }
                            }
                            MeshEvent::Error(err) => {
                                self.status_banner = Some(err);
                            }
                        }
                    }
                }
                if self.watched_peer_id.is_some() {
                    if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                        if let Some(frame) = slot.take() {
                            if Some(frame.streamer_peer_id) == self.watched_peer_id {
                                self.active_stream_frame = Some(iced_image::Handle::from_rgba(
                                    frame.width,
                                    frame.height,
                                    frame.rgba,
                                ));
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::CreateRoom => {
                let invite = InviteLink::generate();
                let uri = invite.to_uri();
                self.active_invite_uri = uri.clone();
                self.invite_copied_flash = true;
                self.join_with_invite(invite);
                clipboard::write(uri)
            }
            Message::InviteInputChanged(val) => {
                self.invite_input = val;
                Task::none()
            }
            Message::JoinRoomWithInput => {
                self.try_join_uri(&self.invite_input.clone());
                Task::none()
            }
            Message::JoinSavedChannel(uri) => {
                self.try_join_uri(&uri);
                Task::none()
            }
            Message::LeaveRoom => {
                let _ = self.mesh_tx.try_send(MeshCommand::LeaveRoom);
                self.peers.clear();
                self.chat_messages.clear();
                self.image_handles.clear();
                self.inline_image_bytes.clear();
                self.copied_msg_id = None;
                self.lightbox_blob_id = None;
                self.chat_input.clear();
                self.share_modal_open = false;
                self.is_local_sharing = false;
                self.local_stream_subscribers = 0;
                self.watched_peer_id = None;
                self.active_stream_frame = None;
                self.stream_expanded = false;
                if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                    *slot = None;
                }
                self.screen = Screen::Lobby;
                Task::none()
            }
            Message::CopyInviteUri => {
                self.invite_copied_flash = true;
                clipboard::write(self.active_invite_uri.clone())
            }
            Message::OpenSettings => {
                if self.screen != Screen::Settings {
                    self.previous_screen = self.screen;
                    self.screen = Screen::Settings;
                }
                Task::none()
            }
            Message::CloseSettings => {
                let _ = self.config.save();
                self.hotkeys.rebind(
                    &self.config.hotkey_toggle_mute,
                    &self.config.hotkey_ptt,
                    &self.config.hotkey_ptm,
                );
                self.screen = self.previous_screen;
                Task::none()
            }
            Message::ToggleMicMute => {
                let next = !self.audio.is_mic_muted();
                self.audio.set_mic_muted(next);
                self.audio.play_cue(if next {
                    SoundCue::Mute
                } else {
                    SoundCue::Unmute
                });
                Task::none()
            }
            Message::ToggleDeafen => {
                let next = !self.audio.is_deafened();
                self.audio.set_deafened(next);
                Task::none()
            }
            Message::SetActivationMode(mode) => {
                self.config.activation_mode = mode;
                self.audio.set_activation_mode(mode);
                let _ = self.config.save();
                Task::none()
            }
            Message::SetAecEnabled(enabled) => {
                self.config.aec_enabled = enabled;
                self.audio.set_aec_enabled(enabled);
                let _ = self.config.save();
                Task::none()
            }
            Message::SetNsMode(mode) => {
                self.config.ns_mode = mode;
                self.audio.set_ns_mode(mode);
                let _ = self.config.save();
                Task::none()
            }
            Message::InputGainChanged(gain) => {
                self.config.input_gain = gain;
                self.audio.set_input_gain(gain);
                Task::none()
            }
            Message::VadThresholdChanged(threshold_db) => {
                self.config.vad_threshold_db = threshold_db;
                self.audio.set_vad_threshold_db(threshold_db);
                let _ = self.config.save();
                Task::none()
            }
            Message::NicknameChanged(nick) => {
                self.config.nickname = nick;
                Task::none()
            }
            Message::RendezvousServerChanged(srv) => {
                self.config.rendezvous_server = srv;
                Task::none()
            }
            Message::HotkeyPtmChanged(val) => {
                self.config.hotkey_ptm = val;
                Task::none()
            }
            Message::HotkeyPttChanged(val) => {
                self.config.hotkey_ptt = val;
                Task::none()
            }
            Message::HotkeyToggleChanged(val) => {
                self.config.hotkey_toggle_mute = val;
                Task::none()
            }
            Message::RefreshAudioDevices => {
                self.input_devices = list_input_devices();
                self.output_devices = list_output_devices();
                Task::none()
            }
            Message::InputDeviceSelected(choice) => {
                self.config.input_device = if choice == SYSTEM_DEFAULT_DEVICE {
                    None
                } else {
                    Some(choice)
                };
                self.audio.set_audio_devices(
                    self.config.input_device.clone(),
                    self.config.output_device.clone(),
                );
                let _ = self.config.save();
                Task::none()
            }
            Message::OutputDeviceSelected(choice) => {
                self.config.output_device = if choice == SYSTEM_DEFAULT_DEVICE {
                    None
                } else {
                    Some(choice)
                };
                self.audio.set_audio_devices(
                    self.config.input_device.clone(),
                    self.config.output_device.clone(),
                );
                let _ = self.config.save();
                Task::none()
            }
            Message::RequestMacosPermissions => {
                request_macos_permissions();
                Task::none()
            }
            Message::SaveChannelNameChanged(name) => {
                self.save_channel_name_input = name;
                Task::none()
            }
            Message::SaveChannelFromInput => {
                match InviteLink::parse(&self.invite_input) {
                    Ok(invite) => {
                        let name = if self.save_channel_name_input.trim().is_empty() {
                            format!("Squad Room #{}", hex_prefix(&invite.room_prefix()))
                        } else {
                            self.save_channel_name_input.clone()
                        };
                        self.config.add_saved_channel(name, invite.to_uri());
                        self.save_channel_name_input.clear();
                        self.status_banner = None;
                    }
                    Err(e) => {
                        self.status_banner = Some(format!("Invalid invite link to save: {e}"));
                    }
                }
                Task::none()
            }
            Message::SaveActiveChannel => {
                if !self.active_invite_uri.is_empty() {
                    let name = if self.save_channel_name_input.trim().is_empty() {
                        InviteLink::parse(&self.active_invite_uri)
                            .map(|inv| format!("Squad Room #{}", hex_prefix(&inv.room_prefix())))
                            .unwrap_or_else(|_| "Squad Channel".to_string())
                    } else {
                        self.save_channel_name_input.clone()
                    };
                    self.config
                        .add_saved_channel(name.clone(), self.active_invite_uri.clone());
                    self.save_channel_name_input = name;
                }
                Task::none()
            }
            Message::RemoveSavedChannel(uri) => {
                self.config.remove_saved_channel(&uri);
                Task::none()
            }
            Message::ChatInputChanged(val) => {
                self.chat_input = val;
                Task::none()
            }
            Message::SendChatText => {
                let text = self.chat_input.trim().to_string();
                if !text.is_empty() {
                    self.chat_input.clear();
                    let _ = self.mesh_tx.try_send(MeshCommand::SendChatText { text });
                }
                Task::none()
            }
            Message::PickFileToSend => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .pick_file()
                        .await
                        .map(|handle| handle.path().to_path_buf())
                },
                Message::FilePicked,
            ),
            Message::FilePicked(Some(path)) => {
                let _ = self.mesh_tx.try_send(MeshCommand::OfferFile { path });
                Task::none()
            }
            Message::FilePicked(None) => Task::none(),
            Message::PasteFromClipboard { text_input_focused } => {
                let screen = self.screen;
                let invite_empty = self.invite_input.trim().is_empty();
                if screen != Screen::Room && (screen != Screen::Lobby || text_input_focused) {
                    return Task::none();
                }
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            read_clipboard_payload(screen, text_input_focused, invite_empty)
                        })
                        .await
                        .ok()
                        .flatten()
                    },
                    Message::ClipboardProcessed,
                )
            }
            Message::ClipboardProcessed(Some(payload)) => {
                match payload {
                    ClipboardPayload::InlineImage { filename, bytes } => {
                        if self.screen == Screen::Room {
                            let _ = self
                                .mesh_tx
                                .try_send(MeshCommand::SendInlineImage { filename, bytes });
                        }
                    }
                    ClipboardPayload::TempFile(path) => {
                        if self.screen == Screen::Room {
                            let _ = self.mesh_tx.try_send(MeshCommand::OfferFile { path });
                        }
                    }
                    ClipboardPayload::InviteUri(uri) => {
                        if self.screen == Screen::Lobby {
                            self.invite_input = uri;
                        }
                    }
                }
                Task::none()
            }
            Message::ClipboardProcessed(None) => Task::none(),
            Message::CopyChatText(msg_id, text) => {
                self.copied_msg_id = Some((msg_id, std::time::Instant::now()));
                clipboard::write(text)
            }
            Message::CopyImageToClipboard(blob_id) => {
                if let Some((_, raw)) = self.inline_image_bytes.get(&blob_id).cloned() {
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || copy_encoded_image_to_clipboard(&raw))
                                .await
                                .map_err(|e| format!("Clipboard task failed: {e}"))?
                        },
                        Message::ClipboardActionDone,
                    );
                }
                Task::none()
            }
            Message::OpenImageLightbox(blob_id) => {
                if self.image_handles.contains_key(&blob_id) {
                    self.lightbox_blob_id = Some(blob_id);
                }
                Task::none()
            }
            Message::CloseImageLightbox => {
                self.lightbox_blob_id = None;
                Task::none()
            }
            Message::SaveImageAs(blob_id) => {
                if let Some((filename, raw)) = self.inline_image_bytes.get(&blob_id).cloned() {
                    return Task::perform(
                        async move {
                            if let Some(handle) = rfd::AsyncFileDialog::new()
                                .set_file_name(&filename)
                                .save_file()
                                .await
                            {
                                std::fs::write(handle.path(), &raw)
                                    .map_err(|e| format!("Failed to save image: {e}"))?;
                            }
                            Ok(())
                        },
                        Message::ClipboardActionDone,
                    );
                }
                Task::none()
            }
            Message::ClipboardActionDone(res) => {
                if let Err(err) = res {
                    self.status_banner = Some(err);
                }
                Task::none()
            }
            Message::EscapePressed => {
                self.lightbox_blob_id = None;
                self.share_modal_open = false;
                Task::none()
            }
            Message::DownloadBlob(blob_id) => {
                let _ = self.mesh_tx.try_send(MeshCommand::DownloadBlob { blob_id });
                Task::none()
            }
            Message::OpenDownloadsFolder(saved_path) => {
                open_in_file_manager(saved_path);
                Task::none()
            }
            Message::PeerVolumeChanged {
                peer_id,
                client_uuid,
                gain,
            } => {
                self.audio.set_peer_gain(peer_id, gain);
                self.config.set_peer_volume(&client_uuid, gain);
                Task::none()
            }
            Message::OpenShareModal => {
                request_screen_capture_permission();
                self.capture_targets = enumerate_capture_targets();
                self.select_first_target_for_tab();
                self.share_modal_open = true;
                Task::none()
            }
            Message::CloseShareModal => {
                self.share_modal_open = false;
                Task::none()
            }
            Message::SelectShareTab(tab) => {
                self.share_tab = tab;
                self.select_first_target_for_tab();
                Task::none()
            }
            Message::RefreshCaptureTargets => {
                self.capture_targets = enumerate_capture_targets();
                self.select_first_target_for_tab();
                Task::none()
            }
            Message::SelectCaptureTarget(idx) => {
                if idx < self.capture_targets.len() {
                    self.selected_capture_target_idx = idx;
                }
                Task::none()
            }
            Message::SetShareResolution(res) => {
                self.share_resolution = res;
                Task::none()
            }
            Message::SetShareFps(fps) => {
                self.share_fps = fps;
                Task::none()
            }
            Message::SetShareBitrate(bps) => {
                self.share_bitrate_bps = bps;
                Task::none()
            }
            Message::StartScreenShare => {
                if let Some(target) = self
                    .capture_targets
                    .get(self.selected_capture_target_idx)
                    .cloned()
                {
                    let config = StreamConfig {
                        target,
                        resolution: self.share_resolution,
                        fps: self.share_fps,
                        bitrate_bps: self.share_bitrate_bps,
                    };
                    let _ = self.mesh_tx.try_send(MeshCommand::StartScreenShare { config });
                    self.is_local_sharing = true;
                    self.share_modal_open = false;
                }
                Task::none()
            }
            Message::StopScreenShare => {
                let _ = self.mesh_tx.try_send(MeshCommand::StopScreenShare);
                self.is_local_sharing = false;
                self.local_stream_subscribers = 0;
                Task::none()
            }
            Message::WatchPeerStream(peer_id) => {
                self.watched_peer_id = Some(peer_id);
                self.active_stream_frame = None;
                self.stream_expanded = false;
                if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                    *slot = None;
                }
                let _ = self
                    .mesh_tx
                    .try_send(MeshCommand::WatchPeerStream { peer_id: Some(peer_id) });
                Task::none()
            }
            Message::StopWatchingStream => {
                self.watched_peer_id = None;
                self.active_stream_frame = None;
                self.stream_expanded = false;
                if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                    *slot = None;
                }
                let _ = self
                    .mesh_tx
                    .try_send(MeshCommand::WatchPeerStream { peer_id: None });
                Task::none()
            }
            Message::ToggleStreamExpanded => {
                self.stream_expanded = !self.stream_expanded;
                Task::none()
            }
            Message::PickAvatarImage => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                        .pick_file()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                Message::AvatarImagePicked,
            ),
            Message::AvatarImagePicked(Some(path)) => {
                match std::fs::read(&path)
                    .map_err(|e| format!("Failed to read image: {e}"))
                    .and_then(|bytes| process_custom_avatar_image(&bytes))
                {
                    Ok((jpeg, hash, rgba)) => {
                        let _ = save_cached_avatar_jpeg(&hash, &jpeg);
                        self.avatars.set_custom_rgba(self.config.client_uuid, rgba);
                        self.config.custom_avatar_hash = Some(hash_to_hex(&hash));
                        let _ = self.config.save();
                        self.status_banner = None;
                        let _ = self.mesh_tx.try_send(MeshCommand::SetLocalAvatar {
                            avatar: Some((hash, jpeg)),
                        });
                    }
                    Err(err) => {
                        self.status_banner = Some(err);
                    }
                }
                Task::none()
            }
            Message::AvatarImagePicked(None) => Task::none(),
            Message::ResetAvatarIdenticon => {
                self.avatars.reset_to_identicon(self.config.client_uuid);
                self.config.custom_avatar_hash = None;
                let _ = self.config.save();
                let _ = self
                    .mesh_tx
                    .try_send(MeshCommand::SetLocalAvatar { avatar: None });
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let content = match self.screen {
            Screen::Lobby => lobby::view_lobby(self),
            Screen::Room => room::view_room(self),
            Screen::Settings => settings::view_settings(self),
        };

        let base = container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(root_container_style);

        if let Some(blob_id) = self.lightbox_blob_id {
            if let Some(handle) = self.image_handles.get(&blob_id) {
                let title = self
                    .inline_image_bytes
                    .get(&blob_id)
                    .map(|(name, _)| name.as_str())
                    .unwrap_or("Image Preview");
                return stack![base, view_image_lightbox(blob_id, title, handle.clone())].into();
            }
        }

        if self.share_modal_open {
            stack![base, share_modal::view_share_modal(self)].into()
        } else {
            base.into()
        }
    }

    fn select_first_target_for_tab(&mut self) {
        let current_matches = self
            .capture_targets
            .get(self.selected_capture_target_idx)
            .map(|t| match self.share_tab {
                ShareSourceTab::Monitors => t.is_monitor(),
                ShareSourceTab::Windows => !t.is_monitor(),
            })
            .unwrap_or(false);
        if !current_matches {
            if let Some((idx, _)) = self.capture_targets.iter().enumerate().find(|(_, t)| {
                match self.share_tab {
                    ShareSourceTab::Monitors => t.is_monitor(),
                    ShareSourceTab::Windows => !t.is_monitor(),
                }
            }) {
                self.selected_capture_target_idx = idx;
            }
        }
    }

    fn try_join_uri(&mut self, uri: &str) {
        match InviteLink::parse(uri) {
            Ok(invite) => {
                self.active_invite_uri = invite.to_uri();
                self.invite_copied_flash = false;
                self.join_with_invite(invite);
            }
            Err(e) => {
                self.status_banner = Some(format!("Invalid invite link: {e}"));
            }
        }
    }

    fn join_with_invite(&mut self, invite: InviteLink) {
        self.chat_messages.clear();
        self.image_handles.clear();
        self.inline_image_bytes.clear();
        self.copied_msg_id = None;
        self.lightbox_blob_id = None;
        self.chat_input.clear();
        let _ = self.mesh_tx.try_send(MeshCommand::JoinRoom {
            rendezvous_server: self.config.rendezvous_server.clone(),
            invite,
            nickname: self.config.nickname.clone(),
            client_uuid: self.config.client_uuid(),
            initial_volumes: HashMap::new(),
        });
        self.screen = Screen::Room;
    }
}

fn view_image_lightbox<'a>(
    blob_id: [u8; 16],
    filename: &'a str,
    handle: iced_image::Handle,
) -> Element<'a, Message> {
    let header = row![
        text(filename).size(14).color(M3_ON_SURFACE),
        horizontal_space(),
        button(text("Copy Image").size(12))
            .padding([8, 14])
            .style(pill_primary_button)
            .on_press(Message::CopyImageToClipboard(blob_id)),
        button(text("Save As...").size(12))
            .padding([8, 14])
            .style(pill_tonal_button)
            .on_press(Message::SaveImageAs(blob_id)),
        button(text("Close (Esc)").size(12))
            .padding([8, 14])
            .style(pill_danger_button)
            .on_press(Message::CloseImageLightbox),
    ]
    .spacing(10)
    .align_y(Vertical::Center);

    let card = container(
        column![
            header,
            container(
                iced_image(handle)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Contain),
            )
            .width(Length::Fill)
            .height(Length::Fill),
        ]
        .spacing(12)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(20)
    .style(|_t: &Theme| container::Style {
        background: Some(Background::Color(M3_SURFACE_CONTAINER)),
        border: Border {
            color: M3_OUTLINE,
            width: 1.0,
            radius: 16.0.into(),
        },
        ..Default::default()
    });

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(32)
        .style(|_t: &Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.84))),
            ..Default::default()
        })
        .into()
}

fn read_clipboard_payload(
    screen: Screen,
    text_input_focused: bool,
    invite_empty: bool,
) -> Option<ClipboardPayload> {
    let mut cb = arboard::Clipboard::new().ok()?;

    if screen == Screen::Lobby {
        if !text_input_focused && invite_empty {
            if let Ok(txt) = cb.get_text() {
                let trimmed = txt.trim();
                if trimmed.starts_with("squad://") {
                    return Some(ClipboardPayload::InviteUri(trimmed.to_string()));
                }
            }
        }
        return None;
    }

    if screen != Screen::Room {
        return None;
    }

    let Ok(img_data) = cb.get_image() else {
        return None;
    };
    let rgba = image::RgbaImage::from_raw(
        img_data.width as u32,
        img_data.height as u32,
        img_data.bytes.into_owned(),
    )?;

    let mut dyn_img = image::DynamicImage::ImageRgba8(rgba);
    if dyn_img.width() > 1920 || dyn_img.height() > 1920 {
        dyn_img = dyn_img.resize(1920, 1920, FilterType::Triangle);
    }

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    for &(max_dim, quality) in &[(1920u32, 85u8), (1280, 75), (1024, 58), (800, 45), (640, 35)] {
        if dyn_img.width() > max_dim || dyn_img.height() > max_dim {
            dyn_img = dyn_img.resize(max_dim, max_dim, FilterType::Triangle);
        }
        let rgb = dyn_img.to_rgb8();
        let mut buf = Cursor::new(Vec::new());
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        if encoder.encode_image(&rgb).is_ok() {
            let bytes = buf.into_inner();
            if !bytes.is_empty() && bytes.len() < INLINE_IMAGE_MAX_BYTES {
                return Some(ClipboardPayload::InlineImage {
                    filename: format!("clipboard_{ts}.jpg"),
                    bytes,
                });
            }
        }
    }

    let tmp_path = std::env::temp_dir().join(format!("squad_clipboard_{ts}.png"));
    if dyn_img.save(&tmp_path).is_ok() {
        return Some(ClipboardPayload::TempFile(tmp_path));
    }
    None
}

fn copy_encoded_image_to_clipboard(encoded_bytes: &[u8]) -> Result<(), String> {
    let img = image::load_from_memory(encoded_bytes)
        .map_err(|e| format!("Failed to decode image for clipboard: {e}"))?;
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    let mut cb =
        arboard::Clipboard::new().map_err(|e| format!("Clipboard unavailable: {e}"))?;
    cb.set_image(arboard::ImageData {
        width,
        height,
        bytes: Cow::Owned(rgba.into_raw()),
    })
    .map_err(|e| format!("Failed to write image to clipboard: {e}"))
}

fn open_in_file_manager(saved_path: Option<PathBuf>) {
    #[cfg(target_os = "macos")]
    {
        if let Some(path) = saved_path.filter(|p| p.exists()) {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn();
            return;
        }
        let dir = downloads_root_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(path) = saved_path.filter(|p| p.exists()) {
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .spawn();
            return;
        }
        let dir = downloads_root_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let dir = saved_path
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(downloads_root_dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

fn hex_prefix(bytes: &[u8; 4]) -> String {
    format!("{:02X}{:02X}{:02X}{:02X}", bytes[0], bytes[1], bytes[2], bytes[3])
}
