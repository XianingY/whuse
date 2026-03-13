use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const TARGET: &str = "riscv64gc-unknown-none-elf";
const KERNEL_PACKAGE: &str = "whuse-riscv64-virt";

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "build".to_string());
    match command.as_str() {
        "build" => cargo(&["build", "-p", KERNEL_PACKAGE, "--target", TARGET]),
        "check" => cargo(&["check", "--workspace"]),
        "qemu" => qemu(),
        other => {
            eprintln!("unknown xtask command: {other}");
            ExitCode::from(2)
        }
    }
}

fn cargo(args: &[&str]) -> ExitCode {
    match Command::new("cargo").args(args).status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute cargo: {err}");
            ExitCode::from(1)
        }
    }
}

fn qemu() -> ExitCode {
    let build_status = cargo(&["build", "-p", KERNEL_PACKAGE, "--target", TARGET]);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }

    let kernel = PathBuf::from("target")
        .join(TARGET)
        .join("debug")
        .join(KERNEL_PACKAGE);

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

    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute qemu-system-riscv64: {err}");
            ExitCode::from(1)
        }
    }
}

