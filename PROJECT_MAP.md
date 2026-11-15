# PROJECT_MAP.md — SquadAir Structural & API Map

## 1. Repository Tree & File Responsibilities

```text
squad-air/
├── Cargo.toml                              # Workspace members & pinned dependency versions
├── SPECIFICATION.md                        # Full functional & protocol requirements
├── PROJECT_MAP.md                          # This structural map & cross-crate API contracts
├── .agent/skills/squad-rust-guardrails/
│   └── SKILL.md                            # Crate-specific API guardrails (iced 0.13, ringbuf 0.4)
├── assets/
│   ├── models/README.md                    # Optional deepfilternet3.onnx placement guide
│   └── sounds/README.md                    # Optional WAV cues manifest (join, leave, peer_join, peer_leave, mute, unmute)
├── crates/
│   ├── squad-proto/                       # [LIB] Wire protocol, AEAD crypto, invite URI parser (Zero UI/Audio deps)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # Re-exports public protocol types & error enum
│   │       ├── packet.rs                   # Binary Big-Endian encoding/decoding for Signaling & P2P packets
│   │       ├── crypto.rs                   # BLAKE3 key derivation + ChaCha20-Poly1305 deterministic-nonce AEAD
│   │       └── invite.rs                   # `voice://<base58_room_id>#<base58_secret>` generator & parser
│   ├── squad-rendezvous/                  # [BIN] Standalone async UDP Hole Punching coordinator
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs                     # CLI (--bind 0.0.0.0:3478), Tokio UDP loop, 2s TTL reaper task
│   │       └── state.rs                    # DashMap room registry (max 10 peers/room), IP token-bucket rate limiter
│   ├── squad-dsp/                         # [LIB] Lock-free audio I/O, Resampler, AEC3, NS, Opus 10ms, Mixer
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # Re-exports AudioEngine, AudioConfig, DspSettings, PeerTelemetry
│   │       ├── engine.rs                   # cpal stream lifecycle + dedicated real-time DSP worker thread
│   │       ├── resampler.rs                # rubato converter (Device Hz <-> 48kHz mono, 480-sample chunks)
│   │       ├── aec.rs                      # WebRTC AEC3 wrapper + far-end reference ring buffer
│   │       ├── denoise.rs                  # NoiseSuppressor (Off | Rnnoise | DeepFilterNet3 via tract-onnx)
│   │       ├── codec.rs                    # Opus 48kHz/10ms encoder & decoder (DTX + In-band FEC)
│   │       ├── jitter.rs                   # Per-peer adaptive 20-40ms jitter buffer + PLC + packet loss %
│   │       ├── mixer.rs                    # Multi-peer f32 summer + per-peer gain (0.0..2.0) + soft-clipper
│   │       └── cues.rs                     # Optional hound WAV loader (graceful None if file missing)
│   └── squad-client/                      # [BIN] Desktop app: iced 0.13 M3 Dark UI + P2P Mesh + Hotkeys + Tray
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs                     # Entry point, tracing init, iced::application launcher
│           ├── config.rs                   # TOML persistence (`dirs::config_dir()/squad-air/config.toml`)
│           ├── hotkeys.rs                  # global-hotkey handler (PushToTalk, PushToMute, ToggleMute)
│           ├── tray.rs                     # tray-icon menu (Show/Hide, Mute, Quit)
│           ├── net/
│           │   ├── mod.rs                  # MeshCommand / MeshEvent channel definitions
│           │   ├── nat.rs                  # Local IPv4 LAN interface discovery (`if-addrs`)
│           │   └── mesh.rs                 # Single-socket UDP Mesh, concurrent WAN/LAN Hole Punching, RTT EWMA
│           └── ui/
│               ├── mod.rs                  # Main App state, iced update/view/subscription
│               ├── theme.rs                # Material Design 3 Dark palette & custom widget styles
│               ├── components.rs           # M3 pill buttons, cards, speaking ring avatar, telemetry badge
│               ├── lobby.rs                # Home view: Create Room / Join via invite URI / Settings trigger
│               ├── room.rs                 # Active call view: 10-peer grid, volume sliders, bottom dock
│               └── settings.rs             # Settings modal: Nickname, Audio devices, AEC/NS toggles, Hotkeys
└── tests/
    └── e2e_mesh_loopback.rs                # Multi-peer E2EE audio + hole punching integration test

2. Cross-Crate Public Contracts (Do Not Deviate)
A. squad-proto Public API
Rust

pub const MAGIC: [u8; 2] = [0xAE, 0x77];
pub const PROTO_VERSION: u8 = 0x01;
pub const MAX_ROOM_PEERS: usize = 10;

pub type RoomId = [u8; 16];
pub type ClientUuid = [u8; 16];
pub type PeerId = u16; // 1..=10

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    pub room_id: RoomId,
    pub secret: [u8; 32],
}
// InviteLink::generate() -> Self
// InviteLink::to_uri(&self) -> String ("voice://<b58_room>#<b58_secret>")
// InviteLink::parse(uri: &str) -> Result<Self, ProtoError>

pub struct RoomCipher { /* ChaCha20Poly1305 */ }
// RoomCipher::from_secret(secret: &[u8; 32]) -> Self (uses blake3::derive_key)
// RoomCipher::seal_p2p(&self, header: &P2pHeader, plaintext: &[u8], out: &mut Vec<u8>) -> Result<(), ProtoError>
// RoomCipher::open_p2p<'a>(&self, datagram: &'a mut [u8]) -> Result<(P2pHeader, &'a [u8]), ProtoError>

B. squad-dsp Public API
Rust

pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME_SAMPLES: usize = 480; // strict 10ms

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseSuppressionMode { Off, Rnnoise, DeepFilterNet3 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicActivationMode { ToggleMute, PushToTalk, PushToMute }

pub struct AudioEngineHandle { /* lock-free atomics + bounded command channel */ }
// AudioEngineHandle::start(input_dev: Option<String>, output_dev: Option<String>, outbound_tx: Sender<Vec<u8>>) -> Result<Self>
// AudioEngineHandle::push_remote_opus(&self, peer_id: PeerId, seq: u64, opus_payload: &[u8])
// AudioEngineHandle::set_peer_gain(&self, peer_id: PeerId, gain: f32) // 0.0..=2.0
// AudioEngineHandle::remove_peer(&self, peer_id: PeerId)
// AudioEngineHandle::set_aec_enabled(&self, enabled: bool)
// AudioEngineHandle::set_ns_mode(&self, mode: NoiseSuppressionMode)
// AudioEngineHandle::set_mic_muted(&self, muted: bool)
// AudioEngineHandle::set_deafened(&self, deafened: bool)
// AudioEngineHandle::play_cue(&self, cue: SoundCue)
// AudioEngineHandle::local_mic_level_db(&self) -> f32
// AudioEngineHandle::peer_stats(&self, peer_id: PeerId) -> Option<PeerAudioStats> { loss_pct: f32, is_speaking: bool }

C. squad-client Internal Channels (net/mod.rs)
Rust

pub enum MeshCommand {
    JoinRoom { server_addr: String, invite: InviteLink, nickname: String, client_uuid: ClientUuid },
    LeaveRoom,
    SendAudioFrame { seq: u64, opus_bytes: Bytes },
}

pub enum MeshEvent {
    Joined { local_peer_id: PeerId },
    PeerUpdated(PeerSnapshot), // peer_id, client_uuid, nickname, conn_state, rtt_ms, loss_pct, is_speaking
    PeerLeft { peer_id: PeerId },
    RemoteAudio { peer_id: PeerId, seq: u64, opus_bytes: Bytes },
    Error(String),
}

3. Data & Threading Topology (Zero-Contention Pipeline)
Plaintext

[cpal Input Thread] --(lock-free HeapRb<f32>)--> [DSP Worker Thread]
                                                   ├─ rubato -> 48kHz (480 samples)
                                                   ├─ aec3 (reads far-end HeapRb)
                                                   ├─ denoise (Off | nnnoiseless | tract-onnx)
                                                   ├─ Opus 10ms encode (DTX + FEC)
                                                   └─(mpsc)--> [Tokio Mesh Task]
                                                                 ├─ ChaCha20-Poly1305 seal
                                                                 └─ UdpSocket::send_to (all connected peers)

[Tokio Mesh Task] <---(UdpSocket::recv_from)--- [Network]
  ├─ ChaCha20-Poly1305 open
  └─(mpsc)--> [DSP Worker Thread]
                ├─ Per-peer JitterBuffer (Opus decode + PLC)
                ├─ Mixer (sum * peer_gain + WAV cues + soft-clip)
                ├─(HeapRb<f32>)--> [AEC Far-End Reference]
                └─(HeapRb<f32>)--> [cpal Output Thread]