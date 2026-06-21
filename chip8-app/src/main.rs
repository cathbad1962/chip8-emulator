//! chip8-app — the shell that drives chip8-core in a window (native) or in a
//! <canvas> on a web page (wasm). This is the "Rust on a live website" half.
//!
//! The same `Chip8App` runs on both targets from one source file; only the two
//! `fn main`s at the bottom differ. `cargo run` gives you the native build;
//! `trunk serve` (in this directory) gives you the web build.

use std::sync::{Arc, Mutex};

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

struct Chip8App {
    vm: Chip8,
    /// Filled by the file picker, drained by `update()`. See [`RomDrop`].
    pending_rom: RomDrop,
}

impl Chip8App {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut vm = Chip8::new();
        vm.load_rom(DEMO_ROM);
        Self {
            vm,
            pending_rom: Arc::new(Mutex::new(None)),
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

        // Largest scale that fits the available space, preserving the 2:1
        // (64x32) aspect ratio. Each CHIP-8 pixel becomes a `scale`-sized
        // square on screen.
        let avail = ui.available_size();
        let scale = (avail.x / WIDTH as f32)
            .min(avail.y / HEIGHT as f32)
            .max(1.0);

        let (response, painter) = ui.allocate_painter(
            egui::vec2(WIDTH as f32 * scale, HEIGHT as f32 * scale),
            egui::Sense::hover(),
        );
        let origin = response.rect.min;

        // Background, then one filled square per lit pixel. At 2048 cells this
        // is free. (For a 160x144 Game Boy you'd switch to uploading the
        // framebuffer as a nearest-filtered texture instead.)
        painter.rect_filled(response.rect, 0.0, egui::Color32::BLACK);
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
                    self.open_rom_picker(ctx);
                }
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

        // 3. TIMERS — once per frame ≈ 60 Hz (the slow clock).
        //    (For frame-rate-independent accuracy, accumulate ctx.input(|i| i.stable_dt)
        //    and tick on each whole 1/60 s elapsed instead. Fine to skip for now.)
        self.vm.tick_timers();

        // 4. RENDER.
        egui::CentralPanel::default().show(ctx, |ui| {
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
