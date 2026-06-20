---
name: add-opcode
description: Implement a CHIP-8 opcode family in chip8-core with a matching unit test. Use when the user wants to add, implement, or fix a CHIP-8 instruction/opcode (e.g. the 0x8 ALU family, 0x3/0x4/0x5/0x9 skips, 0xC random, 0xE key-skip, 0xF misc), or asks to "implement the next opcode".
---

# Implement a CHIP-8 opcode family

Goal: replace a `self.unimplemented(opcode)` stub in `chip8-core/src/lib.rs` with
correct logic, and prove it with a unit test. The pure, dependency-free core means every
opcode can be driven and asserted with zero UI.

## Steps

1. **Read `chip8-core/src/lib.rs`.** The decoder is a `match` on the top nibble inside
   `execute()`. Decoded fields are already in scope: `nnn` (12-bit addr), `nn` (8-bit imm),
   `n` (4-bit nibble), `x`, `y` (register indices). The stub arms carry comments describing
   each family's spec.

2. **Confirm which family** with the user if ambiguous. Recommended order of difficulty:
   `0x3/0x4/0x5/0x9` (skips) → `0xB` (jump+V0) → `0xE` (key skips) → `0x8` (ALU, highest payoff)
   → `0xC` (random) → `0xF` (timers/BCD/load-store, the largest sub-match).

3. **Implement the arm.** Match the existing style — small inline logic, or a `match` on the
   low byte (`nn`) / nibble (`n`) for multi-instruction families (`0x8`, `0xE`, `0xF`).
   Spec reminders:
   - Skips (`0x3/0x4/0x5/0x9`, `0xE`): "skip next instruction" means `self.pc += 2`.
   - `0x8` ALU: set `v[0xF]` to the carry/borrow/shift-out flag. Compute the flag from the
     **pre-operation** values; use `wrapping_*` arithmetic.
   - `0xC` RND: `v[x] = (random u8) & nn`. The core has no deps — add a tiny xorshift/LCG as a
     struct field (seeded in `new()`) rather than pulling in the `rand` crate.
   - `0xF` covers timers (`FX07/FX15/FX18`), wait-for-key (`FX0A`), `I += Vx` (`FX1E`), font
     address (`FX29` → `FONT_START + Vx*5`), BCD (`FX33`), and register dump/load (`FX55/FX65`).

4. **Add a `#[cfg(test)]` test** in the `tests` module: build a tiny ROM as `&[u8]`, `load_rom`,
   `step()` the right number of times, and assert on `vm.v[..]`, `vm.i`, RAM, or `framebuffer()`.
   Follow the two existing tests as templates. For skip opcodes, assert the resulting `pc` or
   that the skipped instruction's effect did/didn't happen.

5. **Verify:** from the repo root, run `cargo test -p chip8-core`. Then `cargo fmt`.

6. If you temporarily made `unimplemented` panic for debugging, restore it to a silent no-op
   before finishing.
