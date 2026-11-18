# SquadAir Release Builds & Installers

SquadAir client releases are packaged as native installers and distro packages across **macOS**, **Windows**, and **Linux** (`.deb`, `.rpm`, `.pkg.tar.zst`, `.tar.gz`).

## Automated GitHub Releases (`vX.X`)

Every push or merge to `main` triggers [`.github/workflows/release.yml`](.github/workflows/release.yml):
1. Computes the next `vX.X` tag automatically (`v1.0` -> `v1.1` -> `v1.2`, ...) unless an explicit `tag` is passed via `workflow_dispatch`.
2. Builds all platforms in parallel:
   - **macOS ARM64:** `SquadAir-Installer-macos-arm64.pkg`, `squad-rendezvous-macos-arm64.tar.gz`
   - **Windows x86_64:** `SquadAir-Setup-windows-x86_64.exe`, `squad-rendezvous-windows-x86_64.zip`
   - **Linux x86_64:** `squad-air_<ver>_amd64.deb` (Ubuntu/Debian), `squad-air-<ver>-1.x86_64.rpm` (Fedora/RHEL), `squad-air-<ver>-1-x86_64.pkg.tar.zst` + `PKGBUILD` (Arch Linux), `squad-client-linux-x86_64.tar.gz`, `squad-rendezvous-linux-x86_64.tar.gz`
3. Publishes a new GitHub Release under tag `vX.X` with `SHA256SUMS.txt`.

## Manual Local Packaging

### macOS & Windows (on macOS Host)

Run from the workspace root:

```bash
./scripts/package_installers.sh
```

Outputs in `dist/`:
- `dist/SquadAir-Installer-macos-arm64.pkg` — macOS Apple Silicon `.pkg` installer
- `dist/SquadAir-Setup-windows-x86_64.exe` — Windows x86_64 GUI setup executable
- `dist/SquadAir.app` — Staged macOS application bundle

Prerequisites on macOS host:
- Rust toolchain with `x86_64-pc-windows-gnu` target (`rustup target add x86_64-pc-windows-gnu`)
- MinGW-w64 (`brew install mingw-w64` providing `x86_64-w64-mingw32-gcc` and `x86_64-w64-mingw32-windres`)
- macOS Xcode Command Line Tools (`/usr/bin/pkgbuild`)

### Linux Distro Packages (on Linux x86_64 Host)

Install system dependencies (`cmake`, `nasm`, `pkg-config`, `libopus-dev`, `libasound2-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, `libx11-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libxdo-dev`, `libxkbcommon-dev`) and packaging tools (`dpkg-dev`, `rpm`, `fakeroot`, `libarchive-tools`, `zstd`), then run:

```bash
cargo build --release --locked -p squad-rendezvous -p squad-client
./scripts/package_linux.sh 1.0
```

Outputs in `dist/`:
- `dist/squad-air_1.0_amd64.deb` (Ubuntu / Debian)
- `dist/squad-air-1.0-1.x86_64.rpm` (Fedora / RHEL / openSUSE)
- `dist/squad-air-1.0-1-x86_64.pkg.tar.zst` and `dist/PKGBUILD` (Arch Linux)
- `dist/squad-client-linux-x86_64.tar.gz` and `dist/squad-rendezvous-linux-x86_64.tar.gz`

## Installer Behavior & In-Place Upgrades

### macOS (`SquadAir-Installer-macos-arm64.pkg`)
- Built with native `/usr/bin/pkgbuild` (`com.squadair.client`, `BundleOverwriteAction = upgrade`, `BundleIsVersionChecked = false`).
- **Preinstall:** stops any running `squad-client` process cleanly before updating files.
- **Install / Upgrade:** installs or upgrades `/Applications/SquadAir.app` directly without prompting for custom locations.
- **Postinstall:** clears `com.apple.quarantine` attributes (`xattr -cr`) and ensures executable permissions.
- **User Data:** preserves client settings under `~/Library/Application Support/squad-air/config.toml`.

### Windows (`SquadAir-Setup-windows-x86_64.exe`)
- Built with `x86_64-w64-mingw32-windres` + `x86_64-w64-mingw32-gcc` from `scripts/windows_installer.rc` and `scripts/windows_installer.c` (embeds release `squad-client.exe` via `RCDATA` resource `101`, no NSIS required).
- **Install / Upgrade:** stops any running `SquadAir.exe`, `Squad-Air.exe`, or `squad-client.exe`, atomically writes `%LOCALAPPDATA%\Programs\SquadAir\SquadAir.exe` (via `MoveFileExW`), creates/updates the Start Menu shortcut, and registers an Add/Remove Programs entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\SquadAir`) without UAC elevation.
- **User Data:** preserves `%APPDATA%\squad-air\config.toml` across upgrades.
