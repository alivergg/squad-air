# SPECIFICATION.md — P2P Encrypted Low-Latency Voice Chat ("SquadAir")

## 1. Executive Summary & Core Constraints
Build a cross-platform (**Windows x86_64** and **macOS Apple Silicon ARM64**), ultra-low-latency, encrypted P2P voice communication application in **pure Rust** designed for competitive gaming (e.g., CS2) with up to **10 concurrent participants** per room.

### Non-Negotiable Constraints
1. **Architecture:** Full-Mesh P2P UDP audio transport + lightweight stateless/in-memory UDP Rendezvous server for NAT Hole Punching (MVP: direct Hole Punching only, no media relay).
2. **End-to-End Encryption:** Symmetric `ChaCha20-Poly1305` on all P2P packets using a 256-bit room secret generated client-side (`voice://<room_id>#<base58_secret>`). The Rendezvous server **never** receives `#<base58_secret>`.
3. **Audio Pipeline:** 48,000 Hz Mono, strict **10 ms frame size (480 samples)**, Opus codec with **DTX** (Discontinuous Transmission) and **FEC**, lock-free real-time audio threads (`ringbuf`).
4. **DSP Suite (User-Configurable in Settings):**
   - **Echo Cancellation (AEC):** Toggle (`Off` / `On`) using pure-Rust `aec3` (WebRTC AEC3 port). Must execute **before** Noise Suppression.
   - **Noise Suppression (NS):** Selector (`Off` / `RNNoise` via `nnnoiseless` / `DeepFilterNet3` via `tract-onnx`).
5. **UI & UX:** Pure Rust `iced` (v0.13) with a custom hardcoded **Material Design 3 (Material You) Dark Theme**, system tray integration, global hotkeys (`Push-to-Talk`, `Push-to-Mute`, `Toggle Mute`), per-peer persistent volume sliders (`0%–200%`), live RTT (ms) and Packet Loss (%) telemetry, and zero-account onboarding with auto-generated default nickname.
6. **Optional Audio Cues:** Load `.wav` notification cues from `./assets/sounds/` if present; silently no-op if missing. Include `assets/sounds/README.md` documenting required filenames.

---

## 2. Cargo Workspace Layout

Create a Cargo workspace with 4 crates to isolate DSP, network protocol, Rendezvous server, and GUI client:

```text
squad-air/
├── Cargo.toml                  # Workspace definition & shared dependency versions
├── SPECIFICATION.md            # This specification
├── assets/
│   ├── models/
│   │   └── README.md           # Instructions for optional deepfilternet3.onnx placement
│   └── sounds/
│       └── README.md           # Manifest of optional .wav notification cues
├── crates/
│   ├── squad-proto/           # Wire protocol, serialization, ChaCha20-Poly1305 E2EE, invite URI parser
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── packet.rs       # Binary packet headers, opcodes, zero-copy/fast encoding
│   │       ├── crypto.rs       # Key derivation (BLAKE3) & ChaCha20-Poly1305 AEAD seal/open
│   │       └── invite.rs       # `voice://<room_id>#<secret>` parser & generator
│   ├── squad-dsp/             # Lock-free audio engine, resampler, AEC3, RNNoise, DeepFilterNet3, Opus, Mixer
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── engine.rs       # cpal input/output streams + SPSC ringbuf orchestration
│   │       ├── resampler.rs    # rubato 48kHz mono frame adapter (480 samples)
│   │       ├── aec.rs          # WebRTC AEC3 wrapper with far-end reference ring buffer
│   │       ├── denoise.rs      # NoiseSuppressor trait: Off, Rnnoise, DeepFilterNet3 (tract-onnx)
│   │       ├── codec.rs        # Opus 10ms encoder/decoder with DTX & in-band FEC
│   │       ├── jitter.rs       # Per-peer adaptive 10-30ms jitter buffer + loss stats
│   │       ├── mixer.rs        # Multi-peer f32 summer + per-peer gain + soft-clipper + WAV cue mixer
│   │       └── cues.rs         # Optional WAV loader (hound) with graceful None fallback
│   ├── squad-rendezvous/      # Standalone lightweight UDP signaling & hole-punching coordinator
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs         # Tokio async UDP server, room registry, TTL cleanup
│   │       └── state.rs        # DashMap room/peer state & rate-limiting
│   └── squad-client/          # Desktop GUI app (iced 0.13 M3 Dark + P2P Mesh + Hotkeys + Tray)
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs         # Entry point, logging, single-instance / tray setup
│           ├── config.rs       # Persistent TOML config (nickname, UUID, server addr, peer volumes, hotkeys)
│           ├── net/
│           │   ├── mod.rs
│           │   ├── mesh.rs     # UDP Mesh manager, Hole Punching state machine, RTT ping/pong
│           │   └── nat.rs      # Local LAN IP discovery (`if-addrs`)
│           ├── hotkeys.rs      # Cross-platform global hotkeys (PTT, PTM, Toggle Mute)
│           ├── tray.rs         # System tray icon & menu (`tray-icon`)
│           └── ui/
│               ├── mod.rs
│               ├── theme.rs    # Material Design 3 Dark Theme palette & widget styles
│               ├── components.rs # M3 pills, cards, speaking ring avatar, RTT/loss badges
│               ├── lobby.rs    # Home screen: Create Room / Join via invite link / Quick Settings
│               ├── room.rs     # Active Call screen: 10-peer grid, volume sliders, mic controls
│               └── settings.rs # Settings modal: Devices, NS selector, AEC toggle, Hotkeys, Nickname
└── tests/
    └── e2e_mesh_loopback.rs    # Full multi-peer E2EE audio + hole punching integration test
```

---

## 3. Dependencies & Build Configuration (`Cargo.toml`)

Use pure-Rust or self-contained C crates that compile cleanly on both Windows (`x86_64-pc-windows-msvc`) and macOS ARM64 (`aarch64-apple-darwin`) without external package managers:

```toml
[workspace]
resolver = "2"
members = [
    "crates/squad-proto",
    "crates/squad-dsp",
    "crates/squad-rendezvous",
    "crates/squad-client",
]

[workspace.dependencies]
tokio = { version = "1.41", features = ["full"] }
bytes = "1.8"
thiserror = "2.0"
anyhow = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
serde = { version = "1.0", features = ["derive"] }
toml = "0.8"
rand = "0.8"
uuid = { version = "1.11", features = ["v4", "serde"] }
bs58 = "0.5"
blake3 = "1.5"
chacha20poly1305 = "0.10"
dashmap = "6.1"
cpal = "0.15"
ringbuf = "0.4"
rubato = "0.16"
nnnoiseless = "0.5"
aec3 = "0.1"
tract-onnx = "0.21"
audiopus = "0.3.0-rc.0"
hound = "3.5"
dirs = "5.0"
if-addrs = "0.13"
iced = { version = "0.13", features = ["tokio", "canvas", "advanced"] }
global-hotkey = "0.6"
tray-icon = "0.19"
```

> **Build Note on `audiopus`:** If `audiopus_sys` requires CMake on Windows, verify `cmake` is present in CI/environment; alternatively fallback to `opus = "0.3"` if prebuilt/cc build is preferred. Document this check in the verification step.

---

## 4. Binary Wire Protocol (`squad-proto`)

All multi-byte integers are **Big-Endian (Network Byte Order)**. Maximum UDP payload is strictly capped at **1200 bytes** to prevent IP fragmentation.

### 4.1 Magic Header & Packet Framing
Every UDP datagram begins with a 4-byte header:
- `magic`: `[u8; 2]` = `[0xAE, 0x77]`
- `version`: `u8` = `0x01`
- `msg_type`: `u8` (opcode)

### 4.2 Signaling Packets (Client ↔ Rendezvous Server — Unencrypted Metadata)
The Rendezvous server never sees room secrets or audio.
- `0x01` **`C2S_REGISTER`**:
  - `room_id`: `[u8; 16]` (128-bit random room ID)
  - `client_uuid`: `[u8; 16]` (persistent device/user UUID for volume memory)
  - `nick_len`: `u8` (max 32 bytes UTF-8)
  - `nickname`: `[u8; nick_len]`
  - `lan_addr_count`: `u8` (max 4)
  - `lan_addrs`: repeated `[ip_v4: [u8; 4], port: u16]`
- `0x02` **`S2C_ROOM_STATE`**:
  - `assigned_peer_id`: `u16` (ephemeral slot ID `1..=10` inside room)
  - `peer_count`: `u8` (`<= 10`)
  - `peers`: repeated `PeerRecord`:
    - `peer_id`: `u16`
    - `client_uuid`: `[u8; 16]`
    - `public_ip`: `[u8; 4]` (or 16B for IPv6 flag; support IPv4 + IPv6 via `addr_family: u8` where `4` = IPv4, `6` = IPv6)
    - `public_port`: `u16`
    - `lan_addr_count`: `u8`
    - `lan_addrs`: repeated `[ip_v4: [u8; 4], port: u16]`
    - `nick_len`: `u8`
    - `nickname`: `[u8; nick_len]`
- `0x03` **`C2S_LEAVE`**:
  - `room_id`: `[u8; 16]`
  - `client_uuid`: `[u8; 16]`
- `0x04` **`S2C_ERROR`**:
  - `error_code`: `u8` (`1 = RoomFull`, `2 = RateLimited`, `3 = InvalidPacket`)

### 4.3 P2P Mesh Packets (Client ↔ Client — End-to-End Encrypted)
All P2P packets carry a cleartext 15-byte header used as **AEAD Additional Authenticated Data (AAD)**, followed by `ChaCha20-Poly1305` ciphertext + 16-byte Poly1305 tag:
- **Cleartext Header (15 bytes = AAD):**
  - `magic`: `[0xAE, 0x77]` (2B)
  - `version`: `0x01` (1B)
  - `msg_type`: `u8` (1B) — `0x10` (`PUNCH_SYN`), `0x11` (`PUNCH_ACK`), `0x12` (`PING`), `0x13` (`PONG`), `0x20` (`AUDIO_OPUS`), `0x21` (`AUDIO_DTX_KEEPALIVE`)
  - `room_id`: `[u8; 4]` (first 4 bytes of `room_id` for fast demux)
  - `sender_peer_id`: `u16` (2B)
  - `seq_num`: `u64` (5B truncated or full 8B — use **full `u64` (8B)**, making total cleartext AAD header **18 bytes**).
- **96-bit Nonce Construction (Zero Wire Overhead):**
  - `nonce[0..2] = sender_peer_id.to_be_bytes()`
  - `nonce[2] = msg_type`
  - `nonce[3..11] = seq_num.to_be_bytes()`
  - `nonce[11] = 0x00`
- **Key Derivation:**
  - Invite URI format: `voice://<base58_room_id_16B>#<base58_secret_32B>`
  - `room_key = blake3::derive_key("squad-air-e2ee-v1", &secret_bytes)` (32 bytes).
- **Encrypted Payloads:**
  - `0x10` `PUNCH_SYN`: `[client_uuid: 16B][timestamp_us: u64]`
  - `0x11` `PUNCH_ACK`: `[client_uuid: 16B][echo_timestamp_us: u64]`
  - `0x12` `PING`: `[timestamp_us: u64]`
  - `0x13` `PONG`: `[echo_timestamp_us: u64]`
  - `0x20` `AUDIO_OPUS`: `[opus_payload_len: u16][opus_bytes...]`
  - `0x21` `AUDIO_DTX_KEEPALIVE`: empty encrypted payload (sent every 500 ms while muted/silent to keep NAT pinhole warm).

---

## 5. Audio & DSP Engine Specification (`squad-dsp`)

### 5.1 Real-Time Threading Rules (Strict Lock-Free)
- `cpal` input and output callbacks run on OS real-time audio threads.
- **FORBIDDEN inside `cpal` callbacks:** heap allocations (`Vec::push`, `Box::new`), `std::sync::Mutex::lock()`, logging (`tracing`), file/network I/O.
- **Communication:** Use `ringbuf::HeapRb<f32>` (Single-Producer Single-Consumer lock-free ring buffers) between `cpal` callbacks and the dedicated `dsp_worker` thread.

### 5.2 Capture DSP Pipeline (Exact Order)
1. **Hardware Input (`cpal`):** Captures at device native sample rate (e.g., 44.1 kHz or 48 kHz) and channel count. Downmixes to mono `f32` in `[-1.0, 1.0]` and pushes into `capture_rb`.
2. **Resampler (`rubato`):** Converts input stream to **48,000 Hz** and slices into exact **480-sample (10 ms)** frames.
3. **Acoustic Echo Cancellation (`aec.rs`):**
   - If `aec_enabled == true`:
     - Drain available 480-sample far-end frames from `aec_render_rb` (fed by the output mixer) and call `aec3.handle_render_frame(&render_frame)`.
     - Process capture frame via `aec3.process_capture_frame(&mut frame, false)`.
4. **Noise Suppression (`denoise.rs`):**
   - `NoiseSuppressionMode::Off`: pass-through.
   - `NoiseSuppressionMode::Rnnoise`:
     - Scale `frame[i] *= 32768.0` before calling `DenoiseState::process_frame(&mut out, &in_scaled)`.
     - Scale back `out[i] /= 32768.0`.
   - `NoiseSuppressionMode::DeepFilterNet3`:
     - Load ONNX model via `tract_onnx` from `./assets/models/deepfilternet3.onnx` (or user config dir).
     - If model file is absent, automatically fall back to `Rnnoise` and expose `df3_available: bool = false` to UI so the user sees a clean status badge rather than an error.
5. **Metering & Hotkey Gate:**
   - Compute RMS amplitude -> dBFS (`20.0 * rms.max(1e-7).log10()`) -> publish via `AtomicU32` (`f32::to_bits()`) for the UI speaking indicator (`threshold > -45.0 dBFS`).
   - Check hotkey state (`AtomicU8`):
     - `PushToTalk`: transmit only while PTT key is held.
     - `PushToMute`: transmit unless PTM key is held.
     - `ToggleMute`: transmit unless muted.
   - If gated/muted, skip Opus encoding and emit `AUDIO_DTX_KEEPALIVE` every 500 ms.
6. **Opus Encoding (`codec.rs`):**
   - `SampleRate::Hz48000`, `Channels::Mono`, `Application::Voip`.
   - Frame size: `480` samples (10 ms).
   - Bitrate: `40_000` bps, `DTX = true`, `InbandFec = true`, `PacketLossPerc = 5`.
   - If Opus returns `len <= 2` (DTX comfort noise / silence frame), suppress UDP send to save bandwidth.

### 5.3 Playback, Jitter Buffer & Mixer Pipeline
1. **Per-Peer Jitter Buffer (`jitter.rs`):**
   - Each remote `peer_id` has an isolated `JitterBuffer` holding decoded 480-sample `f32` frames indexed by `seq_num`.
   - Target jitter depth: **2 frames (20 ms)**, adaptive up to **4 frames (40 ms)** if packet jitter spikes.
   - If a packet is missing when the 10 ms tick fires, invoke Opus PLC (Packet Loss Concealment) via `decoder.decode(None, &mut out, false)` for up to 2 consecutive frames, then fade to `0.0`.
   - Track rolling 5-second packet loss percentage `(expected - received) / expected * 100.0` per peer.
2. **Multi-Peer Mixer (`mixer.rs`):**
   - Every 10 ms:
     - Sum `peer_frame[i] * peer_gain` (where `peer_gain` is `0.0..=2.0`, mapped smoothly from UI slider `0%..200%`).
     - Add active sound cue frame (from `cues.rs`) if a notification sound is currently playing.
     - Apply soft-clipper (`tanh(x)` when `|x| > 0.85`) to prevent digital clipping when multiple peers speak simultaneously.
     - Push mixed 480-sample frame into `aec_render_rb` (for AEC reference) and `playback_rb` (resampled back to output device rate if needed for `cpal` output stream).

### 5.4 Optional Sound Cues (`cues.rs` & `assets/sounds/README.md`)
- On startup, scan `./assets/sounds/` (and platform config dir `squad-air/sounds/`) for:
  - `join.wav` — local user joined a room
  - `leave.wav` — local user left a room
  - `peer_join.wav` — remote peer connected
  - `peer_leave.wav` — remote peer disconnected
  - `mute.wav` — microphone muted
  - `unmute.wav` — microphone unmuted
- Decode WAV via `hound`, convert to mono 48 kHz `f32`, store in `HashMap<CueEvent, Arc<Vec<f32>>>`.
- Missing or invalid files are silently ignored (`None`) with zero warnings/errors in UI.

---

## 6. Networking & NAT Hole Punching (`squad-rendezvous` & `squad-client/src/net`)

### 6.1 Rendezvous Server (`squad-rendezvous`)
- Single binary, listens on `0.0.0.0:3478` (configurable via `--bind`).
- Stores rooms in `DashMap<[u8; 16], RoomState>`.
- Max 10 peers per room; rejects 11th peer with `S2C_ERROR(RoomFull)`.
- Client sends `C2S_REGISTER` every **2.0 seconds** as heartbeat.
- Background cleanup task runs every **2.0 seconds** and evicts peers with `last_seen > 6.0s`, immediately broadcasting updated `S2C_ROOM_STATE` to remaining peers in that room.
- Basic anti-flood token bucket per source IP (max 30 packets/sec per IP).

### 6.2 Client Hole Punching & Mesh State Machine (`mesh.rs`)
- Binds a single dual-stack or IPv4 `tokio::net::UdpSocket` on `0.0.0.0:0` (ephemeral OS port). All Rendezvous traffic and all P2P traffic share this **exact same socket** so the NAT external port mapping observed by Rendezvous matches the port peers punch into!
- **Per-Peer Connection States:**
  1. `Punching { attempts: u32, started_at: Instant }`:
     - Sends encrypted `PUNCH_SYN` every **50 ms** to `peer.public_addr` AND all `peer.lan_addrs`.
     - Timeout after **12 seconds** -> transitions to `NatFailed` (displayed in UI as `"NAT Blocked"` badge).
  2. `Connected { active_addr: SocketAddr, rtt_ms: u32, loss_pct: f32, last_rx: Instant }`:
     - Triggered upon receiving valid decrypted `PUNCH_SYN` (replies `PUNCH_ACK`) or `PUNCH_ACK` from any candidate address; locks `active_addr` to the first responding endpoint.
     - Sends encrypted `PING` every **1000 ms**; updates EWMA `rtt_ms` upon receiving `PONG`.
     - If `last_rx > 6.0s`, drops peer back to `Punching` or removes if absent from `S2C_ROOM_STATE`.

---

## 7. UI/UX & Material Design 3 Dark Theme (`squad-client/src/ui`)

### 7.1 Material Design 3 Dark Color Tokens (`theme.rs`)
Implement a custom `iced::Theme` matching Material 3 Dark specifications:
- `Background`: `#111318`
- `Surface`: `#111318`
- `SurfaceContainerLow`: `#191C20`
- `SurfaceContainer`: `#1D2024`
- `SurfaceContainerHigh`: `#282A2F`
- `SurfaceContainerHighest`: `#33353A`
- `Primary`: `#A8C7FA` (Soft Material Blue-Lavender accent)
- `OnPrimary`: `#062E6F`
- `PrimaryContainer`: `#0842A0`
- `OnPrimaryContainer`: `#D3E3FD`
- `SecondaryContainer`: `#3B4858`
- `OnSecondaryContainer`: `#D7E3F7`
- `OnSurface`: `#E2E2E9`
- `OnSurfaceVariant`: `#C4C6D0`
- `Outline`: `#8E9099`
- `OutlineVariant`: `#44474F`
- `Error`: `#FFB4AB`
- `Success` (custom semantic): `#81C995` (Connected / low ping)
- `Warning` (custom semantic): `#FDD663` (Elevated ping / NAT punching)

### 7.2 Screens & Layout Geometry
- Default window size: `960 x 640` (min `760 x 520`), rounded cards (`16px` radius), pill-shaped action buttons (`9999px` radius), clean sans-serif typography.
- **Screen 1: Lobby (`lobby.rs`)**
  - Top bar: App title + current nickname chip + Gear icon button (opens Settings).
  - Center hero card (`SurfaceContainer`):
    - **"Create Room"** primary pill button -> generates 16B `room_id` + 32B `secret`, connects to Rendezvous, copies `voice://<room_id>#<secret>` to clipboard, transitions to Active Room.
    - **"Join Room"** text input (`Paste voice:// invite link...`) + tonal pill button **"Join"**.
- **Screen 2: Active Room (`room.rs`)**
  - Top header bar: Room status pill (`Connected • N/10`), **"Copy Invite Link"** pill button (with `"Copied!"` feedback state), **"Settings"** icon button, **"Leave"** red-tonal pill button.
  - Center Participant Grid (2 columns, up to 10 cards in `SurfaceContainer`):
    - Each card displays:
      - Circular avatar with animated `2.5px` `#81C995` / `#A8C7FA` border ring when `is_speaking == true`.
      - Nickname + status badge (`Punching...`, `NAT Blocked`, or live **`24 ms • 0.0%`** telemetry).
      - Per-peer Volume Slider (`0%` to `200%`, default `100%`) + mute speaker icon button; changes immediately update `mixer` and persist to `config.toml` keyed by `client_uuid`.
  - Bottom Floating Dock (`SurfaceContainerHigh`, pill-shaped):
    - Mic Mute toggle button, Deafen toggle button, active Hotkey mode indicator badge (`PTT` / `PTM` / `Toggle`), and live mic input level bar.
- **Screen 3: Settings Drawer/Modal (`settings.rs`)**
  - **Profile:** Nickname text field (auto-initialized on first boot to `<Adjective><Animal>-<3 digits>`, e.g., `SwiftFalcon-742`).
  - **Audio Devices:** Input Device dropdown, Output Device dropdown, Input Gain slider.
  - **Voice Processing (DSP):**
    - **Echo Cancellation (AEC):** Segmented pill switch (`Off` | `On`).
    - **Noise Suppression:** Segmented pill selector (`Off` | `RNNoise` | `DeepFilterNet3`).
  - **Hotkeys:** Keybind capture rows for `Toggle Mute`, `Push-to-Talk`, and `Push-to-Mute` (with Clear button).
  - **Network:** Rendezvous Server `host:port` input field.

---

## 8. Cross-Platform System Integration (Windows & macOS ARM64)
- **Config Path:** `dirs::config_dir()/squad-air/config.toml`.
- **System Tray (`tray.rs`):** Show/Hide window, Mute/Unmute Mic, Quit. Closing the window minimizes to tray while in an active call (optional toggle, default true).
- **Global Hotkeys (`hotkeys.rs`):**
  - Use `global-hotkey` crate integrated on the main event loop.
  - On **macOS**, check accessibility permissions gracefully (`AXIsProcessTrusted` via `macos-accessibility-client` or FFI) and display a non-blocking banner in Settings if permissions are needed for background PTT/PTM while CS2 is focused.

---

## 9. Phased Execution Plan for Orchestrator (Argon)

Execute sequentially in milestones, verifying compilation at the end of every milestone:

- **Milestone 1 — Workspace & `squad-proto`:**
  - Initialize workspace `Cargo.toml`, `assets/sounds/README.md`, `assets/models/README.md`.
  - Implement `squad-proto` (packet codecs, `ChaCha20-Poly1305` seal/open with deterministic nonce + AAD, `voice://` invite URI parser).
  - Write unit tests for packet roundtrips, tamper detection (bad AAD/tag), and invite URI parsing.
- **Milestone 2 — `squad-rendezvous`:**
  - Implement UDP Rendezvous server with `DashMap`, room capacity limit (10), peer TTL eviction, and rate limiter.
  - Write async Tokio tests simulating 10 clients registering, receiving peer lists, and timing out.
- **Milestone 3 — `squad-dsp`:**
  - Implement `resampler.rs`, `aec.rs`, `denoise.rs` (`Off`, `Rnnoise` with `32768.0` scaling, `DeepFilterNet3` via `tract-onnx` with fallback), `codec.rs` (Opus 10ms + DTX + FEC), `jitter.rs`, `mixer.rs`, `cues.rs`, and `engine.rs`.
  - Write DSP unit tests: sine-wave roundtrip through `AEC -> RNNoise -> Opus 10ms -> JitterBuffer -> Mixer`, verifying output SNR, frame length `480`, and missing-WAV graceful handling.
- **Milestone 4 — `squad-client` Networking & Config:**
  - Implement `config.rs` (auto-nickname generator, TOML persistence, per-`client_uuid` volume map), `nat.rs` (LAN IP discovery), and `mesh.rs` (single-socket Rendezvous heartbeat + concurrent WAN/LAN `PUNCH_SYN`/`PUNCH_ACK` + `PING`/`PONG` RTT & loss tracker).
- **Milestone 5 — `squad-client` Material 3 UI, Tray & Hotkeys:**
  - Implement `theme.rs` (M3 Dark palette), `components.rs`, `lobby.rs`, `room.rs`, `settings.rs`, `hotkeys.rs`, and `tray.rs`.
  - Wire UI state cleanly to `MeshController` and `AudioEngineHandle` via lock-free atomics and bounded `tokio::sync::mpsc` channels.
- **Milestone 6 — End-to-End Verification:**
  - Implement `tests/e2e_mesh_loopback.rs` spinning up an in-process Rendezvous server + 3 simulated encrypted Mesh peers exchanging synthetic 440 Hz / 880 Hz Opus audio frames and verifying decrypted mixed output, RTT measurement, and packet loss calculation.

---

## 10. Automated Verification Checklist (Must Pass Before Completion)

Run and verify every command below:
1. `cargo fmt --all -- --check` — formatting clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — zero compiler or clippy warnings.
3. `cargo test --workspace` — all unit tests in `squad-proto`, `squad-dsp`, `squad-rendezvous`, `squad-client`, and `tests/e2e_mesh_loopback.rs` pass.
4. `cargo build --release --bin squad-rendezvous --bin squad-client` — release binaries build cleanly on the host target (`aarch64-apple-darwin` or `x86_64-pc-windows-msvc`).
5. **Startup Smoke Test:** Launch `squad-rendezvous` and `squad-client` without any `.wav` files in `assets/sounds/` and without `deepfilternet3.onnx` in `assets/models/`; confirm zero panics, automatic fallback to `RNNoise`, and valid `config.toml` creation with a generated default nickname.