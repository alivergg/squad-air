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