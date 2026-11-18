<p align="center">
  <img src="assets/icons/icon.png" alt="SquadAir Logo" width="128" height="128" />
</p>

<h1 align="center">SquadAir</h1>

<p align="center">
  Lightweight, end-to-end encrypted P2P voice chat, hardware-accelerated screen sharing, and squad messaging built in Rust for low-latency gaming.
</p>

<p align="center">
  <a href=".github/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/alivergg/squad-air/ci.yml?branch=main&label=CI&style=flat-square" alt="CI" /></a>
  <a href=".github/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/alivergg/squad-air/release.yml?branch=main&label=Release&style=flat-square" alt="Release" /></a>
  <a href="Cargo.toml"><img src="https://img.shields.io/badge/rust-edition%202021-orange?style=flat-square&logo=rust" alt="Rust" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License: MIT" /></a>
  <a href="#3-step-quick-start"><img src="https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Debian%2FUbuntu%20%7C%20Fedora%20%7C%20Arch-informational?style=flat-square" alt="Platforms" /></a>
</p>

Audio, video, and chat travel **directly peer-to-peer over a single UDP socket** with ChaCha20-Poly1305 encryption — zero media servers required.

---

## Key Features

- **10 ms Opus Voice Pipeline:** 48 kHz mono Opus encoding (`audiopus`) in strict 10 ms (480-sample) frames with selectable bitrates (**24 / 40 / 64 / 96 kbps**), Discontinuous Transmission (DTX) silence gating, in-band Forward Error Correction (FEC), Packet Loss Concealment (PLC), and selectable jitter buffer latency profiles (**LAN 10 ms**, **Low 20 ms**, **Balanced 30 ms**, **Wi-Fi 50 ms**).
- **3-Tier Hybrid Encrypted Mesh Routing (`LAN / P2P -> Peer-Mesh -> Opt-in Server Relay`):**
  1. **Priority 1 — Direct LAN / WAN P2P:** Concurrent local IPv4 (`if-addrs`) and WAN UDP hole punching connects up to 10 peers per room (`MAX_ROOM_PEERS = 10`) directly with zero server load.
  2. **Priority 2 — 1-Hop Peer-Mesh Relay:** When two players cannot punch through strict NAT directly, traffic automatically routes through a teammate who can reach both peers directly (`MeshReachability`), keeping central server bandwidth at zero.
  3. **Priority 3 — Opt-in Server Relay (Voice & Chat Only):** If a `squad-rendezvous` host explicitly passes `--enable-relay`, NAT-blocked peers fall back to zero-amplification UDP relay for voice & chat only (heavy video streams stay P2P / Peer-Mesh only) and transparently upgrade back to direct P2P as soon as hole punching succeeds.
  - All payloads are authenticated and encrypted with **ChaCha20-Poly1305** AEAD (`BLAKE3` key derivation + sliding `ReplayWindow`), with live **LAN / P2P / Mesh / Relay** route, RTT, buffer, and loss telemetry on every peer card.
- **Zero-Config LAN Rooms & Host Migration:** Click **Host LAN Room** in the lobby to launch an embedded `squad-rendezvous` coordinator inside the client and broadcast the room over UDP LAN discovery (**LAN Rooms Nearby**) — no internet or separate server binary required. If the host leaves, another peer automatically promotes its embedded coordinator and preserves the room.
- **Studio-Grade Voice DSP (`squad-dsp`):**
  - Lock-free `cpal` audio I/O connected to a dedicated real-time DSP thread via `ringbuf` SPSC ring buffers and `rubato` resampling, with automatic **CoreAudio / WASAPI hot-plug recovery** (OS default switches, unplug fallback, preferred-device replug recovery, Bluetooth HFP profile transitions, and live Settings dropdown updates).
  - Selectable noise suppression: **Off**, built-in **RNNoise** (`nnnoiseless`), or optional **DeepFilterNet3** ONNX (`tract-onnx`).
  - Toggleable **WebRTC AEC3** acoustic echo cancellation (`aec3`), **Hear Yourself** mic loopback test in Settings, adjustable input gain & VAD threshold, calibrated voice mixing with a soft peak limiter, per-peer volume sliders (`0%–200%`), and synthesized 48 kHz WAV audio cues.
- **Hardware-Accelerated Screen Sharing (`squad-video`):** Capture displays or windows at **720p / 1080p / Native** and **30 / 60 FPS** in **H.264** or **H.265 (HEVC)** with backend detection (**Apple VideoToolbox** on macOS; **NVIDIA NVENC**, **AMD AMF / VCN**, **Intel QSV**, and **Windows MFT** on Windows), packet fragmentation, and XOR FEC.
- **Click-Through In-Game Overlay:** Always-on-top transparent HUD (`overlay_platform.rs`) using native AppKit passthrough flags on macOS and Win32 layered/transparent flags on Windows, with configurable opacity and speaking-only filtering.
- **Global Hotkeys (Keybind Recorder + Mouse4 / Mouse5):** Click-to-record keybindings in Settings for **Push-to-Mute** (`KeyV`), **Push-to-Talk** (`CapsLock`), **Toggle Mute** (`F9`), and **Toggle Deafen** (`F10`), including side mouse buttons (`Mouse3`, `Mouse4` / `XButton1`, `Mouse5` / `XButton2`).
- **E2EE Room Chat, Saved Channels & System Tray:** Encrypted room text chat, bounded inline image previews, chunked P2P file transfers, saved `squad://` channel bookmarks, custom/identicon avatars, atomic `0600` config persistence, and minimize-to-tray support.

---

## 3-Step Quick Start

### 1. Download & Install ([Latest Release](https://github.com/alivergg/squad-air/releases/latest))

- **macOS (Apple Silicon):**
  Download `SquadAir-Installer-macos-arm64.pkg` and open it. Upgrades `/Applications/SquadAir.app` in place and preserves `~/Library/Application Support/squad-air/config.toml`.
- **Windows (x86_64):**
  Download `SquadAir-Setup-windows-x86_64.exe` and run it. Installs/upgrades `%LOCALAPPDATA%\Programs\SquadAir\SquadAir.exe` with no UAC elevation required and preserves `%APPDATA%\squad-air\config.toml`.
- **Ubuntu / Debian (`.deb`):**
  ```bash
  sudo apt install ./squad-air_*_amd64.deb
  ```
- **Fedora / RHEL / openSUSE (`.rpm`):**
  ```bash
  sudo dnf install ./squad-air-*.x86_64.rpm
  ```
- **Arch Linux (`.pkg.tar.zst` / `PKGBUILD`):**
  ```bash
  sudo pacman -U ./squad-air-*-x86_64.pkg.tar.zst
  ```
  Or build directly from the release `PKGBUILD` with `makepkg -si`.
- **Portable Linux (`.tar.gz`):**
  Extract `squad-client-linux-x86_64.tar.gz` and run `./squad-client`.

### 2. Create a Room & Share the Invite Link

- Open SquadAir, click **Create Room** to generate a cryptographic `squad://<room_id>#<secret>` link (legacy `voice://` links are also supported), and send it to your squad — or click **Host LAN Room** to auto-announce a zero-config local room.
- To join a friend's room, paste their `squad://` link in the lobby or pick a room under **LAN Rooms Nearby** and click **Join**.

### 3. Configure Voice & Play

- Open **Settings** to pick your microphone/headset, run **Hear Yourself** mic loopback, choose **RNNoise** or **DeepFilterNet3**, select your jitter latency profile (**10–50 ms**) & Opus bitrate, bind **Push-to-Talk / Push-to-Mute / Toggle Mute / Toggle Deafen** (including `Mouse4` / `Mouse5`), and enable the click-through **In-Game Overlay**.
- Set **Voice Server** to your self-hosted VPS or LAN `squad-rendezvous` address (`host:3478`, default `127.0.0.1:3478`).

---

## Architecture & Latency Breakdown

SquadAir is organized as a 5-crate Rust workspace:

| Crate | Role |
| :--- | :--- |
| `crates/squad-proto` | Binary wire protocol (`MAX_UDP_PAYLOAD = 1200`), `BLAKE3` + `ChaCha20-Poly1305` AEAD, `ReplayWindow`, `squad://` invite URI parser |
| `crates/squad-rendezvous` | Async UDP hole-punching coordinator (`--bind 0.0.0.0:3478`, optional `--enable-relay` for voice/chat only), 6s peer TTL, 2s reaper, per-IP token-bucket rate limiter |
| `crates/squad-dsp` | Lock-free `cpal` + `ringbuf` audio engine, `rubato` resampler, `aec3`, `nnnoiseless` / `tract-onnx`, 10 ms Opus codec, adaptive jitter buffer, soft-limited mixer |
| `crates/squad-video` | Bounded screen/window capture, H.264/HEVC encoder & decoder, hardware backend detection, MTU packetizer + XOR FEC |
| `crates/squad-client` | `iced` 0.13 M3 Dark UI, click-through overlay, global hotkeys, system tray, file transfer, and single-socket Tokio 3-tier P2P mesh |

### Real-Time Audio & Network Topology

```text
[cpal Input] --(lock-free HeapRb<f32>)--> [DSP Thread]
                                             ├─ rubato -> 48 kHz mono (480 samples / 10 ms)
                                             ├─ WebRTC AEC3 (reads far-end HeapRb<f32>)
                                             ├─ Noise Suppression (Off | RNNoise | DeepFilterNet3)
                                             ├─ Opus 10 ms encode (DTX + In-band FEC)
                                             └─(mpsc)--> [Tokio 3-Tier Mesh]
                                                           ├─ ChaCha20-Poly1305 AEAD seal
                                                           └─ Priority 1: Direct LAN / WAN P2P
                                                              Priority 2: 1-Hop Peer-Mesh Relay
                                                              Priority 3: Server Relay (voice/chat only, --enable-relay)
```

---

## Self-Hosting & LAN Mode

`squad-rendezvous` never sees room secrets (`#secret` stays client-side), decrypted audio, video, or chat.

### Option A: Self-Host on a VPS with Docker Compose

1. Clone the repository on your server and start the hardened read-only container:

   ```bash
   docker compose up -d
   ```

   By default `squad-rendezvous` runs in pure coordination mode (`0` media relay bandwidth). To allow Priority 3 voice & chat relay fallback for peers behind symmetric NAT, pass `--enable-relay` in `docker-compose.yml` (`command: ["--bind", "0.0.0.0:3478", "--enable-relay"]`). Video streams are always blocked at the server relay boundary.

2. Open **UDP port 3478** in your server firewall (and cloud security group):

   ```bash
   sudo ufw allow 3478/udp
   ```

3. In the SquadAir client, open **Settings -> Voice Server** and enter `<your-server-ip>:3478`.

### Option B: Pure LAN Mode (No Internet Required)

1. Click **Host LAN Room** in the Lobby — the client starts an embedded `squad-rendezvous` coordinator locally and announces the room across your subnet.
2. Friends on the same network see the room in **LAN Rooms Nearby** and click **Join** (or join via its `squad://` invite link).
3. If the host leaves, another connected peer automatically promotes its embedded coordinator without closing the room.

Standalone binary option:

```bash
squad-rendezvous --bind 0.0.0.0:3478
# Add --enable-relay to allow voice/chat relay fallback:
squad-rendezvous --bind 0.0.0.0:3478 --enable-relay
```

---

## Optional DeepFilterNet3 Model

To use **DeepFilterNet3** neural noise suppression in **Settings**:
1. Place the exported ONNX model at `./assets/models/deepfilternet3.onnx` or inside your OS config directory under `squad-air/models/deepfilternet3.onnx` (see [`assets/models/README.md`](assets/models/README.md)).
2. If the file is missing, SquadAir automatically falls back to built-in **RNNoise**.

---

## Build from Source & Packaging

### Prerequisites (Ubuntu / Debian)

```bash
sudo apt-get update && sudo apt-get install -y --no-install-recommends \
  cmake nasm pkg-config libopus-dev libasound2-dev libgtk-3-dev \
  libayatana-appindicator3-dev libx11-dev libxcb-shape0-dev \
  libxcb-xfixes0-dev libxdo-dev libxkbcommon-dev
```

### Build & Test

```bash
# Desktop client
cargo build --release --locked -p squad-client

# Rendezvous signaling server
cargo build --release --locked -p squad-rendezvous

# Run workspace tests
cargo test --workspace --locked
```

### Build Native Installers & Automated `vX.X` Releases

- **macOS & Windows installers** (from macOS with `mingw-w64` + `x86_64-pc-windows-gnu`):
  ```bash
  ./scripts/package_installers.sh
  ```
- **Linux packages (`.deb`, `.rpm`, Arch `.pkg.tar.zst` + `PKGBUILD`, `.tar.gz`)** (after building release binaries on Linux with `dpkg-dev`, `rpm`, `fakeroot`, `libarchive-tools`, `zstd`):
  ```bash
  ./scripts/package_linux.sh 1.0
  ```
- **Automated Multi-Platform Releases:** Every push or merge into `main` automatically runs [`.github/workflows/release.yml`](.github/workflows/release.yml), increments the release tag (`v1.0`, `v1.1`, `v1.2`, ...), builds installers and packages for **macOS ARM64**, **Windows x86_64**, **Ubuntu/Debian**, **Fedora**, **Arch Linux**, and **portable Linux**, and publishes them with `SHA256SUMS.txt` under GitHub Releases. See [`BUILD_AND_INSTALLERS.md`](BUILD_AND_INSTALLERS.md) for details.
