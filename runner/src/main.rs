use std::{
    env,
    fs::{self, canonicalize},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use tempfile::tempdir;

fn workspace_root() -> PathBuf {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set...");
    let dir = PathBuf::from(manifest_dir).parent().unwrap().to_path_buf();

    println!("{}", dir.to_string_lossy());

    dir
}

fn main() -> Result<()> {
    let root = workspace_root();

    let kernel_status = Command::new("cargo")
        .args(["build", "--package", "mars_kernel"])
        .status()
        .context("Kernel build failed.")?;

    if !kernel_status.success() {
        anyhow::bail!("Kernel build failed.");
    }

    let boot_status = Command::new("cargo")
        .args(["build", "--package", "mars_bootloader"])
        .status()
        .context("Bootloader build failed.")?;

    if !boot_status.success() {
        anyhow::bail!("Bootloader build failed.");
    }

    let kernel_path = root
        .join("target/aarch64-unknown-none/debug/mars_kernel")
        .canonicalize()
        .unwrap();
    let boot_path = root
        .join("target/aarch64-unknown-uefi/debug/mars_bootloader.efi")
        .canonicalize()
        .unwrap();

    let tmp = tempdir()?;
    let esp_dir = tmp.path();

    fs::create_dir_all(esp_dir.join("EFI/BOOT"))?;
    std::os::unix::fs::symlink(kernel_path, esp_dir.join("kernel.elf"))?;
    std::os::unix::fs::symlink(boot_path, esp_dir.join("EFI/BOOT/BOOTAA64.EFI"))?;

    let code_path = env::var("OVMF_CODE_PATH").context("missing OVMF_CODE_PATH")?;

    let qemu_status = Command::new("qemu-system-aarch64")
        .args([
            "-M",
            "virt",
            "-accel",
            "hvf",
            "-cpu",
            "max",
            "-boot",
            "menu=on,order=c,splash-time=0",
            "-m",
            "768M",
            "-drive",
            &format!("if=pflash,format=raw,readonly=on,file={}", code_path),
            "-drive",
            &format!("file=fat:rw:{},if=virtio", esp_dir.to_string_lossy()),
            "-serial",
            "mon:stdio",
            "-device",
            "virtio-gpu-pci",
            "-S",
            "-s",
        ])
        .status()
        .context("QEMU failed.")?;

    if !qemu_status.success() {
        anyhow::bail!("QEMU failed.");
    }

    Ok(())
}
