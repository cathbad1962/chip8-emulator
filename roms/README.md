# Test ROMs

Small, freely redistributable CHIP-8 programs for exercising the emulator.
Load one with the **Load ROM…** button in the app (or `Chip8::load_rom`).

- **`ibm-logo.ch8`** (132 bytes) — the classic public-domain "IBM logo" test ROM.
  It clears the screen and draws the letters `IBM` using `DXYN` sprite draws, then
  spins forever. A good first-light test: if the logo renders crisply, the draw
  opcode, `I`-register addressing, and the framebuffer are all working. It uses no
  input, timers, or randomness.

- **`beep-test.ch8`** (6 bytes) — a hand-written audio test (original bytes, not
  copyrighted). It loads `0xFF` into the sound timer (`FX18`) and loops, refilling
  it every iteration, so `is_beeping()` stays true and the shell plays a steady
  440 Hz tone until you close the window. Use it to confirm audio output works.
