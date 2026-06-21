# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Workspace layout

The Cargo workspace is at the repo root. Two members:

- `chip8-core` — the CHIP-8 virtual machine. Pure Rust, **zero dependencies**, knows nothing
  about eframe/the browser/filesystem/time. Unit-testable in isolation.
- `chip8-app` — the shell: an `eframe` (egui) UI plus native and web (wasm) entry points.
  Depends on `chip8-core` and `eframe`.

## Commands

Run from the repo root:

- Test the VM core: `cargo test -p chip8-core`
- Run native app: `cargo run -p chip8-app`
- Web dev server: `cd chip8-app && trunk serve` → http://localhost:8080 (trunk 0.21 is installed).
- Web release build: `cd chip8-app && trunk build --release`
- **Run trunk from inside `chip8-app/`, not the repo root** — the root is a virtual workspace with no
  package, so trunk fails with "could not find the root package". Plain wasm compile-check (no bundle):
  `cargo build -p chip8-app --target wasm32-unknown-unknown`.

## Architecture & gotchas

- **All 16 opcode families are implemented.** The decoder in `chip8-core/src/lib.rs` is a `match`
  on the top nibble. `self.unimplemented(opcode)` (a **deliberate silent no-op**) now only catches
  *invalid* sub-opcodes within the `0x0/0x5/0x8/0xE/0xF` families. When debugging, temporarily
  `panic!` in `unimplemented` to surface a malformed opcode instead of silently ignoring it.
- **Two convention choices** that have an older alternative, in case a ROM misbehaves: `0x8` shifts
  (`8XY6`/`8XYE`) shift `Vx` in place (modern CHIP-48/SUPER-CHIP) rather than `Vx = Vy >> 1`; and
  `FX55`/`FX65` leave `I` unchanged rather than incrementing it. Both are commented at the call site.
- **RNG (`CXNN`) is deterministic by default** — the core has no clock, so it seeds a fixed xorshift
  in `new()`. The shell calls `reseed(u32)` after `load_rom()` to inject entropy.
- **Timers run at exactly 60 Hz, independent of the CPU clock.** `tick_timers()` must be called
  60×/sec regardless of how often `step()` runs (the app does ~10 `step()`s per frame). Conflating
  the two clocks is the classic CHIP-8 bug.
- **`VF` (register `v[0xF]`) is the flag register** — carry/borrow/shift-out for the `0x8` ALU
  family, and the collision flag set by `DXYN` draw. Get the `0x8` family's flag semantics right
  and most games work.
- **eframe web entry point is version-fragile.** The wasm `main` in `chip8-app` mirrors the
  `eframe_template` for eframe 0.31. If eframe is upgraded, regenerate from the template and swap
  `Chip8App` back in.
- ROM loading: a "Load ROM…" button (`rfd` file picker) loads real `.ch8`/`.rom` files; native runs
  the blocking dialog on a worker thread, web uses the async dialog + `spawn_local`, both dropping
  bytes into a shared `pending_rom` slot that `update()` drains (rebuilding the VM + `reseed`-ing it).
  `DEMO_ROM` is still the boot ROM shown before anything is loaded.
- Audio: a `Beeper` plays a 440 Hz square wave while `is_beeping()` is true, with a **different backend
  per target** (selected by `#[cfg]`, same `new()/set()` interface). Native: `cpal` (native-only dep;
  needs `libasound2-dev` to build) synthesises on its callback thread, gated by a shared atomic.
  Web: a `web_sys` WebAudio graph (oscillator → gain → speakers); cpal can't be used on wasm because it
  can't satisfy the browser autoplay policy, so we drive the context ourselves and resume it on the
  first user gesture (`pointerdown`/`keydown`). Falls back to silence if no device/context opens.

## Conventions

- rustfmt defaults (no `rustfmt.toml`); run `cargo fmt` from the repo root. Keep `#[rustfmt::skip]` on
  hand-aligned data like `FONT_SET`.
- For every opcode family you implement in `chip8-core`, add a `#[cfg(test)]` test that loads a
  tiny ROM, steps, and asserts register/RAM/framebuffer state (see existing tests in `lib.rs`).
