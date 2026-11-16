---
name: squad-rust-guardrails
description: Strict API rules for iced 0.13, ringbuf 0.4, nnnoiseless, and real-time audio in SquadAir.
---
# Rust & Crate Guardrails for SquadAir

1. **iced 0.13 API (NO legacy traits):**
   - NEVER implement `iced::Sandbox` or `iced::Application` (removed in 0.13).
   - ALWAYS launch via `iced::application(App::title, App::update, App::view).subscription(App::subscription).theme(App::theme).window_size((960.0, 640.0)).run_with(App::new)`.
2. **ringbuf 0.4 Traits:**
   - Always import `use ringbuf::traits::{Consumer, Observer, Producer, Split};` when calling `.split()`, `.try_push()`, or `.try_pop()` on `HeapRb`.
3. **Real-Time Audio Safety (`cpal`):**
   - Zero heap allocations, zero `.lock()`, and zero logging inside `cpal` stream callbacks.
4. **nnnoiseless Sample Range:**
   - `nnnoiseless::DenoiseState` expects `f32` scaled to `[-32768.0, 32767.0]`, NOT `[-1.0, 1.0]`. Multiply by `32768.0` before `process_frame` and divide by `32768.0` after.
5. **Fast Compiler Loop:**
   - Always run `cargo check --workspace --message-format=short` before running full `cargo test` or `cargo clippy`.
6. **Release Packaging & Installers (macOS & Windows):**
   - NEVER deliver raw standalone binaries (`squad-client` / `squad-client.exe`) as final release artifacts.
   - ALWAYS package both platforms simultaneously by running `./scripts/package_installers.sh`.
   - The script compiles macOS ARM64 (`cargo build --release`) and Windows x86_64 (`cargo build --release --target x86_64-pc-windows-gnu`) and builds minimalist, upgrade-in-place installers in `dist/`:
     - `dist/Squad-Air-Installer-macos-arm64.pkg` — upgrades `/Applications/Squad-Air.app` directly (preserving user settings).
     - `dist/Squad-Air-Setup-windows-x86_64.exe` — upgrades `%LOCALAPPDATA%\Programs\Squad-Air\Squad-Air.exe` in place, preserves `config.toml`, and prompts to create a Desktop shortcut.
   - See [BUILD_AND_INSTALLERS.md](../../../BUILD_AND_INSTALLERS.md) for full packaging details.