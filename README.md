# Mars
Mars is a WIP OS kernel, currently only targetting ARMv8.

## Prerequisites
- Rust compiler (MSRV 1.94.0-nightly) with aarch64-unknown-none target support
- QEMU

## Features
- UEFI
- ACPI
- SMP (PSCI)
- Virtual Memory
- Kernel Heap (slab allocator)
