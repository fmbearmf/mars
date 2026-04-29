# Mars
Mars is a WIP kernel for ARMv8 (aka AArch64).

## Prerequisites
* Rust nightly compiler with support for `aarch64-unknown-none` and `aarch64-unknown-uefi`
* Nix (recommended)
* QEMU (may or may not work on real hardware)

## Features
* UEFI
* ACPI
* SMP (PSCI)
* Virtual Memory
* Memory Allocation (page + heap)
* Preemptive Scheduling (WIP)

## Planned Features (in order of priority)
* Formal Driver Verification ([F*](https://fstar-lang.org))
* Filesystem
* Block Devices
* Syscalls
* Other userspace tasks
*
