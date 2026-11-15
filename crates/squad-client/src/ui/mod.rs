pub mod chat;
pub mod components;
pub mod lobby;
pub mod room;
pub mod settings;
pub mod theme;

use crate::config::AppConfig;
use crate::hotkeys::{request_macos_permissions, HotkeyController};
use crate::net::{
    downloads_root_dir, spawn_mesh_worker, ChatContentUi, ChatItemUi, MeshCommand, MeshEvent,
    PeerUiSnapshot,
};
use crate::tray::{SystemTray, TrayAction};
use crate::ui::theme::{material_dark_theme, root_container_style};
use iced::{
    clipboard, event, keyboard, time,
    widget::{container, image as iced_image},
    Element, Length, Subscription, Task, Theme,
};
use image::imageops::FilterType;
use squad_dsp::{
    list_input_devices, list_output_devices, AudioEngineConfig, AudioEngineHandle,
    MicActivationMode, NoiseSuppressionMode, SoundCue,
};
use squad_proto::{ClientUuid, InviteLink, PeerId, INLINE_IMAGE_MAX_BYTES};
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
    PasteFromClipboard,
    ClipboardProcessed(Option<ClipboardPayload>),
    DownloadBlob([u8; 16]),
    OpenDownloadsFolder(Option<PathBuf>),
    PeerVolumeChanged {
        peer_id: PeerId,
        client_uuid: ClientUuid,
        gain: f32,
    },
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
    pub invite_input: String,
    pub save_channel_name_input: String,
    pub active_invite_uri: String,
    pub local_peer_id: PeerId,
    pub peers: Vec<PeerUiSnapshot>,
    pub chat_messages: Vec<ChatItemUi>,
    pub image_handles: HashMap<[u8; 16], iced_image::Handle>,
    pub chat_input: String,
    pub status_banner: Option<String>,
    pub invite_copied_flash: bool,
    pub input_devices: Vec<String>,
    pub output_devices: Vec<String>,
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
            bitrate_bps: 40_000,
        };

        let audio = AudioEngineHandle::start(engine_cfg, audio_out_tx)
            .expect("Failed to initialize Squad-Air DSP audio engine");

        let (mesh_tx, mesh_rx) = spawn_mesh_worker(audio.clone(), audio_out_rx);
        let hotkeys = HotkeyController::new(
            &config.hotkey_toggle_mute,
            &config.hotkey_ptt,
            &config.hotkey_ptm,
        );
        let tray = SystemTray::new();

        let app = Self {
            config,
            screen: Screen::Lobby,
            previous_screen: Screen::Lobby,
            audio,
            mesh_tx,
            mesh_rx: Arc::new(Mutex::new(mesh_rx)),
            hotkeys,
            tray,
            invite_input: String::new(),
            save_channel_name_input: String::new(),
            active_invite_uri: String::new(),
            local_peer_id: 0,
            peers: Vec::new(),
            chat_messages: Vec::new(),
            image_handles: HashMap::new(),
            chat_input: String::new(),
            status_banner: None,
            invite_copied_flash: false,
            input_devices: list_input_devices(),
            output_devices: list_output_devices(),
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
        let keys = event::listen_with(|evt, _status, _win| match evt {
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Character(ref ch),
                modifiers,
                ..
            }) if modifiers.command() && ch.eq_ignore_ascii_case("v") => {
                Some(Message::PasteFromClipboard)
            }
            _ => None,
        });
        Subscription::batch([tick, keys])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.hotkeys.poll_events(&self.audio);
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
                                self.chat_input.clear();
                                self.local_peer_id = 0;
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
                                    bytes: Some(ref raw),
                                    ..
                                } = item.content
                                {
                                    self.image_handles
                                        .insert(blob_id, iced_image::Handle::from_bytes(raw.clone()));
                                }
                                self.chat_messages.push(item);
                                if self.chat_messages.len() > 100 {
                                    self.chat_messages.remove(0);
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
                                            state: st,
                                            bytes,
                                            ..
                                        } if *bid == blob_id => {
                                            *st = state.clone();
                                            if inline_bytes.is_some() {
                                                *bytes = inline_bytes.clone();
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
                            MeshEvent::Error(err) => {
                                self.status_banner = Some(err);
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
                self.chat_input.clear();
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
            Message::PasteFromClipboard => {
                if self.screen != Screen::Room {
                    return Task::none();
                }
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(read_and_compress_clipboard_image)
                            .await
                            .ok()
                            .flatten()
                    },
                    Message::ClipboardProcessed,
                )
            }
            Message::ClipboardProcessed(Some(payload)) => {
                if self.screen == Screen::Room {
                    match payload {
                        ClipboardPayload::InlineImage { filename, bytes } => {
                            let _ = self
                                .mesh_tx
                                .try_send(MeshCommand::SendInlineImage { filename, bytes });
                        }
                        ClipboardPayload::TempFile(path) => {
                            let _ = self.mesh_tx.try_send(MeshCommand::OfferFile { path });
                        }
                    }
                }
                Task::none()
            }
            Message::ClipboardProcessed(None) => Task::none(),
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
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let content = match self.screen {
            Screen::Lobby => lobby::view_lobby(self),
            Screen::Room => room::view_room(self),
            Screen::Settings => settings::view_settings(self),
        };

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(root_container_style)
            .into()
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

fn read_and_compress_clipboard_image() -> Option<ClipboardPayload> {
    let mut cb = arboard::Clipboard::new().ok()?;
    let img_data = cb.get_image().ok()?;
    let rgba = image::RgbaImage::from_raw(
        img_data.width as u32,
        img_data.height as u32,
        img_data.bytes.into_owned(),
    )?;

    let mut dyn_img = image::DynamicImage::ImageRgba8(rgba);
    if dyn_img.width() > 1280 || dyn_img.height() > 1280 {
        dyn_img = dyn_img.resize(1280, 1280, FilterType::Triangle);
    }

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    for &(max_dim, quality) in &[(1280u32, 82u8), (1280, 68), (1024, 55), (800, 45), (640, 35)] {
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
