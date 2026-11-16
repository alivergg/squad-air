---
name: git-milestone-commits
description: Enforce clean, atomic Conventional Commits after completing each implementation stage or milestone in squad-air.
---

# Git Milestone Auto-Commit Workflow

You MUST create a clean Git commit after completing **every individual stage/milestone** of a task (do not lump multiple stages into a single giant commit).

## 1. Repository Bootstrap Check
Before making your first commit in a workspace:
1. Check if `.git` exists (`git status`). If not, run `git init` and ensure `.gitignore` exists and excludes:
   - `/target`
   - `**/*.rs.bk`
   - `.DS_Store`
   - `assets/sounds/*.wav` (optional local user audio)
   - `assets/models/*.onnx` (optional local ML weights)

## 2. Pre-Commit Verification Gate
NEVER commit code that does not compile:
1. Run `cargo check --workspace --message-format=short` (and `cargo test` if the stage added tests).
2. Run `git status -s` to verify no temporary files, build artifacts, or secrets are staged.

## 3. Atomic Commit Rules & Format
Stage only the files belonging to the just-completed stage (`git add <paths>`) and commit using **Conventional Commits**:

```text
<type>(<scope>): <concise imperative description>

- <bullet 1: key architectural or protocol change>
- <bullet 2: key module or test added>

Allowed Types & Scopes

    Types: feat, fix, refactor, test, chore, docs

    Scopes: proto, dsp, video, net, ui, config, e2e, workspace

Good Examples

    feat(proto): add AvatarAnnounce packet and BLAKE3 hash exchange

    feat(ui): implement deterministic 5x5 BLAKE3 identicon generator and circular mask

    feat(net): sync custom 128x128 avatars via Mini-Swarm chunks with disk cache

    test(e2e): verify P2P avatar transfer and BLAKE3 cache hit