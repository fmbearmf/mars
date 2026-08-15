# Mars
Mars is a WIP kernel for ARMv8 (aka AArch64).

## Prerequisites
* Rust nightly compiler with support for `aarch64-unknown-none` and `aarch64-unknown-uefi`
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

## Planned Features (in order of priority)
* Filesystem
* Mach-O binary support
* Syscall Layer
