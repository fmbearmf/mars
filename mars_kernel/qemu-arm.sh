#!/usr/bin/env bash
set -euo pipefail

qemu-system-aarch64 \
    -M virt \
    -accel hvf \
    -cpu host \
    -m 768 \
    -nographic \
    -serial stdio \
    -monitor none \
    -S -s \
    -device loader,file="$1",cpu-num=0
