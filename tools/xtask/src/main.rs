use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const RISCV_TARGET: &str = "riscv64gc-unknown-none-elf";
const RISCV_PACKAGE: &str = "whuse-riscv64-virt";
const LOONGARCH_TARGET: &str = "loongarch64-unknown-none-softfloat";
const LOONGARCH_PACKAGE: &str = "whuse-loongarch64-virt";

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "build".to_string());
    match command.as_str() {
        "build" | "build-riscv" => build_kernel(RISCV_PACKAGE, RISCV_TARGET),
        "build-loongarch" => build_kernel(LOONGARCH_PACKAGE, LOONGARCH_TARGET),
        "check" => cargo(&["check", "--workspace"]),
        "qemu" | "qemu-riscv" => qemu_riscv(),
        "qemu-loongarch" => qemu_loongarch(),
        "oscomp-riscv" => qemu_riscv(),
        "oscomp-loongarch" => qemu_loongarch(),
        other => {
            eprintln!("unknown xtask command: {other}");
            ExitCode::from(2)
        }
    }
}

fn build_kernel(package: &str, target: &str) -> ExitCode {
    cargo(&["build", "-p", package, "--target", target])
}

fn cargo(args: &[&str]) -> ExitCode {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    match Command::new(cargo).args(args).status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute cargo: {err}");
            ExitCode::from(1)
        }
    }
}

fn qemu_riscv() -> ExitCode {
    let build_status = build_kernel(RISCV_PACKAGE, RISCV_TARGET);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }

    let kernel = PathBuf::from("target")
        .join(RISCV_TARGET)
        .join("debug")
        .join(RISCV_PACKAGE);
    let disk = env::var("WHUSE_DISK_IMAGE").ok();

    let mut command = Command::new("qemu-system-riscv64");
    command.args([
        "-machine",
        "virt",
        "-m",
        "256M",
        "-smp",
        "1",
        "-nographic",
        "-bios",
        "default",
        "-kernel",
    ]);
    command.arg(kernel);
    if let Some(disk) = disk {
        command.arg("-drive");
        command.arg(format!("file={disk},if=none,format=raw,id=x0"));
        command.args([
            "-device",
            "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
            "-device",
            "virtio-net-device,netdev=net0",
            "-netdev",
            "user,id=net0",
        ]);
    }

    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute qemu-system-riscv64: {err}");
            ExitCode::from(1)
        }
    }
}

fn qemu_loongarch() -> ExitCode {
    let build_status = build_kernel(LOONGARCH_PACKAGE, LOONGARCH_TARGET);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }

    let kernel = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(LOONGARCH_PACKAGE);
    let disk = env::var("WHUSE_DISK_IMAGE").ok();

    let mut command = Command::new("qemu-system-loongarch64");
    command.args([
        "-machine",
        "virt",
        "-m",
        "1G",
        "-smp",
        "1",
        "-nographic",
        "-kernel",
    ]);
    command.arg(kernel);
    if let Some(disk) = disk {
        command.arg("-drive");
        command.arg(format!("file={disk},if=none,format=raw,id=x0"));
        command.args([
            "-device",
            "virtio-blk-pci,drive=x0",
            "-device",
            "virtio-net-pci,netdev=net0",
            "-netdev",
            "user,id=net0",
        ]);
    }

    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute qemu-system-loongarch64: {err}");
            ExitCode::from(1)
        }
    }
}
