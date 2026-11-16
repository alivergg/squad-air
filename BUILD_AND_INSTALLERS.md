# Squad-Air Release Builds & Installers

All Squad-Air client releases **must** be packaged as minimalist, upgrade-in-place installers for **both macOS and Windows simultaneously** rather than distributing standalone executables.

## One-Command Release Build

Run from the workspace root:

```bash
./scripts/package_installers.sh
```

This builds release binaries for both platforms and writes the final deliverables into `dist/`:

- **`dist/Squad-Air-Installer-macos-arm64.pkg`** — macOS Apple Silicon `.pkg` installer
- **`dist/Squad-Air-Setup-windows-x86_64.exe`** — Windows x86_64 GUI setup executable
- **`dist/Squad-Air.app`** — Staged macOS application bundle

## Installer Behavior & In-Place Upgrades

### macOS (`Squad-Air-Installer-macos-arm64.pkg`)
- Built with native `/usr/bin/pkgbuild` (`com.squadair.client`, `BundleOverwriteAction = upgrade`, `BundleIsVersionChecked = false`).
- **Preinstall:** stops any running `squad-client` process cleanly before updating files.
- **Install / Upgrade:** installs or upgrades `/Applications/Squad-Air.app` directly without prompting for custom locations.
- **Postinstall:** clears `com.apple.quarantine` attributes (`xattr -cr`) and ensures executable permissions.
- **User Data:** preserves client settings under `~/Library/Application Support/squad-air/config.toml`.

### Windows (`Squad-Air-Setup-windows-x86_64.exe`)
- Built purely on macOS with `x86_64-w64-mingw32-windres` + `x86_64-w64-mingw32-gcc` from `scripts/windows_installer.rc` and `scripts/windows_installer.c` (embeds the release `squad-client.exe` via `RCDATA` resource `101`, no NSIS required).
- `crates/squad-client/src/main.rs` sets `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` so release builds launch without a background console window.
- **Install / Upgrade:** closes any running `Squad-Air.exe` or `squad-client.exe`, atomically writes `%LOCALAPPDATA%\Programs\Squad-Air\Squad-Air.exe` (via `MoveFileExW`), creates/updates the Start Menu shortcut, and registers an Add/Remove Programs entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\SquadAir`) with no UAC elevation required.
- **User Data:** preserves `%APPDATA%\squad-air\config.toml` across upgrades.
- **Post-install Prompts:** asks whether to create a Desktop shortcut (`Squad-Air.lnk`) and offers to launch Squad-Air immediately.

## Prerequisites on macOS Host
- Rust toolchain with `x86_64-pc-windows-gnu` target (`rustup target add x86_64-pc-windows-gnu`)
- MinGW-w64 (`brew install mingw-w64` providing `x86_64-w64-mingw32-gcc` and `x86_64-w64-mingw32-windres`)
- macOS Xcode Command Line Tools (`/usr/bin/pkgbuild`)
