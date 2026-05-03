# apogee

![CI](https://github.com/bxrne/apogee/actions/workflows/ci.yml/badge.svg)

A minimal x86-64 bare-metal kernel written in Rust. Runs directly on hardware or in QEMU with no underlying OS.

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

## Memory Management

The kernel implements dynamic memory allocation using a linked-list allocator:

- **Allocator**: `linked_list_allocator` crate provides a bump allocator with free list management
- **Heap**: 100 KiB heap starting at virtual address `0x_4444_4444_0000`
- **Reference Counting**: Full `alloc::Rc<T>` support for shared ownership
- **Collections**: `Vec<T>`, `Box<T>`, and `Rc<T>` all work out of the box

## Testing

Tests run inside QEMU. The kernel communicates results over the serial port (`-serial stdio`) and signals pass/fail to the host via the `isa-debug-exit` QEMU device at I/O port `0xf4`. Exit code `33` (`(0x10 << 1) | 1`) maps to success.

### Integration Tests

```sh
cargo test --test basic_boot      # basic boot and println
cargo test --test should_panic    # panic handling
cargo test --test stack_overflow  # double fault on overflow
cargo test --test heap_allocation # heap alloc, Box, Vec, Rc
```

