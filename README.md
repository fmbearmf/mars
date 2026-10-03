# Mars
Mars is a WIP kernel for ARMv8 (aka AArch64).

## Tested On
* QEMU (TCG)
* Orange Pi 6 Plus

## Prerequisites
* Rust nightly compiler with support for `aarch64-unknown-none` and `aarch64-unknown-uefi`
* Verus (formal verification toolchain)
* Nix (recommended)

## Features
* UEFI
* ACPI
* SMP (multicore execution)
* Virtual Memory
* Memory Allocation (slab + buddy)
* Threading
* Preemptive Scheduling
* Block Devices
* PCIe
* Partial formal verification

## Planned Features (in order of priority)
* Multi-queue block I/O
* Filesystem
* Mach-O binary support
* Syscall Layer
