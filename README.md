# SquadAir

[![CI](https://img.shields.io/github/actions/workflow/status/squad-air/squad-air/ci.yml?branch=main&label=CI&style=flat-square)](.github/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-edition%202021-orange?style=flat-square&logo=rust)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20ARM64%20%7C%20Windows%20x86__64%20%7C%20Linux%20x86__64-informational?style=flat-square)](#3-step-quick-start)

**SquadAir** is a lightweight, end-to-end encrypted P2P voice chat, screen sharing, and squad messaging desktop app built in Rust for low-latency gaming. Audio, video, and chat travel directly peer-to-peer over a single UDP socket — no central media server required.

---

## Key Features

- **10 ms Opus Voice Pipeline:** 48 kHz mono Opus encoding (`audiopus`) in strict 10 ms (480-sample) frames with selectable bitrates (**24 / 40 / 64 / 96 kbps**), Discontinuous Transmission (DTX) silence gating, in-band Forward Error Correction (FEC), Packet Loss Concealment (PLC), and selectable jitter buffer latency profiles (**LAN 10 ms**, **Low 20 ms**, **Balanced 30 ms**, **Wi-Fi 50 ms**).
- **Direct LAN / P2P Encrypted Mesh:** Concurrent WAN and local IPv4 (`if-addrs`) UDP hole punching connects up to 10 peers per room (`MAX_ROOM_PEERS = 10`) directly over UDP, with live **LAN / P2P** route, RTT, buffer, and packet-loss telemetry badges on each peer card. All P2P payloads are encrypted with **ChaCha20-Poly1305** AEAD using keys derived via **BLAKE3** from the invite secret.
- **Studio-Grade Voice DSP (`squad-dsp`):**
  - Lock-free `cpal` audio I/O connected to a dedicated real-time DSP thread via `ringbuf` SPSC ring buffers and `rubato` resampling.
  - Selectable noise suppression: **Off**, built-in **RNNoise** (`nnnoiseless`), or optional **DeepFilterNet3** ONNX (`tract-onnx`).
  - Toggleable **WebRTC AEC3** acoustic echo cancellation (`aec3`), **Hear Yourself** mic loopback test in Settings, adjustable input gain & VAD threshold, comfort-calibrated voice mixing with a smooth soft peak limiter, per-peer volume sliders (`0%–200%`), and optional WAV audio cues.
- **Hardware-Accelerated Screen Sharing (`squad-video`):** Capture displays or application windows at **720p / 1080p / Native** and **30 / 60 FPS** in **H.264** or **H.265 (HEVC)** with hardware backend detection (**Apple VideoToolbox** on macOS; **NVIDIA NVENC**, **AMD AMF / VCN**, **Intel QSV**, and **Windows MFT** on Windows), packet fragmentation, and XOR FEC.
- **Click-Through In-Game Overlay:** Always-on-top transparent overlay (`overlay_platform.rs`) using native AppKit passthrough flags on macOS and Win32 layered/transparent flags on Windows, with configurable opacity and speaking-only filtering.
- **Global Hotkeys (Keybind Recorder + Mouse4 / Mouse5):** Click-to-record keybindings in Settings for **Push-to-Mute** (`KeyV`), **Push-to-Talk** (`CapsLock`), **Toggle Mute** (`F9`), and **Toggle Deafen** (`F10`), including side mouse buttons (`Mouse3`, `Mouse4` / `XButton1`, `Mouse5` / `XButton2`).
- **E2EE Room Chat, Saved Channels & System Tray:** Encrypted room text chat, inline image previews (up to 200 KiB), chunked P2P file transfers, saved `squad://` channel bookmarks, custom/identicon avatars, and minimize-to-tray support.

---

## 3-Step Quick Start

1. **Download & Install**
   - **macOS (Apple Silicon):** Download `SquadAir-Installer-macos-arm64.pkg` from Releases and run it. Installs/upgrades `/Applications/SquadAir.app` in place and preserves `~/Library/Application Support/squad-air/config.toml`.
   - **Windows (x86_64):** Download `SquadAir-Setup-windows-x86_64.exe` from Releases and run it. Installs/upgrades `%LOCALAPPDATA%\Programs\SquadAir\SquadAir.exe` with no UAC elevation required and preserves `%APPDATA%\squad-air\config.toml`.
   - **Linux (x86_64):** Download `squad-client-linux-x86_64.tar.gz` or [build from source](#build-from-source).

2. **Create a Room & Share the Invite Link**
   - Open SquadAir, click **Create Room** to generate a cryptographic `squad://<room_id>#<secret>` link (legacy `voice://` links are also supported), and send it to your squad.
   - To join a friend's room, paste their `squad://` link in the lobby and click **Join**.

3. **Configure Voice & Play**
   - Open **Settings** to pick your microphone/headset, run **Hear Yourself** mic loopback, choose **RNNoise** or **DeepFilterNet3**, select your jitter latency profile (**10–50 ms**) & Opus bitrate, click-to-bind **Push-to-Talk / Push-to-Mute / Toggle Mute / Toggle Deafen** (including `Mouse4` / `Mouse5`), and enable the click-through **In-Game Overlay**.
   - Set **Voice Server** to your self-hosted VPS or LAN `squad-rendezvous` address (`host:3478`, default `127.0.0.1:3478`).

---

## Architecture & Latency Breakdown

SquadAir is organized as a 5-crate Rust workspace:

| Crate | Role |
| :--- | :--- |
| `crates/squad-proto` | Binary wire protocol (`MAX_UDP_PAYLOAD = 1200`), `BLAKE3` + `ChaCha20-Poly1305` AEAD, `squad://` invite URI parser |
| `crates/squad-rendezvous` | Stateless-disk async UDP hole-punching coordinator (`--bind 0.0.0.0:3478`), 6s peer TTL, 2s reaper, per-IP token-bucket rate limiter |
| `crates/squad-dsp` | Lock-free `cpal` + `ringbuf` audio engine, `rubato` resampler, `aec3`, `nnnoiseless` / `tract-onnx`, 10 ms Opus codec, adaptive jitter buffer, mixer |
| `crates/squad-video` | Screen/window capture, H.264/HEVC encoder & decoder, hardware backend detection, MTU packetizer + XOR FEC |
| `crates/squad-client` | `iced` 0.13 M3 Dark UI, click-through overlay, global hotkeys, system tray, and single-socket Tokio P2P mesh |

### Real-Time Audio & Network Topology

```text
[cpal Input] --(lock-free HeapRb<f32>)--> [DSP Thread]
                                             ├─ rubato -> 48 kHz mono (480 samples / 10 ms)
                                             ├─ WebRTC AEC3 (reads far-end HeapRb<f32>)
                                             ├─ Noise Suppression (Off | RNNoise | DeepFilterNet3)
                                             ├─ Opus 10 ms encode (DTX + In-band FEC)
                                             └─(mpsc)--> [Tokio P2P Mesh]
                                                           ├─ ChaCha20-Poly1305 seal
                                                           └─ UDP send_to (direct WAN/LAN peers)
```

Because `squad-rendezvous` only exchanges peer addresses during room join and hole punching, every voice packet, video frame, and chat message flows **directly between peers** with zero media relay hops, zero audio-callback locks or heap allocations, and strict 10 ms Opus framing.

---

## Self-Hosting & LAN Mode

`squad-rendezvous` only coordinates peer discovery and UDP hole punching — it never sees room secrets (`#secret` stays client-side), decrypted audio, video, or chat.

### Option A: Self-Host on a VPS with Docker Compose

1. Clone the repository on your server and start the rendezvous container:

   ```bash
   docker compose up -d
   ```

2. Open **UDP port 3478** in your server firewall (and cloud security group):

   ```bash
   sudo ufw allow 3478/udp
   ```

3. In the SquadAir client, open **Settings -> Voice Server** and enter `<your-server-ip>:3478`.

To inspect logs or customize the bind port:

```bash
docker compose logs -f squad-rendezvous
```

### Option B: Pure LAN Mode (No Internet Required)

Run `squad-rendezvous` on any machine on your local network:

```bash
squad-rendezvous --bind 0.0.0.0:3478
```

Point every player's **Settings -> Voice Server** to `<host-lan-ip>:3478` (or `127.0.0.1:3478` on the host machine itself). Clients advertise their local IPv4 interfaces (`if-addrs`) during registration and connect directly over LAN.

---

## Optional DeepFilterNet3 Model

To use **DeepFilterNet3** neural noise suppression in **Settings**:
1. Place the exported ONNX model at `./assets/models/deepfilternet3.onnx` or inside your OS config directory under `squad-air/models/deepfilternet3.onnx` (see [`assets/models/README.md`](assets/models/README.md)).
2. If the file is missing, SquadAir automatically falls back to built-in **RNNoise**.

---

## Build from Source

### Prerequisites

- **Rust toolchain** (stable)
- **Linux (Debian / Ubuntu)** system libraries:
  ```bash
  sudo apt-get update && sudo apt-get install -y --no-install-recommends \
    cmake nasm pkg-config libopus-dev libasound2-dev libgtk-3-dev \
    libayatana-appindicator3-dev libx11-dev libxcb-shape0-dev \
    libxcb-xfixes0-dev libxdo-dev libxkbcommon-dev
  ```

### Build Binaries

```bash
# Desktop client
cargo build --release --locked -p squad-client

# Rendezvous signaling server
cargo build --release --locked -p squad-rendezvous

# Run workspace tests
cargo test --workspace --locked
```

### Build macOS & Windows Installers

On a macOS host with `mingw-w64` and the `x86_64-pc-windows-gnu` Rust target installed (see [`BUILD_AND_INSTALLERS.md`](BUILD_AND_INSTALLERS.md)):

```bash
./scripts/package_installers.sh
```

This outputs both installers into `dist/`:
- `dist/SquadAir-Installer-macos-arm64.pkg`
- `dist/SquadAir-Setup-windows-x86_64.exe`
- `dist/SquadAir.app`
