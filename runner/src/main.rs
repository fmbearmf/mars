use std::{
    env,
    fs::{self},
    io::Write,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
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
    let mut test_lba_size = None;
    let mut timeout_test = false;
    for argument in env::args().skip(1) {
        test_lba_size = Some(match argument.as_str() {
            "--nvme-test" => 512,
            "--nvme-test-4k" => 4096,
            "--nvme-test-timeout" => {
                timeout_test = true;
                512
            }
            _ => anyhow::bail!("unknown runner option: {argument}"),
        });
    }

    let mut kernel_build = Command::new("cargo");
    kernel_build.args([
        "build",
        "-Z",
        "build-std=core,compiler_builtins,alloc",
        "--target",
        "aarch64-mars-none", // https://github.com/rust-lang/cargo/issues/15365
        "--package",
        "kernel",
    ]);
    if test_lba_size.is_some() {
        kernel_build.args(["--features", "nvme-self-test"]);
    }
    let kernel_status = kernel_build
        .env("RUSTFLAGS", "-Z unstable-options -Z emit-stack-sizes")
        .status()
        .context("Kernel build failed.")?;

    if !kernel_status.success() {
        anyhow::bail!("Kernel build failed.");
    }

    let boot_status = Command::new("cargo")
        .args(["build", "--package", "bootloader"])
        .status()
        .context("Bootloader build failed.")?;

    if !boot_status.success() {
        anyhow::bail!("Bootloader build failed.");
    }

    let kernel_path = root
        .join("target/aarch64-mars-none/debug/kernel")
        .canonicalize()
        .unwrap();
    let boot_path = root
        .join("target/aarch64-unknown-uefi/debug/bootloader.efi")
        .canonicalize()
        .unwrap();

    let tmp = tempdir()?;
    let esp_dir = tmp.path();

    fs::create_dir_all(esp_dir.join("EFI/BOOT"))?;
    std::os::unix::fs::symlink(kernel_path, esp_dir.join("kernel.elf"))?;
    std::os::unix::fs::symlink(boot_path, esp_dir.join("EFI/BOOT/BOOTAA64.EFI"))?;

    let code_path = env::var("OVMF_CODE_PATH").context("missing OVMF_CODE_PATH")?;

    let mut test_image = if test_lba_size.is_some() {
        Some(tempfile::NamedTempFile::new()?)
    } else {
        None
    };
    if let Some(image) = test_image.as_mut() {
        image.as_file_mut().set_len(16 * 1024 * 1024)?;
        image.write_all(b"MARS_NVME_TEST_IMAGE")?;
        image.flush()?;
    }
    let disk = test_image
        .as_ref()
        .map(|image| image.path().to_path_buf())
        .unwrap_or_else(|| root.join("sd.img"));
    let test_serial = if timeout_test {
        "MARS_NVME_TIMEOUT"
    } else {
        "MARS_NVME_TEST"
    };
    let nvme_device = match test_lba_size {
        Some(size) => format!(
            "nvme,drive=nvme0,serial={test_serial},bus=pcie.1,logical_block_size={size},physical_block_size={size}"
        ),
        None => "nvme,drive=nvme0,serial=nvme0,bus=pcie.1".into(),
    };
    let mut qemu = Command::new("qemu-system-aarch64");
    qemu.args([
        "-M",
        "virt,gic-version=3,its=on,virtualization=on",
        "-accel",
        "tcg",
        "-cpu",
        "neoverse-n2",
        "-boot",
        "menu=on,order=c,splash-time=0",
        "-m",
        "1G",
        "-drive",
        &format!("if=pflash,format=raw,readonly=on,file={}", code_path),
        "-drive",
        &format!("file=fat:rw:{},if=virtio", esp_dir.to_string_lossy()),
        "-device",
        "pcie-root-port,id=pcie.1,bus=pcie.0,chassis=1,slot=1",
        "-drive",
        &format!("file={},format=raw,if=none,id=nvme0", disk.display()),
        "-device",
        &nvme_device,
        "-device",
        "qemu-xhci,id=xhci",
        "-serial",
        "mon:stdio",
        //"-monitor",
        //"stdio",
        "-device",
        "virtio-gpu-pci",
        "-smp",
        "8",
        //"-D",
        //"qemu.log",
        //"-d",
        //"int,guest_errors",
        //"--trace",
        //"pl011_*",
        //"--trace",
        //"gicv3_*",
        //"-msg",
        //"timestamp=on",
        //"-S",
        //"-trace",
        //"events=trace-events.txt,file=gicv3_events.log",
    ]);
    if test_lba_size.is_some() {
        qemu.args([
            "-display",
            "none",
            "-semihosting-config",
            "enable=on,target=native",
            "-no-reboot",
        ]);
    } else {
        qemu.arg("-s");
    }
    let mut child = qemu.spawn().context("QEMU failed to start.")?;
    let start = Instant::now();
    let qemu_status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if test_lba_size.is_some() && start.elapsed() > Duration::from_secs(90) {
            child.kill()?;
            child.wait()?;
            anyhow::bail!("NVMe self-test timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    if test_lba_size.is_some() {
        if qemu_status.code() != Some(42) {
            anyhow::bail!("NVMe self-test did not report success: {qemu_status}");
        }
    } else if !qemu_status.success() {
        anyhow::bail!("QEMU failed.");
    }

    Ok(())
}
