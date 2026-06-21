//! chip8-app — the shell that drives chip8-core in a window (native) or in a
//! <canvas> on a web page (wasm). This is the "Rust on a live website" half.
//!
//! The same `Chip8App` runs on both targets from one source file; only the two
//! `fn main`s at the bottom differ. `cargo run` gives you the native build;
//! `trunk serve` (in this directory) gives you the web build.

use std::sync::{Arc, Mutex};

// Audio backend imports differ by target (see the AUDIO section below).
#[cfg(not(target_arch = "wasm32"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(target_arch = "wasm32")]
use eframe::wasm_bindgen::{closure::Closure, JsCast};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};

// egui comes through eframe's re-export so the versions can never drift apart.
use eframe::egui;

use chip8_core::{Chip8, HEIGHT, WIDTH};

/// Bytes the file picker hands back, shared between the picker task and the UI
/// thread. The picker runs off the UI thread (a worker thread on native, an
/// async task on web), so it can't touch the VM directly — it drops the loaded
/// ROM in here and `update()` picks it up on the next frame. `None` means "no
/// ROM waiting"; `take()`-ing it both reads and clears the slot.
type RomDrop = Arc<Mutex<Option<Vec<u8>>>>;

/// How many CPU instructions to run per displayed frame. The CPU clock is
/// FASTER than the 60 Hz timer/frame clock — that's why we step many times here
/// but tick the timers only once per frame. ~10 per frame at 60 fps ≈ 600 Hz,
/// a comfortable CHIP-8 speed. Tune to taste.
const CYCLES_PER_FRAME: usize = 10;

/// The CHIP-8 delay and sound timers count down at exactly 60 Hz, independent of
/// the display's refresh rate. We accumulate real elapsed time and tick once per
/// whole `1/60 s` so the timers stay correct on a 144 Hz monitor, a 30 Hz one,
/// or during a stutter — never conflating the timer clock with the frame clock.
const TIMER_PERIOD: f32 = 1.0 / 60.0;

/// Cap on timer catch-up per frame. If the app was stalled (e.g. the file dialog
/// was open for seconds), we don't want to fire hundreds of ticks at once; a few
/// is plenty to resync without a "spiral of death".
const MAX_TIMER_CATCHUP: u32 = 4;

/// A tiny hand-written CHIP-8 program (my own bytes, not a copyrighted ROM) so
/// the scaffold visibly proves the whole stack end-to-end on first run: it
/// draws the built-in "8" sprite near the centre of the screen, then halts.
///
///   200: 60 1C   V0 = 0x1C   (x = 28)
///   202: 61 0D   V1 = 0x0D   (y = 13)
///   204: A0 78   I  = 0x078  (address of the font '8' sprite)
///   206: D0 15   draw 5-row sprite at (V0, V1)   <- this is the moment the
///                                                    framebuffer lights up
///   208: 12 08   jump to 0x208 (spin forever = halt)
///
/// Edit these bytes and re-run — it's a fast, self-contained way to feel how
/// opcodes move the machine. Replace the whole thing with a file picker or
/// `include_bytes!("game.ch8")` once real ROMs are running.
const DEMO_ROM: &[u8] = &[0x60, 0x1C, 0x61, 0x0D, 0xA0, 0x78, 0xD0, 0x15, 0x12, 0x08];

/// Built-in games, baked into the binary with `include_bytes!` so they ship with
/// the app on every platform — crucially the web build, which has no filesystem
/// to read ROMs from. They're tiny (a few hundred bytes to a few KB each), so the
/// size cost is negligible.
///
/// All are from the Chip8 Community Archive (https://github.com/JohnEarnest/chip8Archive)
/// and released under Creative Commons Zero (CC0, public domain). See
/// `roms/README.md` for per-game attribution.
const GAMES: &[(&str, &[u8])] = &[
    ("Breakout", include_bytes!("../../roms/games/br8kout.ch8")),
    ("Snake", include_bytes!("../../roms/games/snek.ch8")),
    ("Outlaw", include_bytes!("../../roms/games/outlaw.ch8")),
    (
        "Cave Explorer",
        include_bytes!("../../roms/games/caveexplorer.ch8"),
    ),
    (
        "Ghost Escape",
        include_bytes!("../../roms/games/ghostEscape.ch8"),
    ),
    ("Danm8ku", include_bytes!("../../roms/games/danm8ku.ch8")),
    ("Tank!", include_bytes!("../../roms/games/tank.ch8")),
    ("Spacejam!", include_bytes!("../../roms/games/spacejam.ch8")),
    (
        "Slippery Slope",
        include_bytes!("../../roms/games/slipperyslope.ch8"),
    ),
    (
        "Chipquarium",
        include_bytes!("../../roms/games/chipquarium.ch8"),
    ),
];

/// CHIP-8 has a 16-key hex keypad. The conventional mapping onto a QWERTY
/// keyboard, laid out so the physical block matches the original 4x4 pad:
///
///   1 2 3 C        1 2 3 4
///   4 5 6 D   <->  Q W E R
///   7 8 9 E        A S D F
///   A 0 B F        Z X C V
const KEYMAP: [(egui::Key, usize); 16] = [
    (egui::Key::Num1, 0x1),
    (egui::Key::Num2, 0x2),
    (egui::Key::Num3, 0x3),
    (egui::Key::Num4, 0xC),
    (egui::Key::Q, 0x4),
    (egui::Key::W, 0x5),
    (egui::Key::E, 0x6),
    (egui::Key::R, 0xD),
    (egui::Key::A, 0x7),
    (egui::Key::S, 0x8),
    (egui::Key::D, 0x9),
    (egui::Key::F, 0xE),
    (egui::Key::Z, 0xA),
    (egui::Key::X, 0x0),
    (egui::Key::C, 0xB),
    (egui::Key::V, 0xF),
];

// ---------------------------------------------------------------------------
// AUDIO — a 440 Hz square-wave beeper wired to the CHIP-8 sound timer. The core
// knows nothing about audio hardware; it only exposes `is_beeping()` (sound
// timer non-zero) and the shell owns the actual output device.
//
// This is the one piece that needs a genuinely different backend per target:
//   • Native: cpal synthesises samples on its own real-time callback thread,
//     gated by a shared atomic that the UI flips once per frame.
//   • Web: there is no autoplay-free audio. We build a WebAudio graph
//     (square oscillator → gain → speakers) via web_sys and beep by opening the
//     gain gate. The browser starts the AudioContext *suspended*, so we resume
//     it on the first user gesture — exactly the event context the autoplay
//     policy demands, and one `update()` (a requestAnimationFrame tick) can
//     never supply. cpal can't do this on wasm: it creates its context before
//     any gesture and never re-resumes it, so the browser keeps it muted.
//
// Both expose the same tiny interface: `Beeper::new() -> Option<Self>` (audio is
// a nice-to-have; failure just means silence) and `beeper.set(bool)`.
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
struct Beeper {
    // The stream must stay alive for audio to keep flowing — dropping it stops
    // the device. We never touch it again, hence the leading underscore.
    _stream: cpal::Stream,
    on: Arc<AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Beeper {
    /// Open the default output device and start a (silent) stream. Audio is a
    /// nice-to-have, so every failure path returns `None` and the app runs on in
    /// silence rather than refusing to start.
    fn new() -> Option<Self> {
        let device = cpal::default_host().default_output_device()?;
        let config = device.default_output_config().ok()?;
        let on = Arc::new(AtomicBool::new(false));

        // The device dictates the sample format; build the right one.
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => Self::build::<f32>(&device, &config.into(), on.clone()),
            cpal::SampleFormat::I16 => Self::build::<i16>(&device, &config.into(), on.clone()),
            cpal::SampleFormat::U16 => Self::build::<u16>(&device, &config.into(), on.clone()),
            _ => None,
        }?;
        stream.play().ok()?;
        Some(Self {
            _stream: stream,
            on,
        })
    }

    /// Build the output stream for one concrete sample format. The callback
    /// synthesises the square wave on the fly, gated by the shared `on` flag.
    fn build<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        on: Arc<AtomicBool>,
    ) -> Option<cpal::Stream>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let channels = config.channels as usize;
        let step = 440.0 / config.sample_rate.0 as f32; // phase advance per sample
        let mut phase = 0.0f32;
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    let playing = on.load(Ordering::Relaxed);
                    for frame in data.chunks_mut(channels) {
                        // Square wave at a gentle volume; flat silence when off.
                        let v = if !playing {
                            0.0
                        } else if phase < 0.5 {
                            0.15
                        } else {
                            -0.15
                        };
                        phase += step;
                        if phase >= 1.0 {
                            phase -= 1.0;
                        }
                        let sample = T::from_sample(v);
                        for out in frame.iter_mut() {
                            *out = sample; // same value to every channel (mono → all)
                        }
                    }
                },
                |err| log::error!("audio stream error: {err}"),
                None,
            )
            .ok()
    }

    /// Flip the beep on or off. Called once per frame from `update()`.
    fn set(&self, beeping: bool) {
        self.on.store(beeping, Ordering::Relaxed);
    }
}

#[cfg(target_arch = "wasm32")]
struct Beeper {
    ctx: web_sys::AudioContext,
    /// Master gain gate: 0.0 = silent, a small positive value = audible. The
    /// oscillator runs forever; we beep by opening and closing this gate.
    gain: web_sys::GainNode,
    /// Kept alive for the Beeper's lifetime — dropping the closure would
    /// unregister the gesture listener that resumes the audio context.
    _resume: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl Beeper {
    /// Build the WebAudio graph (square oscillator → gain → speakers) and start
    /// the oscillator running silently. `None` if WebAudio is unavailable.
    fn new() -> Option<Self> {
        let ctx = web_sys::AudioContext::new().ok()?;
        let osc = ctx.create_oscillator().ok()?;
        let gain = ctx.create_gain().ok()?;

        osc.set_type(web_sys::OscillatorType::Square);
        osc.frequency().set_value(440.0);
        gain.gain().set_value(0.0); // start silent

        osc.connect_with_audio_node(&gain).ok()?;
        gain.connect_with_audio_node(&ctx.destination()).ok()?;
        osc.start().ok()?;

        // Resume the (suspended) context on the first real user gesture. A click
        // or key press anywhere on the page is enough — and clicking "Load ROM…"
        // counts, so sound is live before any game gets a chance to beep.
        let ctx_for_resume = ctx.clone();
        let resume = Closure::<dyn FnMut()>::new(move || {
            let _ = ctx_for_resume.resume();
        });
        if let Some(win) = web_sys::window() {
            let cb = resume.as_ref().unchecked_ref();
            let _ = win.add_event_listener_with_callback("pointerdown", cb);
            let _ = win.add_event_listener_with_callback("keydown", cb);
        }

        Some(Self {
            ctx,
            gain,
            _resume: resume,
        })
    }

    /// Open or close the gain gate. Called once per frame from `update()`. As a
    /// safety net we also resume the context here if it's still suspended (cheap,
    /// and a no-op once the gesture listener has done its job).
    fn set(&self, beeping: bool) {
        self.gain.gain().set_value(if beeping { 0.15 } else { 0.0 });
        if beeping && self.ctx.state() == web_sys::AudioContextState::Suspended {
            let _ = self.ctx.resume();
        }
    }
}

struct Chip8App {
    vm: Chip8,
    /// Filled by the file picker, drained by `update()`. See [`RomDrop`].
    pending_rom: RomDrop,
    /// Square-wave output mirroring the VM's sound timer. `None` if no audio
    /// device could be opened — the emulator just runs silently.
    beeper: Option<Beeper>,
    /// Seconds of real time owed to the 60 Hz timers but not yet ticked. See
    /// [`TIMER_PERIOD`].
    timer_accumulator: f32,
    /// Index into [`GAMES`] of the last game picked from the dropdown, for the
    /// combo box's displayed text. `None` until a game is chosen (or after a ROM
    /// is loaded from a file instead).
    selected_game: Option<usize>,
}

impl Chip8App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut vm = Chip8::new();
        vm.load_rom(DEMO_ROM);
        // Start audio now. On the web the AudioContext begins *suspended* under
        // browser autoplay rules and only resumes after a user gesture — which
        // happens naturally the first time the user clicks "Load ROM…" or
        // presses a key, so by the time a game actually beeps, sound is live.
        Self {
            vm,
            pending_rom: Arc::new(Mutex::new(None)),
            beeper: Beeper::new(),
            timer_accumulator: 0.0,
            selected_game: None,
        }
    }

    /// Open the OS/browser file dialog and, once the user picks a file, drop its
    /// bytes into `pending_rom` for the next `update()` to load.
    ///
    /// The two targets differ only in *how* we get off the UI thread. Native:
    /// `rfd::FileDialog` is blocking, so we run it on a throwaway thread and
    /// read the file with `std::fs`. Web: there is no filesystem and we must
    /// never block, so we use the async dialog and `spawn_local`, reading the
    /// bytes straight out of the browser File object. Either way we then ask
    /// eframe to repaint so the waiting bytes get noticed promptly.
    fn open_rom_picker(&self, ctx: &egui::Context) {
        let slot = self.pending_rom.clone();
        let ctx = ctx.clone();

        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("CHIP-8 ROM", &["ch8", "rom"])
                .pick_file()
            {
                if let Ok(bytes) = std::fs::read(&path) {
                    *slot.lock().unwrap() = Some(bytes);
                    ctx.request_repaint();
                }
            }
        });

        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            if let Some(file) = rfd::AsyncFileDialog::new()
                .add_filter("CHIP-8 ROM", &["ch8", "rom"])
                .pick_file()
                .await
            {
                let bytes = file.read().await;
                *slot.lock().unwrap() = Some(bytes);
                ctx.request_repaint();
            }
        });
    }

    fn paint_screen(&self, ui: &mut egui::Ui) {
        let fb = self.vm.framebuffer();

        // Take the whole panel: we paint a black background edge-to-edge and
        // centre the CHIP-8 image in it, so the window is always filled and the
        // (letterboxed) image sits in the middle rather than the top-left.
        let (panel, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        let panel = panel.rect;
        painter.rect_filled(panel, 0.0, egui::Color32::BLACK);

        // Largest scale that fits, preserving the 2:1 (64x32) aspect ratio. Each
        // CHIP-8 pixel becomes a `scale`-sized square on screen.
        let scale = (panel.width() / WIDTH as f32)
            .min(panel.height() / HEIGHT as f32)
            .max(1.0);

        // Centre the scaled image within the panel.
        let image = egui::vec2(WIDTH as f32 * scale, HEIGHT as f32 * scale);
        let origin = panel.center() - image / 2.0;

        // One filled square per lit pixel. At 2048 cells this is free. (For a
        // 160x144 Game Boy you'd upload the framebuffer as a nearest-filtered
        // texture instead.)
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if fb[y * WIDTH + x] {
                    let min = origin + egui::vec2(x as f32 * scale, y as f32 * scale);
                    let rect = egui::Rect::from_min_size(min, egui::vec2(scale, scale));
                    // NOTE: the middle arg is the corner radius. `0.0` works on
                    // most egui versions; if your version complains, use
                    // `egui::CornerRadius::ZERO` (newer) or `egui::Rounding::ZERO` (older).
                    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
                }
            }
        }
    }
}

impl eframe::App for Chip8App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 0. LOAD — if the picker left a ROM waiting, swap in a fresh machine.
        //    A new ROM means a new game, so we rebuild the VM from scratch
        //    rather than poke bytes into the running one. `reseed` injects real
        //    entropy here: the core's RNG is deterministic by design (no clock
        //    below the line), so the shell — which *does* have a clock — is the
        //    right place to make `CXNN` actually random. eframe's frame time is
        //    a fine, dependency-free entropy source.
        if let Some(rom) = self.pending_rom.lock().unwrap().take() {
            let mut vm = Chip8::new();
            vm.load_rom(&rom);
            vm.reseed((ctx.input(|i| i.time) * 1_000_000.0) as u32);
            self.vm = vm;
        }

        // Toolbar above the screen.
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Load ROM…").clicked() {
                    // A file load means we're no longer on a built-in game, so
                    // clear the dropdown's selection.
                    self.selected_game = None;
                    self.open_rom_picker(ctx);
                }

                // Built-in games. Selecting one hands its bytes to the same
                // `pending_rom` slot the file picker uses, so the load + reseed
                // logic at the top of `update()` handles it uniformly next frame.
                egui::ComboBox::from_id_salt("games")
                    .selected_text(match self.selected_game {
                        Some(i) => GAMES[i].0,
                        None => "Games ▾",
                    })
                    .show_ui(ui, |ui| {
                        for (i, (name, bytes)) in GAMES.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_game == Some(i), *name)
                                .clicked()
                            {
                                self.selected_game = Some(i);
                                *self.pending_rom.lock().unwrap() = Some(bytes.to_vec());
                            }
                        }
                    });
            });
        });

        // 1. INPUT — sample which keypad keys are held this frame.
        ctx.input(|input| {
            for (key, idx) in KEYMAP {
                self.vm.set_key(idx, input.key_down(key));
            }
        });

        // 2. CPU — run a batch of instructions (the fast clock).
        for _ in 0..CYCLES_PER_FRAME {
            self.vm.step();
        }

        // 3. TIMERS — the slow clock, locked to a true 60 Hz regardless of the
        //    display's frame rate. We bank the real seconds elapsed since the
        //    last frame (`stable_dt` is eframe's smoothed frame time) and spend
        //    them one 1/60 s tick at a time, capped so a long stall can't unleash
        //    a flood of catch-up ticks.
        self.timer_accumulator += ctx.input(|i| i.stable_dt);
        let mut ticks = 0;
        while self.timer_accumulator >= TIMER_PERIOD && ticks < MAX_TIMER_CATCHUP {
            self.vm.tick_timers();
            self.timer_accumulator -= TIMER_PERIOD;
            ticks += 1;
        }
        // Drop any backlog beyond the cap so we don't stay perpetually behind.
        if self.timer_accumulator > TIMER_PERIOD {
            self.timer_accumulator = 0.0;
        }

        // 3b. AUDIO — mirror the sound-timer state to the beeper each frame. The
        //     beeper handles the actual sound (a cpal callback thread on native,
        //     a WebAudio gain gate on the web); here we just tell it on or off.
        if let Some(beeper) = &self.beeper {
            beeper.set(self.vm.is_beeping());
        }

        // 4. RENDER. A margin-free, black-filled panel so the emulator screen
        //    reaches the window edges instead of sitting inside egui's default
        //    padding.
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                self.paint_screen(ui);
            });

        // 5. SCHEDULE THE NEXT FRAME. This is the key web-vs-native difference,
        //    made concrete: a native emulator owns `loop { step(); render(); }`,
        //    but in the browser you can never block — the host calls YOU once
        //    per frame. eframe only repaints on demand, so an always-running
        //    emulator must explicitly ask to be called again. This line is the
        //    eframe equivalent of scheduling the next requestAnimationFrame.
        ctx.request_repaint();
    }
}

// ---------------------------------------------------------------------------
// NATIVE entry point. Develop and debug here (println!, a real debugger, fast
// edit-run loop), then ship the exact same app to the web.
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "CHIP-8",
        native_options,
        Box::new(|cc| Ok(Box::new(Chip8App::new(cc)))),
    )
}

// ---------------------------------------------------------------------------
// WEB entry point. Trunk builds this to wasm and the generated loader calls it.
// It finds the <canvas> by id and hands it to eframe's WebRunner.
//
// ⚠ This is the one version-fragile block in the project. It matches the
//   current `eframe_template`. If it doesn't compile against your eframe
//   version, generate the template for that version and copy its web `main`
//   verbatim, swapping in `Chip8App`:
//     cargo install cargo-generate
//     cargo generate --git https://github.com/emilk/eframe_template
//   Everything else in this file (the App, update loop, paint) is stable API.
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    // Surface panics/logs in the browser console instead of failing silently.
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    let web_options = eframe::WebOptions::default();
    wasm_bindgen_futures::spawn_local(async {
        let document = eframe::web_sys::window()
            .expect("no global `window`")
            .document()
            .expect("no document on window");
        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("index.html is missing <canvas id=\"the_canvas_id\">")
            .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
            .expect("element #the_canvas_id was not a <canvas>");

        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| Ok(Box::new(Chip8App::new(cc)))),
            )
            .await
            .expect("failed to start eframe");
    });
}
