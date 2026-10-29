# apogee

![CI](https://github.com/bxrne/apogee/actions/workflows/ci.yml/badge.svg)

A minimal x86-64 bare-metal kernel written in Rust. Runs directly on hardware or in QEMU with no underlying OS.

## Requirements

Everything is provided by the Nix flake:

- Rust nightly, pinned to the same date as `rust-toolchain.toml`
- `rust-src`, `llvm-tools-preview`, and `rust-analyzer`
- [bootimage](https://github.com/rust-osdev/bootimage) 0.10.5
- [QEMU](https://www.qemu.org/) and GDB

Without Nix, install [Rust](https://rustup.rs/) (nightly, pinned via `rust-toolchain.toml`), `cargo install bootimage`, and `qemu-system-x86`.

## Usage

```sh
nix develop      # flake dev shell, or `nix-shell` via shell.nix
# or, with nix-direnv:
direnv allow

cargo run        # build bootimage and launch in QEMU
cargo test       # run all test binaries in headless QEMU
```

The shell gives the nightly toolchain and builds `bootimage` from the published crate, so no `cargo install` step is needed.

## Design

- A single `KERNEL` root composing small services: process, thread, scheduler, memory, timer, input, console.
- A typed request/response service boundary with caller identity, shared by kernel callers and the `int 0x80` adapter.
- Cooperative kernel threads with real context switches, plus a lock-free scheduler counter.
- Cooperative async tasks on top of waker notifications.
- Interrupt handlers that use lock-free service entry points.

## Memory management

The kernel implements dynamic memory allocation using a fixed-size-block (slab) allocator with a coalescing linked-list fallback:

- **Allocator**: slab lists for sizes 8 B–4 KiB, backed by an address-ordered free list that reuses freed regions and merges adjacent ones
- **Heap**: 100 KiB heap starting at virtual address `0x_4444_4444_0000`
- **Reference Counting**: Full `alloc::Rc<T>` support for shared ownership
- **Collections**: `Vec<T>`, `Box<T>`, and `Rc<T>` all work out of the box

## Testing

Tests run inside QEMU. The kernel communicates results over the serial port (`-serial stdio`) and signals pass/fail to the host via the `isa-debug-exit` QEMU device at I/O port `0xf4`. Exit code `33` (`(0x10 << 1) | 1`) maps to success.

## Roadmap

### Implemented

- Paging and frame allocation, slab / linked-list / bump allocators, `Rc` support.
- Kernel threads with real context switches and a lock-free scheduler.
- Cooperative async task executor with wakers.
- Interrupt handling and the `int 0x80` syscall path, including user-buffer validation and fault recovery.
- Serial test harness that runs the suite in headless QEMU.

### To implement

- Arena allocator.
- Observable process exit state and zombie / reaper semantics.
- Per-process address spaces.
- Syscall coverage for more operations.
- ELF loading and execution.

## Goal

Run real ELFs and become a functional OS.
