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
- Web dev server (needs `trunk`, not currently installed — `cargo install trunk`):
  `cd chip8-app && trunk serve` → http://localhost:8080
- Web release build: `cd chip8-app && trunk build --release`

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
- ROM loading is a hard-coded demo ROM; `Chip8::load_rom(&[u8])` exists but no file picker is wired.
- Audio: `is_beeping()` reports sound-timer state but no actual audio output is connected.

## Conventions

- rustfmt defaults (no `rustfmt.toml`); run `cargo fmt` from the repo root. Keep `#[rustfmt::skip]` on
  hand-aligned data like `FONT_SET`.
- For every opcode family you implement in `chip8-core`, add a `#[cfg(test)]` test that loads a
  tiny ROM, steps, and asserts register/RAM/framebuffer state (see existing tests in `lib.rs`).
