# apogee

![CI](https://github.com/bxrne/apogee/actions/workflows/ci.yml/badge.svg)

A minimal x86-64 bare-metal kernel written in Rust. Runs directly on hardware or in QEMU with no underlying OS. Built to explore foundational kernel concepts: bootable binaries, VGA text output, serial I/O, CPU exception handling, and memory segmentation — all without the standard library.

## Requirements

- [Rust](https://rustup.rs/) (nightly, pinned via `rust-toolchain.toml`)
- [bootimage](https://github.com/rust-osdev/bootimage): `cargo install bootimage`
- [QEMU](https://www.qemu.org/): `sudo apt install qemu-system-x86`

## Usage

```sh
cargo run        # build bootimage and launch in QEMU
cargo test       # run all test binaries in headless QEMU
```

Individual test suites:

```sh
cargo test --test basic_boot
cargo test --test should_panic
cargo test --test stack_overflow
```

## Target

Custom target `x86_64-unknown-none` with:

- `panic-strategy: abort` — no unwinding
- `disable-redzone: true` — required for interrupt safety
- `features: -mmx,-sse,+soft-float` — no FPU state in kernel mode
- `linker: rust-lld` — no host linker dependency

## Testing

Tests run inside QEMU. The kernel communicates results over the serial port (`-serial stdio`) and signals pass/fail to the host via the `isa-debug-exit` QEMU device at I/O port `0xf4`. Exit code `33` (`(0x10 << 1) | 1`) maps to success.

