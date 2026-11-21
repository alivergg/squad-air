pub mod avatar;
pub mod chat;
pub mod components;
pub mod incoming_call;
pub mod lobby;
pub mod overlay;
pub mod overlay_platform;
pub mod room;
pub mod settings;
pub mod share_modal;
pub mod shortcut_normalizer;
pub mod stream_view;
pub mod theme;
pub mod video_shader;

use crate::config::{
    matching_rendezvous_preset, AppConfig, CUSTOM_RENDEZVOUS_OPTION, DEFAULT_RENDEZVOUS_SERVER,
};
use crate::crypto_identity::{
    format_friend_code, load_friends, parse_friend_code, save_friends, FriendContact,
};
use crate::hotkeys::{is_supported_hotkey_name, request_macos_permissions, HotkeyController};
use crate::net::{
    downloads_root_dir, spawn_mesh_worker, ChatContentUi, ChatItemUi, DecodedVideoFrame,
    DiscoveredLanRoom, FriendPresenceUi, IncomingFriendCallUi, MeshCommand, MeshEvent,
    PeerUiSnapshot,
};
use crate::tray::{release_os_memory, SystemTray, TrayAction};
use crate::ui::avatar::{
    hash_to_hex, hex_to_hash, load_cached_avatar_jpeg, process_custom_avatar_image,
    save_cached_avatar_jpeg, AvatarCache,
};
use crate::ui::share_modal::ShareSourceTab;
use crate::ui::shortcut_normalizer::{matches_layout_agnostic_key, shortcut_normalizer};
#[cfg_attr(feature = "managed_updates", allow(unused_imports))]
use crate::ui::theme::{
    material_dark_theme, pill_danger_button, pill_primary_button, pill_success_button,
    pill_tonal_button, root_container_style, M3_ON_SURFACE, M3_ON_SURFACE_VARIANT, M3_OUTLINE,
    M3_OUTLINE_VARIANT, M3_PRIMARY, M3_SURFACE_CONTAINER, M3_SURFACE_HIGH,
};
#[cfg_attr(feature = "managed_updates", allow(unused_imports))]
use iced::{
    advanced::subscription::{from_recipe, EventStream, Hasher, Recipe},
    alignment::Vertical,
    clipboard, event,
    futures::{stream::BoxStream, SinkExt},
    keyboard, mouse, stream,
    widget::{
        button, column, container, horizontal_space, image as iced_image, row, scrollable, stack,
        text, text_editor,
    },
    Background, Border, Color, ContentFit, Element, Length, Point, Size, Subscription, Task, Theme,
};
use image::imageops::FilterType;
use squad_dsp::{
    list_input_devices, list_output_devices, AudioEngineConfig, AudioEngineHandle, JitterProfile,
    MicActivationMode, NoiseSuppressionMode, SoundCue,
};
use squad_proto::{ClientUuid, InviteLink, PeerId, INLINE_IMAGE_MAX_BYTES};
use squad_video::{
    enumerate_capture_targets, request_screen_capture_permission, CaptureTarget, StreamConfig,
    StreamFps, StreamResolution, VideoCodec,
};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, Notify};

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
    PlainText(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySlot {
    Ptm,
    Ptt,
    ToggleMute,
    Deafen,
}

#[derive(Debug, Clone)]
pub enum Message {
    MeshEventsPending,
    TrayEventsPending,
    VideoFrameReady,
    AudioStateChanged,
    ClearCopiedFlash,
    ReleaseMemory,
    CreateRoom,
    LanRoomNameChanged(String),
    HostLanRoom,
    JoinLanRoom(DiscoveredLanRoom),
    InviteInputChanged(String),
    PasteInviteFromClipboard,
    InviteClipboardLoaded(Option<String>),
    JoinRoomWithInput,
    LeaveRoom,
    CopyInviteUri,
    OpenSettings,
    CloseSettings,
    ToggleMicMute,
    ToggleDeafen,
    ToggleMicLoopback,
    SetActivationMode(MicActivationMode),
    SetAecEnabled(bool),
    SetNsMode(NoiseSuppressionMode),
    SetJitterProfile(JitterProfile),
    SetOpusBitrate(i32),
    InputGainChanged(f32),
    CueVolumeChanged(f32),
    CueVolumeReleased,
    TestCueSound,
    VadThresholdChanged(f32),
    NicknameChanged(String),
    RendezvousPresetSelected(String),
    RendezvousServerChanged(String),
    StartRecordingHotkey(HotkeySlot),
    CancelRecordingHotkey,
    RecordedHotkeyInput(String),
    ClearHotkey(HotkeySlot),
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
    ChatEditorAction((ClientUuid, u64), text_editor::Action),
    CopySelectedChatText,
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
    SetShareCodec(VideoCodec),
    ToggleShareAudio,
    StartScreenShare,
    StopScreenShare,
    ToggleLocalStreamPreview,
    WatchPeerStream(PeerId),
    StopWatchingStream,
    StreamVolumeChanged(f32),
    ToggleStreamExpanded,
    ToggleStreamFullscreen,
    ExitStreamFullscreen,
    PickAvatarImage,
    AvatarImagePicked(Option<PathBuf>),
    ResetAvatarIdenticon,
    SetMinimizeToTray(bool),
    SetOverlayEnabled(bool),
    SetOverlaySpeakingOnly(bool),
    OverlayOpacityChanged(f32),
    WindowOpened(iced::window::Id),
    OverlayWindowOpened(iced::window::Id),
    OverlayConfigured,
    WindowCloseRequested(iced::window::Id),
    #[cfg(not(feature = "managed_updates"))]
    CheckForUpdatesManual,
    #[cfg(not(feature = "managed_updates"))]
    UpdateCheckCompleted(Result<Option<crate::updater::ReleaseInfo>, String>),
    #[cfg(not(feature = "managed_updates"))]
    OpenUpdateModal,
    #[cfg(not(feature = "managed_updates"))]
    DismissUpdateModal,
    #[cfg(not(feature = "managed_updates"))]
    SkipUpdateVersion,
    #[cfg(not(feature = "managed_updates"))]
    StartOneClickUpdate,
    #[cfg(not(feature = "managed_updates"))]
    UpdateInstallFinished(Result<String, String>),
    CopyMyFriendId,
    OpenAddFriendModal,
    CloseAddFriendModal,
    AddFriendCodeChanged(String),
    AddFriendNickChanged(String),
    ConfirmAddFriend,
    RemoveFriend([u8; 32]),
    CallFriend([u8; 32]),
    OpenInviteFriendsModal,
    CloseInviteFriendsModal,
    InviteFriendToParty([u8; 32]),
    AcceptIncomingCall,
    DeclineIncomingCall,
    ShortCodeInputChanged(String),
    JoinByShortCode,
    GenerateShortCode,
    CopyShortCode,
    LanRoomPasswordChanged(String),
    ToggleLanHostPasswordVisible,
    LanJoinPasswordChanged(String),
    ToggleLanJoinPasswordVisible,
    ConfirmJoinLockedLanRoom,
    CloseLanPasswordModal,
}

struct UiSyncState {
    mesh_notify: Arc<Notify>,
    tray_notify: Arc<Notify>,
    video_notify: Arc<Notify>,
    audio_ui_notify: Arc<Notify>,
    audio: AudioEngineHandle,
    in_room: AtomicBool,
    in_settings: AtomicBool,
    in_tray: AtomicBool,
    has_overlay: AtomicBool,
    watching_video: AtomicBool,
}

struct AppWorkerRecipe(Arc<UiSyncState>);

impl Recipe for AppWorkerRecipe {
    type Output = Message;

    fn hash(&self, state: &mut Hasher) {
        use std::hash::Hash;
        0x51A0_0001u64.hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<'static, Self::Output> {
        let sync = self.0;
        Box::pin(stream::channel(64, |mut output| async move {
            let mut last_audio_snapshot: Option<(bool, bool, bool, i32, u32)> = None;
            let mut poll_interval = tokio::time::interval(Duration::from_millis(66));
            poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                let in_room = sync.in_room.load(Ordering::Acquire);
                let in_settings = sync.in_settings.load(Ordering::Acquire);
                let in_tray = sync.in_tray.load(Ordering::Acquire);
                let has_overlay = sync.has_overlay.load(Ordering::Acquire);
                let need_audio_poll = (in_room || in_settings) && (!in_tray || has_overlay);

                if !need_audio_poll {
                    last_audio_snapshot = None;
                }

                tokio::select! {
                    biased;

                    _ = sync.mesh_notify.notified() => {
                        let _ = output.send(Message::MeshEventsPending).await;
                    }
                    _ = sync.tray_notify.notified() => {
                        let _ = output.send(Message::TrayEventsPending).await;
                    }
                    _ = sync.video_notify.notified(), if sync.watching_video.load(Ordering::Acquire) => {
                        let _ = output.send(Message::VideoFrameReady).await;
                    }
                    _ = sync.audio_ui_notify.notified() => {
                        if (sync.in_room.load(Ordering::Acquire) || sync.in_settings.load(Ordering::Acquire))
                            && (!sync.in_tray.load(Ordering::Acquire) || sync.has_overlay.load(Ordering::Acquire))
                        {
                            let snap = sample_audio_ui_snapshot(&sync);
                            if last_audio_snapshot != Some(snap) {
                                last_audio_snapshot = Some(snap);
                                let _ = output.send(Message::AudioStateChanged).await;
                            }
                        }
                    }
                    _ = poll_interval.tick(), if need_audio_poll => {
                        let snap = sample_audio_ui_snapshot(&sync);
                        if last_audio_snapshot != Some(snap) {
                            last_audio_snapshot = Some(snap);
                            let _ = output.send(Message::AudioStateChanged).await;
                        }
                    }
                }
            }
        }))
    }
}

fn sample_audio_ui_snapshot(sync: &UiSyncState) -> (bool, bool, bool, i32, u32) {
    let speaking = sync.audio.is_local_speaking();
    let muted = sync.audio.is_mic_muted();
    let deafened = sync.audio.is_deafened();
    let dev_gen = sync.audio.device_list_gen();
    let in_tray = sync.in_tray.load(Ordering::Acquire);
    let db_bucket = if in_tray || muted || deafened {
        -60
    } else {
        let db = sync.audio.local_mic_level_db();
        if db <= -58.5 {
            -60
        } else {
            db.round() as i32
        }
    };
    (speaking, muted, deafened, db_bucket, dev_gen)
}

fn main_window_settings() -> iced::window::Settings {
    iced::window::Settings {
        size: Size::new(1180.0, 740.0),
        icon: crate::load_window_icon(),
        exit_on_close_request: false,
        ..Default::default()
    }
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
    pub chat_text_editors: HashMap<(ClientUuid, u64), text_editor::Content>,
    pub active_chat_selection: Option<String>,
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
    pub share_codec: VideoCodec,
    pub share_audio: bool,
    pub is_local_sharing: bool,
    pub local_stream_preview: bool,
    pub local_stream_subscribers: usize,
    pub watched_peer_id: Option<PeerId>,
    pub active_stream_frame: Option<Arc<DecodedVideoFrame>>,
    pub stream_expanded: bool,
    pub stream_fullscreen: bool,
    pub is_in_tray: bool,
    pub main_window_id: Option<iced::window::Id>,
    pub overlay_window_id: Option<iced::window::Id>,
    pub recording_hotkey: Option<HotkeySlot>,
    pub lan_rooms: Vec<DiscoveredLanRoom>,
    pub lan_room_name_input: String,
    pub lan_room_password_input: String,
    pub lan_host_password_visible: bool,
    pub lan_password_modal_room: Option<DiscoveredLanRoom>,
    pub lan_join_password_input: String,
    pub lan_join_password_visible: bool,
    pub lan_join_password_error: Option<String>,
    pub lan_join_in_progress: bool,
    pub active_lan_locked: bool,
    #[cfg(not(feature = "managed_updates"))]
    pub available_update: Option<crate::updater::ReleaseInfo>,
    #[cfg(not(feature = "managed_updates"))]
    pub update_modal_open: bool,
    #[cfg(not(feature = "managed_updates"))]
    pub update_in_progress: bool,
    #[cfg(not(feature = "managed_updates"))]
    pub update_status_text: Option<String>,
    #[cfg(not(feature = "managed_updates"))]
    update_manual_check: bool,
    pub friends: Vec<FriendContact>,
    pub friend_presences: HashMap<[u8; 32], FriendPresenceUi>,
    pub friend_id_copied_flash: bool,
    pub add_friend_modal_open: bool,
    pub add_friend_code_input: String,
    pub add_friend_nick_input: String,
    pub add_friend_error: Option<String>,
    pub invite_friends_modal_open: bool,
    pub invited_friends_session: HashSet<[u8; 32]>,
    pub incoming_call: Option<IncomingFriendCallUi>,
    pub short_code_input: String,
    pub short_code_joining: bool,
    pub active_short_code: Option<(u32, std::time::Instant)>,
    pub short_code_copied_flash: bool,
    pub rendezvous_custom_mode: bool,
    ui_sync: Arc<UiSyncState>,
}

impl SquadApp {
    pub(crate) fn has_relayed_peer(&self) -> bool {
        self.peers
            .iter()
            .any(|p| p.status == crate::net::PeerConnStatus::Relayed)
    }

    pub(crate) fn is_friend_in_current_room(&self, pub_key: &[u8; 32]) -> bool {
        self.screen == Screen::Room
            && self
                .peers
                .iter()
                .any(|p| p.pub_key.as_ref() == Some(pub_key))
    }

    pub fn new() -> (Self, Task<Message>) {
        let config = AppConfig::load_or_create();
        let friends = load_friends();
        let (audio_out_tx, audio_out_rx) = mpsc::channel(256);

        let engine_cfg = AudioEngineConfig {
            input_device_name: config.input_device.clone(),
            output_device_name: config.output_device.clone(),
            aec_enabled: config.aec_enabled,
            ns_mode: config.ns_mode,
            activation_mode: config.activation_mode,
            input_gain: config.input_gain,
            cue_gain: config.cue_volume,
            vad_threshold_db: config.vad_threshold_db,
            bitrate_bps: config.opus_bitrate_bps,
            jitter_profile: config.jitter_profile,
        };

        let audio = AudioEngineHandle::start(engine_cfg, audio_out_tx)
            .expect("Failed to initialize SquadAir DSP audio engine");
        audio.set_stream_gain(config.stream_volume);

        let mesh_notify = Arc::new(Notify::new());
        let tray_notify = Arc::new(Notify::new());
        let video_notify = Arc::new(Notify::new());
        let audio_ui_notify = Arc::new(Notify::new());

        let video_frame_slot = Arc::new(Mutex::new(None));
        let (mesh_tx, mesh_rx) = spawn_mesh_worker(
            audio.clone(),
            audio_out_rx,
            Arc::clone(&video_frame_slot),
            Arc::clone(&mesh_notify),
            Arc::clone(&video_notify),
        );
        let hotkeys = HotkeyController::new(
            &config.hotkey_toggle_mute,
            &config.hotkey_ptt,
            &config.hotkey_ptm,
            &config.hotkey_deafen,
            audio.clone(),
            Arc::clone(&audio_ui_notify),
        );
        let tray = SystemTray::new(Arc::clone(&tray_notify));
        let ui_sync = Arc::new(UiSyncState {
            mesh_notify,
            tray_notify,
            video_notify,
            audio_ui_notify,
            audio: audio.clone(),
            in_room: AtomicBool::new(false),
            in_settings: AtomicBool::new(false),
            in_tray: AtomicBool::new(false),
            has_overlay: AtomicBool::new(false),
            watching_video: AtomicBool::new(false),
        });
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

        let (main_window_id, open_main_task) = iced::window::open(main_window_settings());

        let rendezvous_custom_mode =
            matching_rendezvous_preset(&config_mut.rendezvous_server).is_none();

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
            chat_text_editors: HashMap::new(),
            active_chat_selection: None,
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
            share_bitrate_bps: 8_000_000,
            share_codec: VideoCodec::H264,
            share_audio: true,
            is_local_sharing: false,
            local_stream_preview: false,
            local_stream_subscribers: 0,
            watched_peer_id: None,
            active_stream_frame: None,
            stream_expanded: false,
            stream_fullscreen: false,
            is_in_tray: false,
            main_window_id: Some(main_window_id),
            overlay_window_id: None,
            recording_hotkey: None,
            lan_rooms: Vec::new(),
            lan_room_name_input: String::new(),
            lan_room_password_input: String::new(),
            lan_host_password_visible: false,
            lan_password_modal_room: None,
            lan_join_password_input: String::new(),
            lan_join_password_visible: false,
            lan_join_password_error: None,
            lan_join_in_progress: false,
            active_lan_locked: false,
            #[cfg(not(feature = "managed_updates"))]
            available_update: None,
            #[cfg(not(feature = "managed_updates"))]
            update_modal_open: false,
            #[cfg(not(feature = "managed_updates"))]
            update_in_progress: false,
            #[cfg(not(feature = "managed_updates"))]
            update_status_text: None,
            #[cfg(not(feature = "managed_updates"))]
            update_manual_check: false,
            friends,
            friend_presences: HashMap::new(),
            friend_id_copied_flash: false,
            add_friend_modal_open: false,
            add_friend_code_input: String::new(),
            add_friend_nick_input: String::new(),
            add_friend_error: None,
            invite_friends_modal_open: false,
            invited_friends_session: HashSet::new(),
            incoming_call: None,
            short_code_input: String::new(),
            short_code_joining: false,
            active_short_code: None,
            short_code_copied_flash: false,
            rendezvous_custom_mode,
            ui_sync,
        };
        app.sync_presence_to_mesh();

        #[cfg(not(feature = "managed_updates"))]
        let startup_task = {
            let update_task = Task::perform(
                async {
                    tokio::task::spawn_blocking(crate::updater::check_latest_release)
                        .await
                        .map_err(|e| format!("Update check failed: {e}"))
                        .and_then(|res| res)
                },
                Message::UpdateCheckCompleted,
            );
            Task::batch([open_main_task.map(Message::WindowOpened), update_task])
        };

        #[cfg(feature = "managed_updates")]
        let startup_task = open_main_task.map(Message::WindowOpened);

        (app, startup_task)
    }

    pub fn title(&self, window_id: iced::window::Id) -> String {
        if Some(window_id) == self.overlay_window_id {
            return "SquadAir Overlay".to_string();
        }
        match self.screen {
            Screen::Lobby => "SquadAir".to_string(),
            Screen::Room => "SquadAir — Voice Room".to_string(),
            Screen::Settings => "SquadAir — Settings".to_string(),
        }
    }

    pub fn theme(&self, _window_id: iced::window::Id) -> Theme {
        material_dark_theme()
    }

    pub fn style(&self, _theme: &Theme) -> iced::daemon::Appearance {
        iced::daemon::Appearance {
            background_color: Color::TRANSPARENT,
            text_color: M3_ON_SURFACE,
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let worker = from_recipe(AppWorkerRecipe(Arc::clone(&self.ui_sync)));
        let close_reqs = iced::window::close_requests().map(Message::WindowCloseRequested);
        if self.is_in_tray {
            return Subscription::batch([worker, close_reqs]);
        }
        let keys = if self.recording_hotkey.is_some() {
            event::listen_with(listen_recording_event)
        } else {
            event::listen_with(listen_normal_event)
        };
        Subscription::batch([worker, keys, close_reqs])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AudioStateChanged => {
                if self.screen == Screen::Settings && self.audio.take_devices_dirty() {
                    self.input_devices = list_input_devices();
                    self.output_devices = list_output_devices();
                }
                Task::none()
            }
            Message::ClearCopiedFlash => {
                if let Some((_, ts)) = self.copied_msg_id {
                    if ts.elapsed() >= Duration::from_secs(2) {
                        self.copied_msg_id = None;
                    }
                }
                Task::none()
            }
            Message::ReleaseMemory => {
                if self.is_in_tray {
                    release_os_memory();
                }
                Task::none()
            }
            Message::TrayEventsPending => match self.tray.poll_action(&self.audio) {
                Some(TrayAction::ShowWindow) => Task::batch(self.restore_from_tray()),
                Some(TrayAction::Quit) => {
                    self.hotkeys.shutdown();
                    let _ = self.mesh_tx.try_send(MeshCommand::Shutdown);
                    self.audio.shutdown();
                    iced::exit()
                }
                Some(TrayAction::ToggleMute) | None => Task::none(),
            },
            Message::VideoFrameReady => {
                if self.watched_peer_id.is_some() || self.local_stream_preview {
                    if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                        if let Some(frame) = slot.take() {
                            let matches_watch =
                                Some(frame.streamer_peer_id) == self.watched_peer_id;
                            let matches_preview = self.local_stream_preview
                                && frame.streamer_peer_id == self.local_peer_id;
                            if matches_watch || matches_preview {
                                self.active_stream_frame = Some(Arc::new(frame));
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::MeshEventsPending => {
                let mut pending_tasks = Vec::new();
                let mut overlay_sync_needed = false;
                let mut events = Vec::new();
                if let Ok(mut rx) = self.mesh_rx.try_lock() {
                    while let Ok(evt) = rx.try_recv() {
                        events.push(evt);
                    }
                }
                for evt in events {
                    match evt {
                        MeshEvent::JoinedRoom {
                            local_peer_id,
                            invite_uri,
                            is_lan_locked,
                        } => {
                            if self.screen != Screen::Room {
                                self.prepare_for_room_entry();
                            }
                            self.local_peer_id = local_peer_id;
                            self.active_invite_uri = invite_uri.clone();
                            self.save_channel_name_input = self
                                .config
                                .saved_channels
                                .iter()
                                .find(|c| c.invite_uri == invite_uri)
                                .map(|c| c.name.clone())
                                .unwrap_or_default();
                            self.short_code_joining = false;
                            self.short_code_input.clear();
                            self.active_short_code = None;
                            self.short_code_copied_flash = false;
                            self.invite_friends_modal_open = false;
                            self.invited_friends_session.clear();
                            self.incoming_call = None;
                            self.lan_password_modal_room = None;
                            self.lan_join_password_input.clear();
                            self.lan_join_password_visible = false;
                            self.lan_join_password_error = None;
                            self.lan_join_in_progress = false;
                            self.active_lan_locked = is_lan_locked;
                            self.screen = Screen::Room;
                            self.status_banner = None;
                            self.audio.set_streams_active(true);
                            self.audio.set_monitoring_in_settings(false);
                            self.hotkeys.set_active(true);
                            self.ui_sync.in_room.store(true, Ordering::Release);
                            self.ui_sync.in_settings.store(false, Ordering::Release);
                            self.ui_sync.audio_ui_notify.notify_one();
                            overlay_sync_needed = true;
                        }
                        MeshEvent::LanAuthFailed(err) => {
                            self.lan_join_in_progress = false;
                            if self.lan_password_modal_room.is_some() {
                                self.lan_join_password_error = Some(err);
                            } else {
                                self.status_banner = Some(err);
                            }
                        }
                        MeshEvent::LeftRoom => {
                            self.peers.clear();
                            self.chat_messages.clear();
                            self.chat_text_editors.clear();
                            self.active_chat_selection = None;
                            self.image_handles.clear();
                            self.inline_image_bytes.clear();
                            self.copied_msg_id = None;
                            self.lightbox_blob_id = None;
                            self.chat_input.clear();
                            self.local_peer_id = 0;
                            self.share_modal_open = false;
                            self.active_short_code = None;
                            self.short_code_copied_flash = false;
                            self.invite_friends_modal_open = false;
                            self.invited_friends_session.clear();
                            self.short_code_joining = false;
                            self.active_lan_locked = false;
                            self.is_local_sharing = false;
                            self.local_stream_preview = false;
                            self.local_stream_subscribers = 0;
                            self.watched_peer_id = None;
                            self.active_stream_frame = None;
                            self.stream_expanded = false;
                            let was_fs = self.stream_fullscreen;
                            self.stream_fullscreen = false;
                            if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                                *slot = None;
                            }
                            self.screen = Screen::Lobby;
                            self.audio.set_streams_active(false);
                            self.audio.set_monitoring_in_settings(false);
                            self.hotkeys.set_active(false);
                            self.ui_sync.in_room.store(false, Ordering::Release);
                            self.ui_sync.in_settings.store(false, Ordering::Release);
                            self.ui_sync.watching_video.store(false, Ordering::Release);
                            self.ui_sync.audio_ui_notify.notify_one();
                            overlay_sync_needed = true;
                            if was_fs {
                                if let Some(id) = self.main_window_id {
                                    pending_tasks.push(set_window_fullscreen_task(id, false));
                                }
                            }
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
                                if !self.is_in_tray
                                    && crate::ui::avatar::decode_bounded_image(
                                        raw,
                                        4096,
                                        64 * 1024 * 1024,
                                    )
                                    .is_ok()
                                {
                                    self.image_handles.insert(
                                        blob_id,
                                        iced_image::Handle::from_bytes(raw.clone()),
                                    );
                                    self.inline_image_bytes
                                        .insert(blob_id, (filename.clone(), raw.clone()));
                                }
                            }
                            let already = self.chat_messages.iter().any(|existing| {
                                existing.sender_uuid == item.sender_uuid
                                    && existing.msg_id == item.msg_id
                            });
                            if !already {
                                if !self.is_in_tray {
                                    if let ChatContentUi::Text(ref body_text) = item.content {
                                        self.chat_text_editors.insert(
                                            (item.sender_uuid, item.msg_id),
                                            text_editor::Content::with_text(body_text),
                                        );
                                    }
                                }
                                self.chat_messages.push(item);
                                self.chat_messages
                                    .sort_by_key(|m| (m.timestamp_ms, m.msg_id));
                                if self.chat_messages.len() > 30 {
                                    let drop_n = self.chat_messages.len() - 30;
                                    let mut dropped_blobs = Vec::new();
                                    for dropped in self.chat_messages.drain(0..drop_n) {
                                        self.chat_text_editors
                                            .remove(&(dropped.sender_uuid, dropped.msg_id));
                                        if let ChatContentUi::InlineImage { blob_id, .. } =
                                            dropped.content
                                        {
                                            dropped_blobs.push(blob_id);
                                        }
                                    }
                                    for blob_id in dropped_blobs {
                                        if self.lightbox_blob_id != Some(blob_id)
                                            && !self.chat_messages.iter().any(|m| {
                                                matches!(
                                                    m.content,
                                                    ChatContentUi::InlineImage { blob_id: bid, .. }
                                                        if bid == blob_id
                                                )
                                            })
                                        {
                                            self.image_handles.remove(&blob_id);
                                            self.inline_image_bytes.remove(&blob_id);
                                        }
                                    }
                                }
                            }
                        }
                        MeshEvent::BlobStateChanged {
                            blob_id,
                            state,
                            inline_bytes,
                        } => {
                            let valid_inline = !self.is_in_tray
                                && inline_bytes.as_deref().is_some_and(|raw| {
                                    crate::ui::avatar::decode_bounded_image(
                                        raw,
                                        4096,
                                        64 * 1024 * 1024,
                                    )
                                    .is_ok()
                                });
                            if valid_inline {
                                if let Some(ref raw) = inline_bytes {
                                    self.image_handles.insert(
                                        blob_id,
                                        iced_image::Handle::from_bytes(raw.clone()),
                                    );
                                }
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
                                            if valid_inline {
                                                self.inline_image_bytes.insert(
                                                    blob_id,
                                                    (filename.clone(), raw.clone()),
                                                );
                                            }
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
                            if let Some(peer) = self.peers.iter_mut().find(|p| p.peer_id == peer_id)
                            {
                                peer.stream_info = stream.clone();
                            }
                            if stream.is_none() && self.watched_peer_id == Some(peer_id) {
                                self.watched_peer_id = None;
                                self.active_stream_frame = None;
                                self.stream_expanded = false;
                                self.ui_sync
                                    .watching_video
                                    .store(self.local_stream_preview, Ordering::Release);
                                if self.stream_fullscreen {
                                    self.stream_fullscreen = false;
                                    if let Some(id) = self.main_window_id {
                                        pending_tasks.push(set_window_fullscreen_task(id, false));
                                    }
                                }
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
                        MeshEvent::StreamCodecFallbackToH264 { reason } => {
                            tracing::info!("Stream codec fell back to H.264: {reason}");
                            self.share_codec = VideoCodec::H264;
                            self.status_banner = Some(
                                "Switched stream to H.264 for viewer compatibility".to_string(),
                            );
                        }
                        MeshEvent::LanRoomsUpdated(rooms) => {
                            self.lan_rooms = rooms;
                        }
                        MeshEvent::FriendsStatusUpdated(list) => {
                            self.friend_presences = list
                                .into_iter()
                                .filter(|f| f.online)
                                .map(|f| (f.pub_key, f))
                                .collect();
                        }
                        MeshEvent::IncomingFriendCall(call) => {
                            let already_in_same_room = self.screen == Screen::Room
                                && InviteLink::parse(&self.active_invite_uri)
                                    .is_ok_and(|inv| inv.room_id == call.invite.room_id);
                            if !already_in_same_room {
                                self.incoming_call = Some(call);
                                if self.is_in_tray {
                                    pending_tasks.extend(self.restore_from_tray());
                                } else if let Some(id) = self.main_window_id {
                                    pending_tasks.push(iced::window::gain_focus(id));
                                }
                            }
                        }
                        MeshEvent::ShortCodeGenerated {
                            code,
                            expires_in_secs,
                        } => {
                            self.active_short_code = Some((
                                code,
                                std::time::Instant::now()
                                    + Duration::from_secs(expires_in_secs as u64),
                            ));
                            self.short_code_copied_flash = false;
                            self.status_banner = None;
                        }
                        MeshEvent::ShortCodeCleared => {
                            self.active_short_code = None;
                            self.short_code_copied_flash = false;
                        }
                        MeshEvent::FriendInviteSent { target_pub_key } => {
                            self.invited_friends_session.insert(target_pub_key);
                        }
                        MeshEvent::Error(err) => {
                            self.short_code_joining = false;
                            self.status_banner = Some(err);
                        }
                    }
                }
                if overlay_sync_needed {
                    pending_tasks.push(self.sync_overlay_window());
                }
                if pending_tasks.is_empty() {
                    Task::none()
                } else {
                    Task::batch(pending_tasks)
                }
            }
            Message::CreateRoom => {
                let invite = InviteLink::generate();
                let uri = invite.to_uri();
                self.active_invite_uri = uri.clone();
                self.invite_copied_flash = true;
                let join_task = self.join_with_invite(
                    invite,
                    self.config.rendezvous_server.clone(),
                    false,
                    None,
                );
                Task::batch([join_task, clipboard::write(uri)])
            }
            Message::LanRoomNameChanged(val) => {
                self.lan_room_name_input = val;
                Task::none()
            }
            Message::HostLanRoom => {
                let room_name = self.lan_room_name_input.trim().to_string();
                let raw_pw = self.lan_room_password_input.trim().to_string();
                if raw_pw.len() > squad_proto::MAX_LAN_PASSWORD_LEN {
                    self.status_banner = Some(format!(
                        "LAN room password exceeds maximum length of {} bytes",
                        squad_proto::MAX_LAN_PASSWORD_LEN
                    ));
                    return Task::none();
                }
                let password = if raw_pw.is_empty() {
                    None
                } else {
                    Some(raw_pw)
                };
                self.lan_room_password_input.clear();
                self.lan_host_password_visible = false;
                self.prepare_for_room_entry();
                self.active_lan_locked = password.is_some();
                let _ = self.mesh_tx.try_send(MeshCommand::HostLanRoom {
                    room_name,
                    password,
                    nickname: self.config.nickname.clone(),
                    client_uuid: self.config.client_uuid(),
                });
                self.enter_room_ui_state()
            }
            Message::JoinLanRoom(room) => {
                if let Some(secret) = room.secret {
                    let invite = InviteLink {
                        room_id: room.room_id,
                        secret,
                    };
                    self.active_invite_uri = invite.to_uri();
                    self.invite_copied_flash = false;
                    self.active_lan_locked = false;
                    self.join_with_invite(
                        invite,
                        room.host_addr.to_string(),
                        true,
                        Some(room.room_name),
                    )
                } else {
                    self.lan_password_modal_room = Some(room);
                    self.lan_join_password_input.clear();
                    self.lan_join_password_visible = false;
                    self.lan_join_password_error = None;
                    self.lan_join_in_progress = false;
                    Task::none()
                }
            }
            Message::InviteInputChanged(val) => {
                self.invite_input = val;
                Task::none()
            }
            Message::PasteInviteFromClipboard => Task::perform(
                async move {
                    tokio::task::spawn_blocking(read_invite_clipboard_text)
                        .await
                        .ok()
                        .flatten()
                },
                Message::InviteClipboardLoaded,
            ),
            Message::InviteClipboardLoaded(raw) => {
                if let Some(text) = raw.as_deref().and_then(sanitize_invite_clipboard_text) {
                    self.invite_input = text;
                    self.status_banner = None;
                } else {
                    self.status_banner = Some("Clipboard has no text to paste".to_string());
                }
                Task::none()
            }
            Message::JoinRoomWithInput => {
                let uri = self.invite_input.clone();
                self.try_join_uri(&uri)
            }
            Message::JoinSavedChannel(uri) => self.try_join_uri(&uri),
            Message::LeaveRoom => {
                let _ = self.mesh_tx.try_send(MeshCommand::LeaveRoom);
                self.peers.clear();
                self.chat_messages.clear();
                self.chat_text_editors.clear();
                self.active_chat_selection = None;
                self.image_handles.clear();
                self.inline_image_bytes.clear();
                self.copied_msg_id = None;
                self.lightbox_blob_id = None;
                self.chat_input.clear();
                self.share_modal_open = false;
                self.active_short_code = None;
                self.short_code_copied_flash = false;
                self.invite_friends_modal_open = false;
                self.invited_friends_session.clear();
                self.short_code_joining = false;
                self.active_lan_locked = false;
                self.is_local_sharing = false;
                self.local_stream_preview = false;
                self.local_stream_subscribers = 0;
                self.watched_peer_id = None;
                self.active_stream_frame = None;
                self.stream_expanded = false;
                let was_fs = self.stream_fullscreen;
                self.stream_fullscreen = false;
                if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                    *slot = None;
                }
                self.screen = Screen::Lobby;
                self.audio.set_streams_active(false);
                self.audio.set_monitoring_in_settings(false);
                self.hotkeys.set_active(false);
                self.ui_sync.in_room.store(false, Ordering::Release);
                self.ui_sync.in_settings.store(false, Ordering::Release);
                self.ui_sync.watching_video.store(false, Ordering::Release);
                self.ui_sync.audio_ui_notify.notify_one();
                let overlay_task = self.sync_overlay_window();
                if was_fs {
                    if let Some(id) = self.main_window_id {
                        Task::batch([overlay_task, set_window_fullscreen_task(id, false)])
                    } else {
                        overlay_task
                    }
                } else {
                    overlay_task
                }
            }
            Message::CopyInviteUri => {
                self.invite_copied_flash = true;
                clipboard::write(self.active_invite_uri.clone())
            }
            Message::OpenSettings => {
                if self.screen != Screen::Settings {
                    self.previous_screen = self.screen;
                    self.screen = Screen::Settings;
                    self.audio.set_streams_active(true);
                    self.audio.set_monitoring_in_settings(true);
                    if self.audio.take_devices_dirty() {
                        self.input_devices = list_input_devices();
                        self.output_devices = list_output_devices();
                    }
                    self.ui_sync.in_settings.store(true, Ordering::Release);
                    self.ui_sync.audio_ui_notify.notify_one();
                }
                Task::none()
            }
            Message::CloseSettings => {
                self.recording_hotkey = None;
                self.audio.set_mic_loopback(false);
                let _ = self.config.save();
                self.rebind_hotkeys();
                self.sync_presence_to_mesh();
                self.screen = self.previous_screen;
                self.audio.set_monitoring_in_settings(false);
                if self.screen != Screen::Room {
                    self.audio.set_streams_active(false);
                }
                self.ui_sync.in_settings.store(false, Ordering::Release);
                self.ui_sync.audio_ui_notify.notify_one();
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
                self.audio.play_cue(if next {
                    SoundCue::Mute
                } else {
                    SoundCue::Unmute
                });
                Task::none()
            }
            Message::ToggleMicLoopback => {
                let next = !self.audio.is_mic_loopback();
                self.audio.set_mic_loopback(next);
                self.ui_sync.audio_ui_notify.notify_one();
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
            Message::SetJitterProfile(profile) => {
                self.config.jitter_profile = profile;
                self.audio.set_jitter_profile(profile);
                let _ = self.config.save();
                Task::none()
            }
            Message::SetOpusBitrate(bps) => {
                self.config.opus_bitrate_bps = bps;
                self.audio.set_opus_bitrate(bps);
                let _ = self.config.save();
                Task::none()
            }
            Message::InputGainChanged(gain) => {
                self.config.input_gain = gain;
                self.audio.set_input_gain(gain);
                Task::none()
            }
            Message::CueVolumeChanged(gain) => {
                let clamped = if gain.is_finite() {
                    gain.clamp(0.0, 2.0)
                } else {
                    1.0
                };
                self.config.cue_volume = clamped;
                self.audio.set_cue_gain(clamped);
                Task::none()
            }
            Message::CueVolumeReleased => {
                let _ = self.config.save();
                Task::none()
            }
            Message::TestCueSound => {
                self.audio.play_cue(SoundCue::Join);
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
            Message::RendezvousPresetSelected(choice) => {
                if choice == CUSTOM_RENDEZVOUS_OPTION {
                    self.rendezvous_custom_mode = true;
                    let custom_trimmed = self.config.custom_rendezvous_server.trim();
                    if !custom_trimmed.is_empty() {
                        self.config.rendezvous_server = custom_trimmed.to_string();
                        let _ = self.config.save();
                        self.sync_presence_to_mesh();
                    }
                } else {
                    self.rendezvous_custom_mode = false;
                    self.config.rendezvous_server = choice;
                    let _ = self.config.save();
                    self.sync_presence_to_mesh();
                }
                Task::none()
            }
            Message::RendezvousServerChanged(srv) => {
                self.rendezvous_custom_mode = true;
                self.config.custom_rendezvous_server = srv.clone();
                let trimmed = srv.trim();
                self.config.rendezvous_server = if trimmed.is_empty() {
                    DEFAULT_RENDEZVOUS_SERVER.to_string()
                } else {
                    trimmed.to_string()
                };
                Task::none()
            }
            Message::StartRecordingHotkey(slot) => {
                self.recording_hotkey = if self.recording_hotkey == Some(slot) {
                    None
                } else {
                    Some(slot)
                };
                Task::none()
            }
            Message::CancelRecordingHotkey => {
                self.recording_hotkey = None;
                Task::none()
            }
            Message::RecordedHotkeyInput(key_name) => {
                if let Some(slot) = self.recording_hotkey.take() {
                    self.set_hotkey_slot(slot, key_name);
                    let _ = self.config.save();
                    self.rebind_hotkeys();
                }
                Task::none()
            }
            Message::ClearHotkey(slot) => {
                if self.recording_hotkey == Some(slot) {
                    self.recording_hotkey = None;
                }
                self.set_hotkey_slot(slot, String::new());
                let _ = self.config.save();
                self.rebind_hotkeys();
                Task::none()
            }
            Message::RefreshAudioDevices => {
                let _ = self.audio.take_devices_dirty();
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
            Message::PickFileToSend => {
                if self.has_relayed_peer() {
                    self.status_banner = Some("Disabled on Relay — Voice & Chat only".to_string());
                    return Task::none();
                }
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_file()
                            .await
                            .map(|handle| handle.path().to_path_buf())
                    },
                    Message::FilePicked,
                )
            }
            Message::FilePicked(Some(path)) => {
                if self.has_relayed_peer() {
                    self.status_banner = Some("Disabled on Relay — Voice & Chat only".to_string());
                    return Task::none();
                }
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
                            if self.has_relayed_peer() {
                                self.status_banner =
                                    Some("Disabled on Relay — Voice & Chat only".to_string());
                            } else {
                                let _ = self
                                    .mesh_tx
                                    .try_send(MeshCommand::SendInlineImage { filename, bytes });
                            }
                        }
                    }
                    ClipboardPayload::TempFile(path) => {
                        if self.screen == Screen::Room {
                            if self.has_relayed_peer() {
                                self.status_banner =
                                    Some("Disabled on Relay — Voice & Chat only".to_string());
                            } else {
                                let _ = self.mesh_tx.try_send(MeshCommand::OfferFile { path });
                            }
                        }
                    }
                    ClipboardPayload::InviteUri(uri) => {
                        if self.screen == Screen::Lobby {
                            self.invite_input = uri;
                        }
                    }
                    ClipboardPayload::PlainText(txt) => {
                        let single_line = txt.replace(['\r', '\n'], " ");
                        if self.screen == Screen::Lobby {
                            self.invite_input.push_str(single_line.trim());
                        } else if self.screen == Screen::Room {
                            self.chat_input.push_str(&single_line);
                        }
                    }
                }
                Task::none()
            }
            Message::ClipboardProcessed(None) => Task::none(),
            Message::CopyChatText(msg_id, text) => {
                self.copied_msg_id = Some((msg_id, std::time::Instant::now()));
                Task::batch([
                    clipboard::write(text),
                    Task::perform(
                        async {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        },
                        |_| Message::ClearCopiedFlash,
                    ),
                ])
            }
            Message::ChatEditorAction(key, action) => {
                if !action.is_edit() {
                    let is_scroll = matches!(action, text_editor::Action::Scroll { .. });
                    if let Some(editor) = self.chat_text_editors.get_mut(&key) {
                        editor.perform(action);
                        if !is_scroll {
                            self.active_chat_selection = editor.selection();
                        }
                    }
                }
                Task::none()
            }
            Message::CopySelectedChatText => {
                if let Some(sel) = self.active_chat_selection.clone().filter(|s| !s.is_empty()) {
                    clipboard::write(sel)
                } else {
                    Task::none()
                }
            }
            Message::CopyImageToClipboard(blob_id) => {
                if let Some((_, raw)) = self.inline_image_bytes.get(&blob_id).cloned() {
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                copy_encoded_image_to_clipboard(&raw)
                            })
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
                    let safe_name =
                        squad_proto::sanitize_display_str(&filename, 128).replace(['/', '\\'], "_");
                    let safe_name = if safe_name.is_empty() || safe_name == "." || safe_name == ".."
                    {
                        "image.png".to_string()
                    } else {
                        safe_name
                    };
                    return Task::perform(
                        async move {
                            if let Some(handle) = rfd::AsyncFileDialog::new()
                                .set_file_name(&safe_name)
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
                self.add_friend_modal_open = false;
                self.invite_friends_modal_open = false;
                self.clear_lan_password_modal(true);
                #[cfg(not(feature = "managed_updates"))]
                if !self.update_in_progress {
                    self.update_modal_open = false;
                }
                if self.stream_fullscreen {
                    self.stream_fullscreen = false;
                    if let Some(id) = self.main_window_id {
                        return set_window_fullscreen_task(id, false);
                    }
                }
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
                if self.has_relayed_peer() {
                    self.status_banner = Some("Disabled on Relay — Voice & Chat only".to_string());
                    return Task::none();
                }
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
            Message::SetShareCodec(codec) => {
                self.share_codec = codec;
                Task::none()
            }
            Message::ToggleShareAudio => {
                self.share_audio = !self.share_audio;
                Task::none()
            }
            Message::StartScreenShare => {
                if self.has_relayed_peer() {
                    self.share_modal_open = false;
                    self.status_banner = Some("Disabled on Relay — Voice & Chat only".to_string());
                    return Task::none();
                }
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
                        codec: self.share_codec,
                        share_audio: self.share_audio,
                    };
                    let _ = self
                        .mesh_tx
                        .try_send(MeshCommand::StartScreenShare { config });
                    self.is_local_sharing = true;
                    self.local_stream_preview = false;
                    self.share_modal_open = false;
                }
                Task::none()
            }
            Message::StopScreenShare => {
                let _ = self.mesh_tx.try_send(MeshCommand::StopScreenShare);
                self.is_local_sharing = false;
                self.local_stream_preview = false;
                self.local_stream_subscribers = 0;
                self.ui_sync
                    .watching_video
                    .store(self.watched_peer_id.is_some(), Ordering::Release);
                if self.watched_peer_id.is_none() {
                    self.active_stream_frame = None;
                    if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                        *slot = None;
                    }
                    if self.stream_fullscreen {
                        self.stream_fullscreen = false;
                        if let Some(id) = self.main_window_id {
                            return set_window_fullscreen_task(id, false);
                        }
                    }
                }
                Task::none()
            }
            Message::ToggleLocalStreamPreview => {
                if self.is_local_sharing {
                    self.local_stream_preview = !self.local_stream_preview;
                    if self.local_stream_preview {
                        self.watched_peer_id = None;
                        self.active_stream_frame = None;
                        self.stream_expanded = false;
                    } else if self.watched_peer_id.is_none() {
                        self.active_stream_frame = None;
                        if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                            *slot = None;
                        }
                    }
                    self.ui_sync.watching_video.store(
                        self.watched_peer_id.is_some() || self.local_stream_preview,
                        Ordering::Release,
                    );
                    if self.local_stream_preview {
                        self.ui_sync.video_notify.notify_one();
                    }
                    let _ = self.mesh_tx.try_send(MeshCommand::SetLocalStreamPreview {
                        enabled: self.local_stream_preview,
                    });
                    if !self.local_stream_preview
                        && self.stream_fullscreen
                        && self.watched_peer_id.is_none()
                    {
                        self.stream_fullscreen = false;
                        if let Some(id) = self.main_window_id {
                            return set_window_fullscreen_task(id, false);
                        }
                    }
                }
                Task::none()
            }
            Message::WatchPeerStream(peer_id) => {
                self.local_stream_preview = false;
                self.watched_peer_id = Some(peer_id);
                self.active_stream_frame = None;
                self.stream_expanded = false;
                self.ui_sync.watching_video.store(true, Ordering::Release);
                self.ui_sync.video_notify.notify_one();
                if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                    *slot = None;
                }
                let _ = self.mesh_tx.try_send(MeshCommand::WatchPeerStream {
                    peer_id: Some(peer_id),
                });
                Task::none()
            }
            Message::StopWatchingStream => {
                self.watched_peer_id = None;
                self.active_stream_frame = None;
                self.stream_expanded = false;
                self.ui_sync
                    .watching_video
                    .store(self.local_stream_preview, Ordering::Release);
                let was_fs = self.stream_fullscreen;
                self.stream_fullscreen = false;
                if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                    *slot = None;
                }
                let _ = self
                    .mesh_tx
                    .try_send(MeshCommand::WatchPeerStream { peer_id: None });
                if was_fs {
                    if let Some(id) = self.main_window_id {
                        return set_window_fullscreen_task(id, false);
                    }
                }
                Task::none()
            }
            Message::StreamVolumeChanged(gain) => {
                let clamped = gain.clamp(0.0, 2.0);
                self.audio.set_stream_gain(clamped);
                self.config.stream_volume = clamped;
                let _ = self.config.save();
                Task::none()
            }
            Message::ToggleStreamExpanded => {
                self.stream_expanded = !self.stream_expanded;
                Task::none()
            }
            Message::ToggleStreamFullscreen => {
                if self.screen == Screen::Room
                    && (self.watched_peer_id.is_some() || self.local_stream_preview)
                {
                    self.stream_fullscreen = !self.stream_fullscreen;
                    if let Some(id) = self.main_window_id {
                        return set_window_fullscreen_task(id, self.stream_fullscreen);
                    }
                }
                Task::none()
            }
            Message::ExitStreamFullscreen => {
                if self.stream_fullscreen {
                    self.stream_fullscreen = false;
                    if let Some(id) = self.main_window_id {
                        return set_window_fullscreen_task(id, false);
                    }
                }
                Task::none()
            }
            Message::WindowCloseRequested(window_id) => {
                if Some(window_id) != self.main_window_id {
                    return Task::none();
                }
                if self.config.minimize_to_tray && self.tray.is_available() {
                    self.is_in_tray = true;
                    self.main_window_id = None;
                    self.share_modal_open = false;
                    self.add_friend_modal_open = false;
                    self.invite_friends_modal_open = false;
                    self.clear_lan_password_modal(true);
                    self.lightbox_blob_id = None;
                    self.watched_peer_id = None;
                    self.local_stream_preview = false;
                    self.active_stream_frame = None;
                    self.stream_expanded = false;
                    self.stream_fullscreen = false;
                    self.image_handles.clear();
                    self.image_handles.shrink_to_fit();
                    self.inline_image_bytes.clear();
                    self.inline_image_bytes.shrink_to_fit();
                    self.capture_targets.clear();
                    self.capture_targets.shrink_to_fit();
                    self.chat_text_editors.clear();
                    self.chat_text_editors.shrink_to_fit();
                    self.active_chat_selection = None;
                    if self.overlay_window_id.is_none() {
                        self.avatars.clear_memory_cache();
                    }
                    if let Ok(mut slot) = self.video_frame_slot.try_lock() {
                        *slot = None;
                    }
                    let in_room = self.screen == Screen::Room
                        || (self.screen == Screen::Settings
                            && self.previous_screen == Screen::Room);
                    self.recording_hotkey = None;
                    self.audio.set_mic_loopback(false);
                    self.audio.set_monitoring_in_settings(false);
                    if !in_room {
                        self.audio.set_streams_active(false);
                    }
                    self.ui_sync.in_settings.store(false, Ordering::Release);
                    self.ui_sync.in_tray.store(true, Ordering::Release);
                    self.ui_sync.watching_video.store(false, Ordering::Release);
                    self.ui_sync.audio_ui_notify.notify_one();
                    let _ = self
                        .mesh_tx
                        .try_send(MeshCommand::SetTrayMode { in_tray: true });
                    Task::batch([
                        iced::window::close(window_id),
                        Task::perform(
                            async {
                                tokio::time::sleep(Duration::from_millis(120)).await;
                            },
                            |_| Message::ReleaseMemory,
                        ),
                    ])
                } else {
                    self.hotkeys.shutdown();
                    let _ = self.mesh_tx.try_send(MeshCommand::Shutdown);
                    self.audio.shutdown();
                    iced::exit()
                }
            }
            Message::SetMinimizeToTray(enabled) => {
                self.config.minimize_to_tray = enabled;
                let _ = self.config.save();
                Task::none()
            }
            Message::SetOverlayEnabled(enabled) => {
                self.config.overlay_enabled = enabled;
                let _ = self.config.save();
                self.sync_overlay_window()
            }
            Message::SetOverlaySpeakingOnly(speaking_only) => {
                self.config.overlay_speaking_only = speaking_only;
                let _ = self.config.save();
                Task::none()
            }
            Message::OverlayOpacityChanged(opacity) => {
                self.config.overlay_opacity = opacity.clamp(0.30, 1.0);
                let _ = self.config.save();
                if let Some(ov_id) = self.overlay_window_id {
                    overlay_platform::set_overlay_opacity(ov_id, self.config.overlay_opacity)
                } else {
                    Task::none()
                }
            }
            Message::WindowOpened(id) => {
                if self.main_window_id == Some(id) {
                    iced::window::gain_focus(id)
                } else {
                    Task::none()
                }
            }
            Message::OverlayConfigured => Task::none(),
            Message::OverlayWindowOpened(id) => {
                if self.overlay_window_id == Some(id) {
                    overlay_platform::configure_overlay_window(id, self.config.overlay_opacity)
                } else {
                    Task::none()
                }
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
            #[cfg(not(feature = "managed_updates"))]
            Message::CheckForUpdatesManual => {
                self.update_manual_check = true;
                self.update_status_text = Some("Checking GitHub Releases...".to_string());
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(crate::updater::check_latest_release)
                            .await
                            .map_err(|e| format!("Update check failed: {e}"))
                            .and_then(|res| res)
                    },
                    Message::UpdateCheckCompleted,
                )
            }
            #[cfg(not(feature = "managed_updates"))]
            Message::UpdateCheckCompleted(result) => {
                let is_manual = self.update_manual_check;
                self.update_manual_check = false;
                match result {
                    Ok(Some(info)) => {
                        let skipped = self.config.skipped_update_version.as_deref()
                            == Some(info.tag_name.as_str());
                        self.available_update = Some(info);
                        self.update_status_text = None;
                        if is_manual || !skipped {
                            self.update_modal_open = true;
                        }
                    }
                    Ok(None) => {
                        self.available_update = None;
                        if is_manual {
                            self.update_status_text = Some(format!(
                                "You are up to date (v{}).",
                                crate::current_app_version()
                            ));
                        }
                    }
                    Err(err) => {
                        if is_manual {
                            self.update_status_text = Some(err);
                        }
                    }
                }
                Task::none()
            }
            #[cfg(not(feature = "managed_updates"))]
            Message::OpenUpdateModal => {
                if self.available_update.is_some() {
                    self.update_modal_open = true;
                    if !self.update_in_progress {
                        self.update_status_text = None;
                    }
                }
                Task::none()
            }
            #[cfg(not(feature = "managed_updates"))]
            Message::DismissUpdateModal => {
                if !self.update_in_progress {
                    self.update_modal_open = false;
                }
                Task::none()
            }
            #[cfg(not(feature = "managed_updates"))]
            Message::SkipUpdateVersion => {
                if !self.update_in_progress {
                    if let Some(ref info) = self.available_update {
                        self.config.skipped_update_version = Some(info.tag_name.clone());
                        let _ = self.config.save();
                    }
                    self.update_modal_open = false;
                }
                Task::none()
            }
            #[cfg(not(feature = "managed_updates"))]
            Message::StartOneClickUpdate => {
                if !self.update_in_progress {
                    if let Some(info) = self.available_update.clone() {
                        self.update_in_progress = true;
                        self.update_status_text =
                            Some("Downloading installer & verifying SHA-256...".to_string());
                        return Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    crate::updater::download_verify_and_launch(info)
                                })
                                .await
                                .map_err(|e| format!("Update installer task failed: {e}"))
                                .and_then(|res| res)
                            },
                            Message::UpdateInstallFinished,
                        );
                    }
                }
                Task::none()
            }
            #[cfg(not(feature = "managed_updates"))]
            Message::UpdateInstallFinished(result) => {
                self.update_in_progress = false;
                match result {
                    Ok(msg) => {
                        self.update_status_text = Some(msg.clone());
                        self.status_banner = Some(msg);
                    }
                    Err(err) => {
                        self.update_status_text = Some(err.clone());
                        self.status_banner = Some(err);
                    }
                }
                Task::none()
            }
            Message::CopyMyFriendId => {
                self.friend_id_copied_flash = true;
                let code =
                    format_friend_code(&self.config.identity_pub_key(), &self.config.nickname);
                clipboard::write(code)
            }
            Message::OpenAddFriendModal => {
                self.add_friend_modal_open = true;
                self.add_friend_error = None;
                self.friend_id_copied_flash = false;
                Task::none()
            }
            Message::CloseAddFriendModal => {
                self.add_friend_modal_open = false;
                self.add_friend_error = None;
                Task::none()
            }
            Message::AddFriendCodeChanged(val) => {
                self.add_friend_code_input = val;
                self.add_friend_error = None;
                Task::none()
            }
            Message::AddFriendNickChanged(val) => {
                self.add_friend_nick_input = val;
                Task::none()
            }
            Message::ConfirmAddFriend => {
                match parse_friend_code(&self.add_friend_code_input) {
                    Ok((pub_key, code_nick)) => {
                        if pub_key == self.config.identity_pub_key() {
                            self.add_friend_error =
                                Some("Cannot add your own Friend ID".to_string());
                            return Task::none();
                        }
                        let explicit = squad_proto::sanitize_display_str(
                            self.add_friend_nick_input.trim(),
                            squad_proto::MAX_NICKNAME_LEN,
                        );
                        let from_code = code_nick
                            .map(|n| {
                                squad_proto::sanitize_display_str(
                                    n.trim(),
                                    squad_proto::MAX_NICKNAME_LEN,
                                )
                            })
                            .unwrap_or_default();
                        let final_nick = if !explicit.is_empty() {
                            explicit
                        } else if !from_code.is_empty() {
                            from_code
                        } else {
                            "Friend".to_string()
                        };
                        if let Some(existing) =
                            self.friends.iter_mut().find(|f| f.pub_key == pub_key)
                        {
                            existing.nickname = final_nick;
                        } else {
                            let now_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            self.friends.push(FriendContact {
                                pub_key,
                                nickname: final_nick,
                                added_at_ms: now_ms,
                            });
                        }
                        save_friends(&self.friends);
                        self.sync_presence_to_mesh();
                        self.add_friend_modal_open = false;
                        self.add_friend_code_input.clear();
                        self.add_friend_nick_input.clear();
                        self.add_friend_error = None;
                        self.status_banner = None;
                    }
                    Err(err) => {
                        self.add_friend_error = Some(err);
                    }
                }
                Task::none()
            }
            Message::RemoveFriend(pub_key) => {
                self.friends.retain(|f| f.pub_key != pub_key);
                self.friend_presences.remove(&pub_key);
                save_friends(&self.friends);
                self.sync_presence_to_mesh();
                Task::none()
            }
            Message::CallFriend(pub_key) => {
                self.prepare_for_room_entry();
                let _ = self.mesh_tx.try_send(MeshCommand::CallOrInviteFriend {
                    target_pub_key: pub_key,
                });
                self.status_banner = Some("Calling friend...".to_string());
                Task::none()
            }
            Message::OpenInviteFriendsModal => {
                if self.screen == Screen::Room {
                    self.invite_friends_modal_open = true;
                }
                Task::none()
            }
            Message::CloseInviteFriendsModal => {
                self.invite_friends_modal_open = false;
                Task::none()
            }
            Message::InviteFriendToParty(pub_key) => {
                if self.is_friend_in_current_room(&pub_key) {
                    return Task::none();
                }
                let _ = self.mesh_tx.try_send(MeshCommand::CallOrInviteFriend {
                    target_pub_key: pub_key,
                });
                Task::none()
            }
            Message::AcceptIncomingCall => {
                if let Some(call) = self.incoming_call.take() {
                    if !self
                        .friends
                        .iter()
                        .any(|f| f.pub_key == call.sender_pub_key)
                    {
                        let now_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let nick = squad_proto::sanitize_display_str(
                            call.sender_nickname.trim(),
                            squad_proto::MAX_NICKNAME_LEN,
                        );
                        let final_nick = if nick.is_empty() {
                            "Friend".to_string()
                        } else {
                            nick
                        };
                        self.friends.push(FriendContact {
                            pub_key: call.sender_pub_key,
                            nickname: final_nick,
                            added_at_ms: now_ms,
                        });
                        save_friends(&self.friends);
                        self.sync_presence_to_mesh();
                    }
                    let invite = call.invite;
                    self.active_invite_uri = invite.to_uri();
                    self.invite_copied_flash = false;
                    return self.join_with_invite(
                        invite,
                        self.config.rendezvous_server.clone(),
                        false,
                        None,
                    );
                }
                Task::none()
            }
            Message::DeclineIncomingCall => {
                self.incoming_call = None;
                Task::none()
            }
            Message::ShortCodeInputChanged(val) => {
                self.short_code_input = val;
                Task::none()
            }
            Message::JoinByShortCode => {
                if self.short_code_joining {
                    return Task::none();
                }
                match parse_short_code_input(&self.short_code_input) {
                    Ok(code) => {
                        self.short_code_joining = true;
                        self.status_banner = None;
                        let _ = self.mesh_tx.try_send(MeshCommand::JoinByShortCode {
                            rendezvous_server: self.config.rendezvous_server.clone(),
                            code,
                            nickname: self.config.nickname.clone(),
                            client_uuid: self.config.client_uuid(),
                        });
                    }
                    Err(err) => {
                        self.status_banner = Some(err);
                    }
                }
                Task::none()
            }
            Message::GenerateShortCode => {
                self.short_code_copied_flash = false;
                let _ = self.mesh_tx.try_send(MeshCommand::GenerateShortCode);
                Task::none()
            }
            Message::CopyShortCode => {
                if let Some((code, _)) = self.active_short_code {
                    self.short_code_copied_flash = true;
                    return clipboard::write(format_short_code(code));
                }
                Task::none()
            }
            Message::LanRoomPasswordChanged(val) => {
                self.lan_room_password_input = val;
                Task::none()
            }
            Message::ToggleLanHostPasswordVisible => {
                self.lan_host_password_visible = !self.lan_host_password_visible;
                Task::none()
            }
            Message::LanJoinPasswordChanged(val) => {
                self.lan_join_password_input = val;
                self.lan_join_password_error = None;
                Task::none()
            }
            Message::ToggleLanJoinPasswordVisible => {
                self.lan_join_password_visible = !self.lan_join_password_visible;
                Task::none()
            }
            Message::ConfirmJoinLockedLanRoom => {
                if self.lan_join_in_progress {
                    return Task::none();
                }
                if let Some(room) = self.lan_password_modal_room.clone() {
                    let clean_pw = self.lan_join_password_input.trim().to_string();
                    if clean_pw.is_empty() {
                        self.lan_join_password_error = Some("Enter room password".to_string());
                        return Task::none();
                    }
                    if clean_pw.len() > squad_proto::MAX_LAN_PASSWORD_LEN {
                        self.lan_join_password_error = Some(format!(
                            "Password exceeds maximum length of {} bytes",
                            squad_proto::MAX_LAN_PASSWORD_LEN
                        ));
                        return Task::none();
                    }
                    self.lan_join_in_progress = true;
                    self.lan_join_password_error = None;
                    let _ = self.mesh_tx.try_send(MeshCommand::JoinLockedLanRoom {
                        room,
                        password: clean_pw,
                        nickname: self.config.nickname.clone(),
                        client_uuid: self.config.client_uuid(),
                    });
                }
                Task::none()
            }
            Message::CloseLanPasswordModal => {
                self.clear_lan_password_modal(true);
                Task::none()
            }
        }
    }

    pub fn view(&self, window_id: iced::window::Id) -> Element<'_, Message> {
        if Some(window_id) == self.overlay_window_id {
            return overlay::view_overlay(self);
        }
        if Some(window_id) != self.main_window_id {
            return container(text("")).into();
        }

        let content = match self.screen {
            Screen::Lobby => lobby::view_lobby(self),
            Screen::Room => room::view_room(self),
            Screen::Settings => settings::view_settings(self),
        };

        let base = container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(root_container_style);

        let root: Element<'_, Message> = if let Some(ref call) = self.incoming_call {
            stack![base, incoming_call::view_incoming_call_modal(self, call)].into()
        } else if let Some(blob_id) = self.lightbox_blob_id {
            if let Some(handle) = self.image_handles.get(&blob_id) {
                let title = self
                    .inline_image_bytes
                    .get(&blob_id)
                    .map(|(name, _)| name.as_str())
                    .unwrap_or("Image Preview");
                stack![base, view_image_lightbox(blob_id, title, handle.clone())].into()
            } else {
                base.into()
            }
        } else if let Some(ref lan_room) = self.lan_password_modal_room {
            stack![base, incoming_call::view_lan_password_modal(self, lan_room)].into()
        } else if self.add_friend_modal_open {
            stack![base, incoming_call::view_add_friend_modal(self)].into()
        } else if self.invite_friends_modal_open {
            stack![base, incoming_call::view_invite_friends_modal(self)].into()
        } else if self.share_modal_open {
            stack![base, share_modal::view_share_modal(self)].into()
        } else {
            #[cfg(not(feature = "managed_updates"))]
            if self.update_modal_open {
                if let Some(ref info) = self.available_update {
                    let modal: Element<'_, Message> = stack![
                        base,
                        view_update_modal(
                            info,
                            self.update_in_progress,
                            self.update_status_text.as_deref()
                        )
                    ]
                    .into();
                    return shortcut_normalizer(modal);
                }
            }
            base.into()
        };

        shortcut_normalizer(root)
    }

    fn rebind_hotkeys(&self) {
        self.hotkeys.rebind(
            &self.config.hotkey_toggle_mute,
            &self.config.hotkey_ptt,
            &self.config.hotkey_ptm,
            &self.config.hotkey_deafen,
        );
    }

    fn set_hotkey_slot(&mut self, slot: HotkeySlot, value: String) {
        match slot {
            HotkeySlot::Ptm => self.config.hotkey_ptm = value,
            HotkeySlot::Ptt => self.config.hotkey_ptt = value,
            HotkeySlot::ToggleMute => self.config.hotkey_toggle_mute = value,
            HotkeySlot::Deafen => self.config.hotkey_deafen = value,
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
            if let Some((idx, _)) =
                self.capture_targets
                    .iter()
                    .enumerate()
                    .find(|(_, t)| match self.share_tab {
                        ShareSourceTab::Monitors => t.is_monitor(),
                        ShareSourceTab::Windows => !t.is_monitor(),
                    })
            {
                self.selected_capture_target_idx = idx;
            }
        }
    }

    fn try_join_uri(&mut self, uri: &str) -> Task<Message> {
        match InviteLink::parse(uri) {
            Ok(invite) => {
                self.active_invite_uri = invite.to_uri();
                self.invite_copied_flash = false;
                self.join_with_invite(invite, self.config.rendezvous_server.clone(), false, None)
            }
            Err(e) => {
                self.status_banner = Some(format!("Invalid invite link: {e}"));
                Task::none()
            }
        }
    }

    fn clear_lan_password_modal(&mut self, cancel_auth: bool) {
        let had_modal = self.lan_password_modal_room.take().is_some();
        self.lan_join_password_input.clear();
        self.lan_join_password_visible = false;
        self.lan_join_password_error = None;
        self.lan_join_in_progress = false;
        if cancel_auth && had_modal {
            let _ = self.mesh_tx.try_send(MeshCommand::CancelLanAuth);
        }
    }

    fn prepare_for_room_entry(&mut self) {
        self.chat_messages.clear();
        self.chat_text_editors.clear();
        self.active_chat_selection = None;
        self.image_handles.clear();
        self.inline_image_bytes.clear();
        self.copied_msg_id = None;
        self.lightbox_blob_id = None;
        self.chat_input.clear();
        self.active_short_code = None;
        self.short_code_copied_flash = false;
        self.invite_friends_modal_open = false;
        self.invited_friends_session.clear();
        self.short_code_joining = false;
        self.active_lan_locked = false;
        self.clear_lan_password_modal(false);
    }

    fn sync_presence_to_mesh(&self) {
        let friend_pub_keys = self.friends.iter().map(|f| f.pub_key).collect();
        let _ = self.mesh_tx.try_send(MeshCommand::UpdatePresenceConfig {
            rendezvous_server: self.config.rendezvous_server.clone(),
            nickname: self.config.nickname.clone(),
            client_uuid: self.config.client_uuid(),
            identity_secret: self.config.identity_secret().to_bytes(),
            friend_pub_keys,
        });
    }

    fn restore_from_tray(&mut self) -> Vec<Task<Message>> {
        self.is_in_tray = false;
        self.ui_sync.in_tray.store(false, Ordering::Release);
        self.ui_sync.audio_ui_notify.notify_one();
        for item in &self.chat_messages {
            match &item.content {
                ChatContentUi::Text(body_text) => {
                    self.chat_text_editors
                        .entry((item.sender_uuid, item.msg_id))
                        .or_insert_with(|| text_editor::Content::with_text(body_text));
                }
                ChatContentUi::InlineImage {
                    blob_id,
                    filename,
                    bytes: Some(raw),
                    ..
                } => {
                    self.image_handles
                        .entry(*blob_id)
                        .or_insert_with(|| iced_image::Handle::from_bytes(raw.clone()));
                    self.inline_image_bytes
                        .entry(*blob_id)
                        .or_insert_with(|| (filename.clone(), raw.clone()));
                }
                _ => {}
            }
        }
        if self.screen == Screen::Room {
            self.audio.set_streams_active(true);
        } else if self.screen == Screen::Settings {
            self.audio.set_streams_active(true);
            self.audio.set_monitoring_in_settings(true);
            self.ui_sync.in_settings.store(true, Ordering::Release);
        }
        let _ = self
            .mesh_tx
            .try_send(MeshCommand::SetTrayMode { in_tray: false });
        if let Some(id) = self.main_window_id {
            vec![
                iced::window::change_mode(id, iced::window::Mode::Windowed),
                iced::window::gain_focus(id),
            ]
        } else {
            let (id, open_task) = iced::window::open(main_window_settings());
            self.main_window_id = Some(id);
            vec![open_task.map(Message::WindowOpened)]
        }
    }

    fn enter_room_ui_state(&mut self) -> Task<Message> {
        self.screen = Screen::Room;
        self.audio.set_streams_active(true);
        self.audio.set_monitoring_in_settings(false);
        self.hotkeys.set_active(true);
        self.ui_sync.in_room.store(true, Ordering::Release);
        self.ui_sync.in_settings.store(false, Ordering::Release);
        self.ui_sync.audio_ui_notify.notify_one();
        self.sync_overlay_window()
    }

    fn join_with_invite(
        &mut self,
        invite: InviteLink,
        rendezvous_server: String,
        is_lan_room: bool,
        room_name: Option<String>,
    ) -> Task<Message> {
        self.prepare_for_room_entry();
        let _ = self.mesh_tx.try_send(MeshCommand::JoinRoom {
            rendezvous_server,
            invite,
            nickname: self.config.nickname.clone(),
            client_uuid: self.config.client_uuid(),
            initial_volumes: HashMap::new(),
            is_lan_room,
            room_name,
        });
        self.enter_room_ui_state()
    }

    fn sync_overlay_window(&mut self) -> Task<Message> {
        let in_room = self.screen == Screen::Room
            || (self.screen == Screen::Settings && self.previous_screen == Screen::Room);
        let should_open = self.config.overlay_enabled && in_room;

        if should_open {
            if self.overlay_window_id.is_some() {
                self.ui_sync.has_overlay.store(true, Ordering::Release);
                return Task::none();
            }
            #[allow(unused_mut)]
            let mut settings = iced::window::Settings {
                size: Size::new(200.0, 420.0),
                position: iced::window::Position::SpecificWith(overlay_left_center_position),
                visible: true,
                resizable: false,
                decorations: false,
                transparent: true,
                level: iced::window::Level::AlwaysOnTop,
                exit_on_close_request: false,
                ..Default::default()
            };
            #[cfg(target_os = "windows")]
            {
                settings.platform_specific.skip_taskbar = true;
                settings.platform_specific.undecorated_shadow = false;
                settings.platform_specific.drag_and_drop = false;
            }
            let (ov_id, open_task) = iced::window::open(settings);
            self.overlay_window_id = Some(ov_id);
            self.ui_sync.has_overlay.store(true, Ordering::Release);
            self.ui_sync.audio_ui_notify.notify_one();
            open_task.map(Message::OverlayWindowOpened)
        } else if let Some(ov_id) = self.overlay_window_id.take() {
            self.ui_sync.has_overlay.store(false, Ordering::Release);
            self.ui_sync.audio_ui_notify.notify_one();
            iced::window::close(ov_id)
        } else {
            self.ui_sync.has_overlay.store(false, Ordering::Release);
            Task::none()
        }
    }
}

fn overlay_left_center_position(win: Size, screen: Size) -> Point {
    Point::new(18.0, ((screen.height - win.height) * 0.5).max(32.0))
}

pub fn format_short_code(code: u32) -> String {
    format!("{:03}-{:03}", code / 1000, code % 1000)
}

fn parse_short_code_input(raw: &str) -> Result<u32, String> {
    let digits: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err("Enter a 6-digit code (e.g. 482-910)".to_string());
    }
    digits
        .parse::<u32>()
        .map_err(|_| "Enter a valid 6-digit voice code".to_string())
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
        button(text("Close").size(12))
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

#[cfg(not(feature = "managed_updates"))]
fn view_update_modal<'a>(
    info: &'a crate::updater::ReleaseInfo,
    in_progress: bool,
    status_text: Option<&'a str>,
) -> Element<'a, Message> {
    let current = crate::current_app_version();
    let header = column![
        text(format!("Update Available: SquadAir {}", info.tag_name))
            .size(19)
            .color(M3_PRIMARY),
        text(format!(
            "Current version: v{current} • Verified SHA-256 1-Click Installer ({})",
            info.asset_name
        ))
        .size(12)
        .color(M3_ON_SURFACE_VARIANT),
    ]
    .spacing(4);

    let notes_box = container(
        scrollable(text(&info.notes).size(13).color(M3_ON_SURFACE))
            .height(Length::Fixed(180.0))
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .padding(12)
    .style(|_t: &Theme| container::Style {
        background: Some(Background::Color(M3_SURFACE_HIGH)),
        border: Border {
            color: M3_OUTLINE_VARIANT,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    });

    let mut content_col = column![header, notes_box].spacing(14);
    if let Some(status) = status_text {
        content_col = content_col.push(text(status).size(12).color(M3_ON_SURFACE_VARIANT));
    }

    let mut skip_btn = button(text("Skip This Version").size(12))
        .padding([9, 14])
        .style(pill_tonal_button);
    let mut later_btn = button(text("Later").size(12))
        .padding([9, 14])
        .style(pill_tonal_button);
    let update_label = if in_progress {
        "Downloading & Verifying..."
    } else {
        "Update Now (1-Click)"
    };
    let mut update_btn = button(text(update_label).size(13))
        .padding([10, 18])
        .style(pill_success_button);

    if !in_progress {
        skip_btn = skip_btn.on_press(Message::SkipUpdateVersion);
        later_btn = later_btn.on_press(Message::DismissUpdateModal);
        update_btn = update_btn.on_press(Message::StartOneClickUpdate);
    }

    let actions = row![skip_btn, horizontal_space(), later_btn, update_btn]
        .spacing(8)
        .align_y(Vertical::Center);

    let card = container(content_col.push(actions))
        .width(Length::Fixed(540.0))
        .padding(24)
        .style(|_t: &Theme| container::Style {
            background: Some(Background::Color(M3_SURFACE_CONTAINER)),
            text_color: Some(M3_ON_SURFACE),
            border: Border {
                color: M3_OUTLINE,
                width: 1.0,
                radius: 20.0.into(),
            },
            ..Default::default()
        });

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_t: &Theme| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.68))),
            ..Default::default()
        })
        .into()
}

fn sanitize_invite_clipboard_text(raw: &str) -> Option<String> {
    let single_line = raw.replace(['\r', '\n'], " ");
    let trimmed = single_line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn read_invite_clipboard_text() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    cb.get_text().ok()
}

fn read_clipboard_payload(
    screen: Screen,
    text_input_focused: bool,
    invite_empty: bool,
) -> Option<ClipboardPayload> {
    let mut cb = arboard::Clipboard::new().ok()?;

    if screen == Screen::Lobby {
        if !text_input_focused {
            if let Ok(txt) = cb.get_text() {
                let trimmed = txt.trim();
                if invite_empty && trimmed.starts_with("squad://") {
                    return Some(ClipboardPayload::InviteUri(trimmed.to_string()));
                } else if !trimmed.is_empty() {
                    return Some(ClipboardPayload::PlainText(txt));
                }
            }
        }
        return None;
    }

    if screen != Screen::Room {
        return None;
    }

    let img_data = match cb.get_image() {
        Ok(img) => img,
        Err(_) => {
            if !text_input_focused {
                if let Ok(txt) = cb.get_text() {
                    if !txt.is_empty() {
                        return Some(ClipboardPayload::PlainText(txt));
                    }
                }
            }
            return None;
        }
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

    for &(max_dim, quality) in &[
        (1920u32, 85u8),
        (1280, 75),
        (1024, 58),
        (800, 45),
        (640, 35),
    ] {
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
    let img = crate::ui::avatar::decode_bounded_image(encoded_bytes, 4096, 64 * 1024 * 1024)
        .map_err(|e| format!("Failed to decode image for clipboard: {e}"))?;
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("Clipboard unavailable: {e}"))?;
    cb.set_image(arboard::ImageData {
        width,
        height,
        bytes: Cow::Owned(rgba.into_raw()),
    })
    .map_err(|e| format!("Failed to write image to clipboard: {e}"))
}

fn spawn_and_reap(mut cmd: std::process::Command) {
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn open_in_file_manager(saved_path: Option<PathBuf>) {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        if let Some(path) = saved_path.filter(|p| p.exists()) {
            cmd.arg("-R").arg(path);
        } else {
            let dir = downloads_root_dir();
            let _ = std::fs::create_dir_all(&dir);
            cmd.arg(dir);
        }
        spawn_and_reap(cmd);
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("explorer");
        if let Some(path) = saved_path.filter(|p| p.exists()) {
            cmd.arg(format!("/select,{}", path.display()));
        } else {
            let dir = downloads_root_dir();
            let _ = std::fs::create_dir_all(&dir);
            cmd.arg(dir);
        }
        spawn_and_reap(cmd);
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let dir = saved_path
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(downloads_root_dir);
        let _ = std::fs::create_dir_all(&dir);
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(dir);
        spawn_and_reap(cmd);
    }
}

fn set_window_fullscreen_task(window_id: iced::window::Id, fullscreen: bool) -> Task<Message> {
    let mode = if fullscreen {
        iced::window::Mode::Fullscreen
    } else {
        iced::window::Mode::Windowed
    };
    iced::window::change_mode(window_id, mode)
}

fn hex_prefix(bytes: &[u8; 4]) -> String {
    format!(
        "{:02X}{:02X}{:02X}{:02X}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

fn listen_normal_event(
    evt: iced::Event,
    status: iced::event::Status,
    _win: iced::window::Id,
) -> Option<Message> {
    let iced::Event::Keyboard(keyboard::Event::KeyPressed {
        key,
        physical_key,
        modifiers,
        ..
    }) = evt
    else {
        return None;
    };

    let is_c = matches_layout_agnostic_key(&key, physical_key, b'c', keyboard::key::Code::KeyC);
    if modifiers.command() && !modifiers.alt() && is_c && status != iced::event::Status::Captured {
        return Some(Message::CopySelectedChatText);
    }
    let is_v = matches_layout_agnostic_key(&key, physical_key, b'v', keyboard::key::Code::KeyV);
    if modifiers.command() && !modifiers.alt() && is_v {
        return Some(Message::PasteFromClipboard {
            text_input_focused: status == iced::event::Status::Captured,
        });
    }
    let is_f = matches_layout_agnostic_key(&key, physical_key, b'f', keyboard::key::Code::KeyF);
    if status != iced::event::Status::Captured
        && !modifiers.command()
        && !modifiers.control()
        && !modifiers.alt()
        && is_f
    {
        return Some(Message::ToggleStreamFullscreen);
    }
    if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
        return Some(Message::EscapePressed);
    }
    None
}

fn listen_recording_event(
    evt: iced::Event,
    _status: iced::event::Status,
    _win: iced::window::Id,
) -> Option<Message> {
    match evt {
        iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key, physical_key, ..
        }) => {
            if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
                return Some(Message::CancelRecordingHotkey);
            }
            let name = iced_key_to_hotkey_name(&key, physical_key)?;
            if is_supported_hotkey_name(&name) {
                Some(Message::RecordedHotkeyInput(name))
            } else {
                None
            }
        }
        iced::Event::Mouse(mouse::Event::ButtonPressed(btn)) => {
            let name = match btn {
                mouse::Button::Middle => Some("Mouse3"),
                mouse::Button::Back | mouse::Button::Other(4) => Some("Mouse4"),
                mouse::Button::Forward | mouse::Button::Other(5) => Some("Mouse5"),
                _ => None,
            }?;
            if is_supported_hotkey_name(name) {
                Some(Message::RecordedHotkeyInput(name.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn iced_key_to_hotkey_name(
    key: &keyboard::Key,
    physical_key: keyboard::key::Physical,
) -> Option<String> {
    use keyboard::key::{Code, Named, Physical};
    if let Physical::Code(code) = physical_key {
        let mapped = match code {
            Code::KeyA => Some("KeyA"),
            Code::KeyB => Some("KeyB"),
            Code::KeyC => Some("KeyC"),
            Code::KeyD => Some("KeyD"),
            Code::KeyE => Some("KeyE"),
            Code::KeyF => Some("KeyF"),
            Code::KeyG => Some("KeyG"),
            Code::KeyH => Some("KeyH"),
            Code::KeyI => Some("KeyI"),
            Code::KeyJ => Some("KeyJ"),
            Code::KeyK => Some("KeyK"),
            Code::KeyL => Some("KeyL"),
            Code::KeyM => Some("KeyM"),
            Code::KeyN => Some("KeyN"),
            Code::KeyO => Some("KeyO"),
            Code::KeyP => Some("KeyP"),
            Code::KeyQ => Some("KeyQ"),
            Code::KeyR => Some("KeyR"),
            Code::KeyS => Some("KeyS"),
            Code::KeyT => Some("KeyT"),
            Code::KeyU => Some("KeyU"),
            Code::KeyV => Some("KeyV"),
            Code::KeyW => Some("KeyW"),
            Code::KeyX => Some("KeyX"),
            Code::KeyY => Some("KeyY"),
            Code::KeyZ => Some("KeyZ"),
            Code::Digit0 => Some("Digit0"),
            Code::Digit1 => Some("Digit1"),
            Code::Digit2 => Some("Digit2"),
            Code::Digit3 => Some("Digit3"),
            Code::Digit4 => Some("Digit4"),
            Code::Digit5 => Some("Digit5"),
            Code::Digit6 => Some("Digit6"),
            Code::Digit7 => Some("Digit7"),
            Code::Digit8 => Some("Digit8"),
            Code::Digit9 => Some("Digit9"),
            Code::F1 => Some("F1"),
            Code::F2 => Some("F2"),
            Code::F3 => Some("F3"),
            Code::F4 => Some("F4"),
            Code::F5 => Some("F5"),
            Code::F6 => Some("F6"),
            Code::F7 => Some("F7"),
            Code::F8 => Some("F8"),
            Code::F9 => Some("F9"),
            Code::F10 => Some("F10"),
            Code::F11 => Some("F11"),
            Code::F12 => Some("F12"),
            Code::Space => Some("Space"),
            Code::Tab => Some("Tab"),
            Code::Enter => Some("Enter"),
            Code::Backspace => Some("Backspace"),
            Code::Delete => Some("Delete"),
            Code::Insert => Some("Insert"),
            Code::Home => Some("Home"),
            Code::End => Some("End"),
            Code::PageUp => Some("PageUp"),
            Code::PageDown => Some("PageDown"),
            Code::ArrowUp => Some("ArrowUp"),
            Code::ArrowDown => Some("ArrowDown"),
            Code::ArrowLeft => Some("ArrowLeft"),
            Code::ArrowRight => Some("ArrowRight"),
            Code::CapsLock => Some("CapsLock"),
            Code::ShiftLeft => Some("LShift"),
            Code::ShiftRight => Some("RShift"),
            Code::ControlLeft => Some("LCtrl"),
            Code::ControlRight => Some("RCtrl"),
            Code::AltLeft => Some("LAlt"),
            Code::AltRight => Some("RAlt"),
            Code::SuperLeft => Some("MetaLeft"),
            Code::SuperRight => Some("MetaRight"),
            Code::Backquote => Some("Backquote"),
            Code::Minus => Some("Minus"),
            Code::Equal => Some("Equal"),
            Code::BracketLeft => Some("BracketLeft"),
            Code::BracketRight => Some("BracketRight"),
            Code::Backslash => Some("Backslash"),
            Code::Semicolon => Some("Semicolon"),
            Code::Quote => Some("Quote"),
            Code::Comma => Some("Comma"),
            Code::Period => Some("Period"),
            Code::Slash => Some("Slash"),
            _ => None,
        };
        if let Some(name) = mapped {
            return Some(name.to_string());
        }
    }

    match key {
        keyboard::Key::Named(Named::CapsLock) => Some("CapsLock".to_string()),
        keyboard::Key::Named(Named::Space) => Some("Space".to_string()),
        keyboard::Key::Named(Named::Tab) => Some("Tab".to_string()),
        keyboard::Key::Named(Named::Enter) => Some("Enter".to_string()),
        keyboard::Key::Named(Named::Backspace) => Some("Backspace".to_string()),
        keyboard::Key::Named(Named::Delete) => Some("Delete".to_string()),
        keyboard::Key::Named(Named::Insert) => Some("Insert".to_string()),
        keyboard::Key::Named(Named::Home) => Some("Home".to_string()),
        keyboard::Key::Named(Named::End) => Some("End".to_string()),
        keyboard::Key::Named(Named::PageUp) => Some("PageUp".to_string()),
        keyboard::Key::Named(Named::PageDown) => Some("PageDown".to_string()),
        keyboard::Key::Named(Named::ArrowUp) => Some("ArrowUp".to_string()),
        keyboard::Key::Named(Named::ArrowDown) => Some("ArrowDown".to_string()),
        keyboard::Key::Named(Named::ArrowLeft) => Some("ArrowLeft".to_string()),
        keyboard::Key::Named(Named::ArrowRight) => Some("ArrowRight".to_string()),
        keyboard::Key::Named(Named::F1) => Some("F1".to_string()),
        keyboard::Key::Named(Named::F2) => Some("F2".to_string()),
        keyboard::Key::Named(Named::F3) => Some("F3".to_string()),
        keyboard::Key::Named(Named::F4) => Some("F4".to_string()),
        keyboard::Key::Named(Named::F5) => Some("F5".to_string()),
        keyboard::Key::Named(Named::F6) => Some("F6".to_string()),
        keyboard::Key::Named(Named::F7) => Some("F7".to_string()),
        keyboard::Key::Named(Named::F8) => Some("F8".to_string()),
        keyboard::Key::Named(Named::F9) => Some("F9".to_string()),
        keyboard::Key::Named(Named::F10) => Some("F10".to_string()),
        keyboard::Key::Named(Named::F11) => Some("F11".to_string()),
        keyboard::Key::Named(Named::F12) => Some("F12".to_string()),
        keyboard::Key::Named(Named::Shift) => Some("Shift".to_string()),
        keyboard::Key::Named(Named::Control) => Some("Ctrl".to_string()),
        keyboard::Key::Named(Named::Alt) => Some("Alt".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_invite_clipboard_text;

    #[test]
    fn sanitize_invite_clipboard_trims_and_rejects_empty() {
        assert_eq!(sanitize_invite_clipboard_text("   \r\n\t "), None);
        assert_eq!(
            sanitize_invite_clipboard_text("  squad://abc\r\ndef  "),
            Some("squad://abc  def".to_string())
        );
        assert_eq!(
            sanitize_invite_clipboard_text("\n  squad://room-key  \n"),
            Some("squad://room-key".to_string())
        );
    }
}
