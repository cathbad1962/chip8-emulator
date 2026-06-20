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
    rng: u32, // xorshift state for CXNN; non-zero. See next_random()/reseed().
}

/// Fixed default RNG seed. The core has no clock, so randomness is deterministic
/// by default (good for tests/replays). The shell can call `reseed()` after
/// `load_rom()` to introduce real entropy.
const RNG_SEED: u32 = 0x9E37_79B9; // any non-zero constant

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
            rng: RNG_SEED,
        }
    }

    /// Reseed the CXNN random generator. xorshift state must be non-zero, so a
    /// zero seed falls back to the default. Call after `load_rom()` if you want
    /// non-deterministic randomness (e.g. seed from wall-clock time in the shell).
    pub fn reseed(&mut self, seed: u32) {
        self.rng = if seed == 0 { RNG_SEED } else { seed };
    }

    /// Advance the xorshift32 PRNG and return the low byte.
    fn next_random(&mut self) -> u8 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x & 0xFF) as u8
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
            // Skip family: "skip the next instruction" means advance pc past one
            // more 2-byte opcode (pc += 2).
            0x3 => {
                // 3XNN  SE  Vx, byte — skip if Vx == NN
                if self.v[x] == nn {
                    self.pc += 2;
                }
            }
            0x4 => {
                // 4XNN  SNE Vx, byte — skip if Vx != NN
                if self.v[x] != nn {
                    self.pc += 2;
                }
            }
            0x5 => {
                // 5XY0  SE  Vx, Vy — skip if Vx == Vy
                if self.v[x] == self.v[y] {
                    self.pc += 2;
                }
            }
            // 8XY_  ALU family, dispatched on the low nibble. VF is the
            // carry/borrow/shift-out flag and is written LAST, so when x == 0xF
            // the flag wins over the arithmetic result (the documented quirk).
            0x8 => match n {
                0x0 => self.v[x] = self.v[y],  // 8XY0  LD  Vx, Vy
                0x1 => self.v[x] |= self.v[y], // 8XY1  OR  Vx, Vy
                0x2 => self.v[x] &= self.v[y], // 8XY2  AND Vx, Vy
                0x3 => self.v[x] ^= self.v[y], // 8XY3  XOR Vx, Vy
                0x4 => {
                    // 8XY4  ADD Vx, Vy — VF = 1 on carry
                    let (res, carry) = self.v[x].overflowing_add(self.v[y]);
                    self.v[x] = res;
                    self.v[0xF] = carry as u8;
                }
                0x5 => {
                    // 8XY5  SUB Vx, Vy — VF = NOT borrow (1 if Vx >= Vy)
                    let (res, borrow) = self.v[x].overflowing_sub(self.v[y]);
                    self.v[x] = res;
                    self.v[0xF] = (!borrow) as u8;
                }
                0x6 => {
                    // 8XY6  SHR Vx — VF = shifted-out LSB. Modern (CHIP-48/SUPER-CHIP)
                    // convention: shift Vx in place, ignoring Vy.
                    let lsb = self.v[x] & 1;
                    self.v[x] >>= 1;
                    self.v[0xF] = lsb;
                }
                0x7 => {
                    // 8XY7  SUBN Vx, Vy — Vx = Vy - Vx, VF = NOT borrow (1 if Vy >= Vx)
                    let (res, borrow) = self.v[y].overflowing_sub(self.v[x]);
                    self.v[x] = res;
                    self.v[0xF] = (!borrow) as u8;
                }
                0xE => {
                    // 8XYE  SHL Vx — VF = shifted-out MSB (modern: shift Vx in place).
                    let msb = self.v[x] >> 7;
                    self.v[x] <<= 1;
                    self.v[0xF] = msb;
                }
                _ => self.unimplemented(opcode),
            },
            0x9 => {
                // 9XY0  SNE Vx, Vy — skip if Vx != Vy
                if self.v[x] != self.v[y] {
                    self.pc += 2;
                }
            }
            0xB => self.pc = nnn + self.v[0] as u16, // BNNN  JP V0, addr — pc = NNN + V0
            0xC => self.v[x] = self.next_random() & nn, // CXNN  RND Vx, byte — Vx = rand() & NN
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
    fn alu_logical_ops() {
        let mut vm = Chip8::new();
        // 600C V0=0x0C, 610A V1=0x0A, 8011 V0 |= V1 -> 0x0E
        vm.load_rom(&[0x60, 0x0C, 0x61, 0x0A, 0x80, 0x11]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x0E);

        let mut vm = Chip8::new();
        // 600C, 610A, 8012 AND -> 0x08
        vm.load_rom(&[0x60, 0x0C, 0x61, 0x0A, 0x80, 0x12]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x08);

        let mut vm = Chip8::new();
        // 600C, 610A, 8013 XOR -> 0x06
        vm.load_rom(&[0x60, 0x0C, 0x61, 0x0A, 0x80, 0x13]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x06);
    }

    #[test]
    fn alu_add_sets_carry() {
        let mut vm = Chip8::new();
        // 60FF V0=0xFF, 6101 V1=0x01, 8014 V0 += V1 -> 0x00 with carry
        vm.load_rom(&[0x60, 0xFF, 0x61, 0x01, 0x80, 0x14]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x00);
        assert_eq!(vm.v[0xF], 1); // carry out

        let mut vm = Chip8::new();
        // 6001, 6101, 8014 -> 0x02, no carry
        vm.load_rom(&[0x60, 0x01, 0x61, 0x01, 0x80, 0x14]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x02);
        assert_eq!(vm.v[0xF], 0);
    }

    #[test]
    fn alu_sub_and_subn_borrow_flag() {
        // 8XY5: VF = 1 when NO borrow (Vx >= Vy)
        let mut vm = Chip8::new();
        // 6005, 6103, 8015 -> 5-3=2, VF=1
        vm.load_rom(&[0x60, 0x05, 0x61, 0x03, 0x80, 0x15]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x02);
        assert_eq!(vm.v[0xF], 1);

        // borrow case: 3-5 wraps to 0xFE, VF=0
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x03, 0x61, 0x05, 0x80, 0x15]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0xFE);
        assert_eq!(vm.v[0xF], 0);

        // 8XY7 SUBN: Vx = Vy - Vx. 6003, 6105, 8017 -> 5-3=2, VF=1
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x03, 0x61, 0x05, 0x80, 0x17]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x02);
        assert_eq!(vm.v[0xF], 1);
    }

    #[test]
    fn alu_shifts_capture_lost_bit() {
        // 8XY6 SHR: 6005 (101b), 8006 -> 2, VF=1 (lost LSB)
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x80, 0x06]);
        vm.step();
        vm.step();
        assert_eq!(vm.v[0], 0x02);
        assert_eq!(vm.v[0xF], 1);

        // 8XYE SHL: 6081 (10000001b), 800E -> 0x02, VF=1 (lost MSB)
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x81, 0x80, 0x0E]);
        vm.step();
        vm.step();
        assert_eq!(vm.v[0], 0x02);
        assert_eq!(vm.v[0xF], 1);
    }

    #[test]
    fn skip_se_immediate() {
        // 3XNN skips the next instruction when Vx == NN.
        // 6005 V0=5, 3005 SE V0,5 -> skip, 60FF (skipped), 61AB V1=0xAB
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x30, 0x05, 0x60, 0xFF, 0x61, 0xAB]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x05); // 60FF was skipped
        assert_eq!(vm.v[1], 0xAB); // landed on 61AB

        // No skip when Vx != NN: 6005, 3006 (5 != 6), 60FF runs
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x30, 0x06, 0x60, 0xFF]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0xFF); // not skipped
    }

    #[test]
    fn skip_sne_immediate() {
        // 4XNN skips when Vx != NN.
        // 6005, 4006 (5 != 6) -> skip, 60FF (skipped), 61AB
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x40, 0x06, 0x60, 0xFF, 0x61, 0xAB]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x05);
        assert_eq!(vm.v[1], 0xAB);

        // No skip when equal: 6005, 4005, 60FF runs
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x40, 0x05, 0x60, 0xFF]);
        for _ in 0..3 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0xFF);
    }

    #[test]
    fn skip_se_sne_register() {
        // 5XY0 skips when Vx == Vy.
        // 6005, 6105, 5010 -> skip, 60FF (skipped), 62AB
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x61, 0x05, 0x50, 0x10, 0x60, 0xFF, 0x62, 0xAB]);
        for _ in 0..4 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x05);
        assert_eq!(vm.v[2], 0xAB);

        // 9XY0 skips when Vx != Vy.
        // 6005, 6106, 9010 -> skip, 60FF (skipped), 62AB
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x61, 0x06, 0x90, 0x10, 0x60, 0xFF, 0x62, 0xAB]);
        for _ in 0..4 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0x05);
        assert_eq!(vm.v[2], 0xAB);

        // 9XY0 does NOT skip when equal: 6005, 6105, 9010, 60FF runs
        let mut vm = Chip8::new();
        vm.load_rom(&[0x60, 0x05, 0x61, 0x05, 0x90, 0x10, 0x60, 0xFF]);
        for _ in 0..4 {
            vm.step();
        }
        assert_eq!(vm.v[0], 0xFF);
    }

    #[test]
    fn jump_with_v0_offset() {
        // BNNN jumps to NNN + V0. With V0 = 4 and base 0x208, land on 0x20C.
        // 6004 V0=4, B208 JP 0x208+V0 -> pc=0x20C, where 61AB V1=0xAB lives.
        // Layout: 0x200 6004 | 0x202 B208 | 0x204..0x20B filler | 0x20C 61AB
        let mut rom = vec![0x60, 0x04, 0xB2, 0x08];
        rom.resize(0x0C, 0x00); // pad up to offset 0x0C (RAM 0x20C)
        rom.extend_from_slice(&[0x61, 0xAB]);
        let mut vm = Chip8::new();
        vm.load_rom(&rom);
        vm.step(); // 6004
        vm.step(); // B208
        assert_eq!(vm.pc, 0x20C);
        vm.step(); // 61AB at the jump target
        assert_eq!(vm.v[1], 0xAB);
    }

    // Collect the first `count` CXNN results into V0, with mask NN and an
    // optional reseed applied after load_rom.
    fn rnd_sequence(nn: u8, count: usize, seed: Option<u32>) -> Vec<u8> {
        let mut rom = Vec::new();
        for _ in 0..count {
            rom.push(0xC0); // CX with X=0
            rom.push(nn);
        }
        let mut vm = Chip8::new();
        vm.load_rom(&rom);
        if let Some(s) = seed {
            vm.reseed(s);
        }
        (0..count)
            .map(|_| {
                vm.step();
                vm.v[0]
            })
            .collect()
    }

    #[test]
    fn rnd_respects_mask() {
        // CX0F: every result must fit in the low nibble.
        for v in rnd_sequence(0x0F, 64, None) {
            assert_eq!(v & 0xF0, 0);
        }
        // CX00: mask of zero is always zero.
        assert!(rnd_sequence(0x00, 16, None).iter().all(|&v| v == 0));
    }

    #[test]
    fn rnd_is_deterministic_until_reseeded() {
        // Default seed -> identical sequences across fresh machines.
        assert_eq!(rnd_sequence(0xFF, 8, None), rnd_sequence(0xFF, 8, None));
        // A different seed produces a different sequence.
        assert_ne!(
            rnd_sequence(0xFF, 8, None),
            rnd_sequence(0xFF, 8, Some(0x1234_5678))
        );
        // reseed(0) falls back to the default seed.
        assert_eq!(rnd_sequence(0xFF, 8, None), rnd_sequence(0xFF, 8, Some(0)));
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
