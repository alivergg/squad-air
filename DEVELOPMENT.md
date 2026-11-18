# DEVELOPMENT.md — SquadAir Engineering, Security & Release Playbook

This document defines the mandatory development workflow, cybersecurity standards, testing gates, and release discipline for all future work on **SquadAir** (both human contributors and AI agents).

---

## 1. Branching & Release Workflow (`main` = Public `vX.X` Release)

Every push or merge to `origin/main` automatically runs:
- `.github/workflows/ci.yml` — formatting, Linux/macOS/Windows checks, Clippy (`-D warnings`), and full test suite.
- `.github/workflows/release.yml` — auto-increments the `vX.X` release tag (`v1.0` -> `v1.1` -> `v1.2`, ...) and builds/publishes installers for **macOS ARM64** (`.pkg`), **Windows x86_64** (`.exe` + `.zip`), and **Linux x86_64** (`.deb`, `.rpm`, Arch `.pkg.tar.zst` + `PKGBUILD`, `.tar.gz`) along with `SHA256SUMS.txt`.

### Mandatory Branching Rule
- **Never commit untested code directly onto `origin/main`.**
- Develop, refactor, and test all new features or fixes in a dedicated feature/testing branch (e.g. `dev`, `feat/<scope>-<name>`, `fix/<scope>-<name>`).
- Merge or push to `origin/main` **only** when the entire milestone is verified, formatted, documented, and ready to become the next public `vX.X` release.

---

## 2. Pre-Release Verification Gate

Before merging or pushing any branch into `main`, all of the following checks **must pass with zero warnings and zero failures**:

```bash
# 1. Formatting
cargo fmt --all -- --check

# 2. Compilation (workspace & all targets)
cargo check --workspace --all-targets --locked

# 3. Strict Lints (host + Windows GNU cross-check when targeting Windows packages)
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --target x86_64-pc-windows-gnu --locked -- -D warnings

# 4. Full Unit + E2E Loopback Test Suite
cargo test --workspace --locked

# 5. Packaging & Installer Sanity Checks
bash -n scripts/package_installers.sh
bash -n scripts/package_linux.sh
x86_64-w64-mingw32-gcc -fsyntax-only -Wall -Wextra scripts/windows_installer.c
```

Additional rules from `.agent/skills/squad-ait-rust-guardrails/SKILL.md`:
- Prefer `cargo check --workspace --message-format=short` for fast edit loops, then run full Clippy and `cargo test --workspace --locked` before finishing a milestone.
- Never assume desktop UI or network tests passed if they were skipped or sandbox-blocked; always verify full socket/loopback execution.

---

## 3. Cybersecurity & Privacy Standards (Zero-Trust P2P)

SquadAir processes untrusted UDP datagrams, peer-supplied chat/files/images, screen frames, and system audio streams for public end users. Every PR must satisfy these security invariants:

### A. Wire Protocol, Crypto & Anti-Replay (`squad-proto`, `squad-rendezvous`, `net/mesh.rs`)
- **Strict Bounds:** Enforce `MAX_UDP_PAYLOAD = 1200` and checked integer math (`checked_add` / `checked_mul` / `saturating_mul`) before any slice indexing or buffer allocation.
- **Authenticated Encryption:** All P2P payloads require `ChaCha20-Poly1305` AEAD sealing with `BLAKE3`-derived room keys and per-stream sliding `ReplayWindow` (`replay_control`, `replay_voice`, `replay_dtx`, `replay_stream_audio`).
- **Source & Leave Authorization:** Never bypass `src_addr` / `peer.active_addr` validation. Rendezvous `LEAVE` and relay forwarding must authenticate both `src_addr` and room membership; silent-drop invalid/rate-limited packets with zero amplification.
- **Secret Handling:** Redact invite/room secrets (`InviteLink`, `LanDiscoveryPacket`) in `Debug` output as `[REDACTED]` and zeroize secret key material on `Drop`. Enforce pre-decode URI caps on `squad://` / `voice://` invites.
- **Display Text Sanitization:** Always pass untrusted peer strings (nicknames, room names, filenames, titles, reasons) through `squad_proto::sanitize_display_str` to strip control, zero-width, and Bidi override characters.

### B. Untrusted Media, Video & Image Decoding (`squad-video`, `ui/avatar.rs`, `ui/mod.rs`)
- **Bounded Video & Deflate:** Enforce `MAX_VIDEO_DIM` (`4096`), `MAX_VIDEO_PIXELS` (`3840 * 2160`), `MAX_VIDEO_FRAGS` (`2048`), wraparound-safe serial numbers (`wrapping_sub`), pre-insertion `frag_idx` validation, and capped pending queues (`<= 8` frames). Never preallocate untrusted `expected_len` buffers before streaming decompression (`DeflateDecoder::take(expected_len + 1)`).
- **Bounded Image Decoding:** Decode all peer avatars, inline chat images, and clipboard images via `decode_bounded_image` using `image::ImageReader` + `image::Limits` (`max_image_width`, `max_image_height`, `max_alloc`). Evict unused UI image/avatar caches when scrolling/draining history.

### C. Filesystem, Config & OS Boundaries (`config.rs`, `net/transfer.rs`, `scripts/windows_installer.c`)
- **Path Traversal & File Transfers:** Sanitize all received filenames (`sanitize_filename`: strip slashes, control/Bidi chars, trailing dots/spaces, Windows reserved stems `CON`/`NUL`/`PRN`/`AUX`/`COM*`/`LPT*`), use `symlink_metadata().is_err()` for collision-free destination paths, validate `BlobOffer` invariants and seeder authorization (`blob.seeders.contains(&sender_peer_id)`), and enforce tight queue/download caps.
- **Config Persistence:** Back up unparseable non-empty config files (`*.toml.bak`) instead of silently overwriting, write via `.toml.tmp` + `sync_all()` + atomic rename, and lock down Unix permissions (`0o700` dir, `0o600` file).
- **Installer & FFI Safety:** Protect Windows installers with `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)`, validate `LOCALAPPDATA` lookup and `CoInitializeEx` `HRESULT`, use `u8 != 0` instead of `bool` in C/ObjC ABI `extern "C"` FFI signatures, and guard CoreAudio/GDI/CoreFoundation resource lifetimes.

### D. Real-Time Audio Safety (`squad-dsp`)
- Zero heap allocations, zero blocking mutexes, zero logging (`tracing::*`), and zero panics inside `cpal` real-time callbacks.
- Sanitize `NaN` / `Inf` samples in decoders, resamplers, denoisers, jitter PLC, and soft limiters.
- Keep asset loading self-contained (do not embed gitignored binaries via `include_bytes!`; a clean `git clone` must always compile).

### E. Repository Hygiene & Secrets
- Never commit API tokens, signing keys, certificates, user-specific home paths (use `${HOME}` / XDG / OS dirs), `.DS_Store` files, or build artifacts (`/target`, `/dist`).
- Before publishing releases or merging external PRs, scan diffs and check `Cargo.lock` changes cleanly under `--locked`.

---

## 4. Documentation & Commit Discipline

Whenever a change touches user-facing behavior, features, routing rules, CLI flags, dependencies, or packaging:
1. **Update Documentation in the Same PR/Milestone:**
   - [README.md](README.md) — features, architecture overview, install instructions, CLI flags, screenshots/branding.
   - [BUILD_AND_INSTALLERS.md](BUILD_AND_INSTALLERS.md) — build dependencies, packaging scripts, CI release outputs.
   - [PROJECT_MAP.md](PROJECT_MAP.md) & [SPECIFICATION.md](SPECIFICATION.md) — new crates/modules, protocol opcodes, architectural contracts.
2. **Atomic Conventional Commits:**
   - Follow `.agent/skills/git-milestone-commit/SKILL.md`.
   - Format: `<type>(<scope>): <short imperative summary>`
   - Allowed types: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `chore`, `style`
   - Allowed scopes: `proto`, `dsp`, `video`, `net`, `ui`, `config`, `e2e`, `workspace`, `readme`
   - One logical change per commit; run checks and tests before committing.
