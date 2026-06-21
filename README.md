# CHIP-8 Emulator

A CHIP-8 virtual machine written in Rust, with an [egui](https://github.com/emilk/egui)
front-end that runs both as a native desktop app and in the browser (WebAssembly)
from a single source.

## Workspace layout

A Cargo workspace with two crates:

- **`chip8-core`** — the virtual machine. Pure Rust, **zero dependencies**, knows
  nothing about the UI, the browser, the filesystem, or wall-clock time. All 16
  opcode families are implemented and unit-tested.
- **`chip8-app`** — the shell: an `eframe`/egui UI plus the native and web entry
  points. Depends on `chip8-core`.

## Features

- Full CHIP-8 instruction set (graphics, input, sound, timers).
- Runs natively and in the browser from one codebase.
- **Built-in games** — a handful of public-domain (CC0) games baked into the
  binary, reachable from the **Games ▾** dropdown (works offline, even on the web).
- **ROM file picker** — load any `.ch8`/`.rom` (native OS dialog / browser file input).
- **Audio** — a 440 Hz square-wave beeper driven by the sound timer (`cpal` on
  native, WebAudio on the web).
- **Keyboard** — the 16-key hex keypad mapped onto QWERTY.
- Timers tick at a true 60 Hz, independent of the display refresh rate.

## Build & run

Run from the repo root unless noted.

| Task | Command |
| --- | --- |
| Test the VM core | `cargo test -p chip8-core` |
| Run the native app | `cargo run -p chip8-app` |
| Web dev server | `cd chip8-app && trunk serve` → http://localhost:8080 |
| Web release build | `cd chip8-app && trunk build --release` |

The web build needs [`trunk`](https://trunkrs.dev) (`cargo install trunk`) and the
wasm target (`rustup target add wasm32-unknown-unknown`). Native audio needs the
ALSA dev headers on Debian/Ubuntu (`sudo apt install libasound2-dev`).

> Run `trunk` from inside `chip8-app/`, not the repo root — the root is a virtual
> workspace with no package, so trunk fails with "could not find the root package".

## Controls

The hex keypad maps onto QWERTY so the physical block matches the original 4×4 pad:

```
 keyboard      CHIP-8 keypad
 1 2 3 4        1 2 3 C
 Q W E R   →    4 5 6 D
 A S D F        7 8 9 E
 Z X C V        A 0 B F
```

## Games

A few games are baked into the binary and selectable from the **Games ▾** dropdown:
Breakout, Snake, Outlaw, Cave Explorer, and Ghost Escape. They come from the
[Chip8 Community Archive](https://github.com/JohnEarnest/chip8Archive) and are all
released under **Creative Commons Zero (CC0, public domain)**. See
[`roms/README.md`](roms/README.md) for per-game attribution.

## Test ROMs

The [`roms/`](roms/) directory also ships tiny programs that exercise each I/O path
(see [`roms/README.md`](roms/README.md)). Load one with **Load ROM…**:

- **`ibm-logo.ch8`** — draws the "IBM" logo; a render / first-light test.
- **`beep-test.ch8`** — beeps for ~2 seconds then stops; an audio test.
- **`input-test.ch8`** — shows the hex digit of whichever key you press; a keyboard test.

## License

The bundled games under `roms/games/` are public domain (CC0) — see
[`roms/README.md`](roms/README.md) for sources and attribution. The test ROMs under
`roms/` are either public domain (`ibm-logo.ch8`) or original to this project.
