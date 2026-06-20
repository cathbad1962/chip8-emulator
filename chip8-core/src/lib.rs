//! chip8-core — a browser-agnostic CHIP-8 virtual machine.
//!
//! This crate is PURE Rust with no dependencies. It knows nothing about eframe,
//! egui, the browser, the filesystem, or wall-clock time. It is the half of the
//! project that would be IDENTICAL if you wrote this as a native desktop binary
//! — and it's the half you can `cargo test` in isolation.
//!
//! Public surface (everything the shell needs):
//!   Chip8::new()              construct a fresh machine (font already loaded)
//!   .load_rom(&[u8])          reset and load a program at 0x200
//!   .step()                   fetch/decode/execute ONE instruction (~500 Hz)
//!   .tick_timers()            decrement the two 60 Hz timers (exactly 60 Hz)
//!   .framebuffer() -> &[bool] the 64*32 pixel grid, for the UI to paint
//!   .set_key(idx, pressed)    push keypad state in (idx 0x0..=0xF)
//!   .is_beeping() -> bool     true while the sound timer is non-zero

pub const WIDTH: usize = 64;
pub const HEIGHT: usize = 32;

const RAM_SIZE: usize = 4096;
const PROGRAM_START: usize = 0x200; // programs load here (lower 512 bytes were
                                    // the original interpreter's home, by convention)
const FONT_START: usize = 0x50;
const NUM_REGS: usize = 16;
const STACK_DEPTH: usize = 16;
const NUM_KEYS: usize = 16;

/// The 16 built-in hex-digit sprites (0-F), 5 bytes tall each. A program points
/// the index register `I` at one of these and draws it with DXYN.
#[rustfmt::skip]
const FONT_SET: [u8; 80] = [
    0xF0, 0x90, 0x90, 0x90, 0xF0, // 0
    0x20, 0x60, 0x20, 0x20, 0x70, // 1
    0xF0, 0x10, 0xF0, 0x80, 0xF0, // 2
    0xF0, 0x10, 0xF0, 0x10, 0xF0, // 3
    0x90, 0x90, 0xF0, 0x10, 0x10, // 4
    0xF0, 0x80, 0xF0, 0x10, 0xF0, // 5
    0xF0, 0x80, 0xF0, 0x90, 0xF0, // 6
    0xF0, 0x10, 0x20, 0x40, 0x40, // 7
    0xF0, 0x90, 0xF0, 0x90, 0xF0, // 8
    0xF0, 0x90, 0xF0, 0x10, 0xF0, // 9
    0xF0, 0x90, 0xF0, 0x90, 0x90, // A
    0xE0, 0x90, 0xE0, 0x90, 0xE0, // B
    0xF0, 0x80, 0x80, 0x80, 0xF0, // C
    0xE0, 0x90, 0x90, 0x90, 0xE0, // D
    0xF0, 0x80, 0xF0, 0x80, 0xF0, // E
    0xF0, 0x80, 0xF0, 0x80, 0x80, // F
];

pub struct Chip8 {
    ram: [u8; RAM_SIZE],
    display: [bool; WIDTH * HEIGHT],
    v: [u8; NUM_REGS], // general registers V0..VF; VF doubles as the flag register
    i: u16,            // index register (addresses)
    pc: u16,           // program counter
    stack: [u16; STACK_DEPTH],
    sp: usize, // stack pointer (next free slot)
    delay_timer: u8,
    sound_timer: u8,
    keys: [bool; NUM_KEYS],
}

impl Default for Chip8 {
    fn default() -> Self {
        Self::new()
    }
}

impl Chip8 {
    pub fn new() -> Self {
        let mut ram = [0u8; RAM_SIZE];
        ram[FONT_START..FONT_START + FONT_SET.len()].copy_from_slice(&FONT_SET);
        Self {
            ram,
            display: [false; WIDTH * HEIGHT],
            v: [0; NUM_REGS],
            i: 0,
            pc: PROGRAM_START as u16,
            stack: [0; STACK_DEPTH],
            sp: 0,
            delay_timer: 0,
            sound_timer: 0,
            keys: [false; NUM_KEYS],
        }
    }

    /// Reset the machine and load a program at 0x200.
    pub fn load_rom(&mut self, rom: &[u8]) {
        *self = Self::new();
        let end = PROGRAM_START + rom.len();
        self.ram[PROGRAM_START..end].copy_from_slice(rom);
    }

    pub fn framebuffer(&self) -> &[bool] {
        &self.display
    }

    pub fn is_beeping(&self) -> bool {
        self.sound_timer > 0
    }

    pub fn set_key(&mut self, idx: usize, pressed: bool) {
        if idx < NUM_KEYS {
            self.keys[idx] = pressed;
        }
    }

    /// Decrement the two timers. Call this at EXACTLY 60 Hz, independently of
    /// how often you call `step()`. (Conflating the two clocks is the classic
    /// CHIP-8 bug — games crawl or the logic desyncs from the countdown.)
    pub fn tick_timers(&mut self) {
        if self.delay_timer > 0 {
            self.delay_timer -= 1;
        }
        if self.sound_timer > 0 {
            self.sound_timer -= 1;
        }
    }

    /// Fetch / decode / execute one instruction — the heart of the machine.
    /// Call this many times per frame (CHIP-8 typically runs a few hundred
    /// instructions per second).
    pub fn step(&mut self) {
        let opcode = self.fetch();
        self.execute(opcode);
    }

    fn fetch(&mut self) -> u16 {
        let hi = self.ram[self.pc as usize] as u16;
        let lo = self.ram[(self.pc + 1) as usize] as u16;
        self.pc += 2;
        (hi << 8) | lo
    }

    fn execute(&mut self, opcode: u16) {
        // Decoded fields, named the conventional CHIP-8 way:
        let nnn = opcode & 0x0FFF; // 12-bit address
        let nn = (opcode & 0x00FF) as u8; // 8-bit immediate
        let n = (opcode & 0x000F) as usize; // 4-bit nibble
        let x = ((opcode & 0x0F00) >> 8) as usize; // register index
        let y = ((opcode & 0x00F0) >> 4) as usize; // register index

        // The whole decoder is a match on the top nibble, fanning out from
        // there. Every one of the 16 families has an arm below; the ones that
        // are `unimplemented(...)` are YOUR project.
        match (opcode & 0xF000) >> 12 {
            0x0 => match nn {
                0xE0 => self.display = [false; WIDTH * HEIGHT], // 00E0  CLS
                0xEE => {
                    // 00EE  RET — pop a return address off the stack
                    self.sp -= 1;
                    self.pc = self.stack[self.sp];
                }
                _ => self.unimplemented(opcode),
            },
            0x1 => self.pc = nnn, // 1NNN  JP addr
            0x2 => {
                // 2NNN  CALL addr — push the return address, then jump
                self.stack[self.sp] = self.pc;
                self.sp += 1;
                self.pc = nnn;
            }
            0x6 => self.v[x] = nn, // 6XNN  LD  Vx, byte
            0x7 => self.v[x] = self.v[x].wrapping_add(nn), // 7XNN  ADD Vx, byte
            0xA => self.i = nnn,   // ANNN  LD  I, addr
            0xD => self.draw(x, y, n), // DXYN  DRW Vx, Vy, n

            // ---------------------------------------------------------------
            // NOT YET IMPLEMENTED — this is the project. Each arm below is a
            // stub. Replace `self.unimplemented(opcode)` with the real logic,
            // bringing them online one family at a time. (The demo ROM never
            // reaches these, so the app runs cleanly until you get here.)
            // ---------------------------------------------------------------
            0x3 => self.unimplemented(opcode), // 3XNN  SE  Vx, byte  -> skip next if Vx == NN  (pc += 2)
            0x4 => self.unimplemented(opcode), // 4XNN  SNE Vx, byte  -> skip next if Vx != NN
            0x5 => self.unimplemented(opcode), // 5XY0  SE  Vx, Vy    -> skip next if Vx == Vy
            0x8 => self.unimplemented(opcode), // 8XY0..8XYE  ALU family: set/or/and/xor/add/sub/shr/subn/shl. VF is the carry/borrow/shift-out flag — get this group right and most games work.
            0x9 => self.unimplemented(opcode), // 9XY0  SNE Vx, Vy    -> skip next if Vx != Vy
            0xB => self.unimplemented(opcode), // BNNN  JP  V0, addr  -> pc = NNN + V0
            0xC => self.unimplemented(opcode), // CXNN  RND Vx, byte  -> Vx = (random u8) & NN  (you'll need an RNG; a tiny LCG/xorshift in this crate keeps it dependency-free)
            0xE => self.unimplemented(opcode), // EX9E / EXA1  -> skip next if key Vx is / isn't pressed (reads self.keys)
            0xF => self.unimplemented(opcode), // FX07/FX0A/FX15/FX18/FX1E/FX29/FX33/FX55/FX65 -> timers, wait-for-key, I += Vx, font address, BCD, register dump/load
            _ => unreachable!("top nibble is 4 bits"),
        }
    }

    /// DXYN — draw the N-byte sprite stored at memory[I] at screen (Vx, Vy).
    ///
    /// Drawing is XOR-based: each set sprite bit FLIPS the pixel under it. If a
    /// flip turns an already-lit pixel OFF, that's a collision and VF is set to
    /// 1. This one mechanic is how every CHIP-8 game does hit detection.
    fn draw(&mut self, x: usize, y: usize, n: usize) {
        // The starting coordinate wraps; pixels drawn past an edge are clipped.
        let origin_x = self.v[x] as usize % WIDTH;
        let origin_y = self.v[y] as usize % HEIGHT;
        self.v[0xF] = 0;
        for row in 0..n {
            let sprite_byte = self.ram[self.i as usize + row];
            for col in 0..8 {
                // Test bits left-to-right: 0x80 >> col.
                if (sprite_byte & (0x80 >> col)) != 0 {
                    let px = origin_x + col;
                    let py = origin_y + row;
                    if px >= WIDTH || py >= HEIGHT {
                        continue; // clip at the screen edge
                    }
                    let idx = py * WIDTH + px;
                    if self.display[idx] {
                        self.v[0xF] = 1; // collision: a lit pixel got turned off
                    }
                    self.display[idx] ^= true;
                }
            }
        }
    }

    /// Reached when an unimplemented opcode runs. It's a deliberate no-op so the
    /// demo keeps running while you fill the decode tree in. While developing a
    /// specific family you'll often prefer to fail loudly instead:
    ///
    ///   panic!("unimplemented opcode: {:#06X}", _opcode);
    fn unimplemented(&self, _opcode: u16) {}
}

// A starter test, to show the pattern. The whole point of the dependency-free
// core is that you can drive opcodes like this with zero UI. As you implement
// each family, add a test that sets up registers/RAM, steps once, and asserts.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ld_then_add_immediate() {
        let mut vm = Chip8::new();
        // 6005  -> V0 = 0x05
        // 7003  -> V0 = V0 + 0x03
        vm.load_rom(&[0x60, 0x05, 0x70, 0x03]);
        vm.step();
        vm.step();
        assert_eq!(vm.v[0], 0x08);
    }

    #[test]
    fn draw_sets_pixels_and_collision_flag() {
        let mut vm = Chip8::new();
        // I = font '0' (0x50), draw 5 rows at (0,0): A050 D005
        vm.load_rom(&[0xA0, 0x50, 0xD0, 0x05]);
        vm.step();
        vm.step();
        assert!(vm.framebuffer().iter().any(|&p| p)); // something got drawn
        assert_eq!(vm.v[0xF], 0); // first draw on a blank screen: no collision
    }
}
