use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const RISCV_TARGET: &str = "riscv64gc-unknown-none-elf";
const RISCV_PACKAGE: &str = "whuse-riscv64-virt";
const LOONGARCH_TARGET: &str = "loongarch64-unknown-none-softfloat";
const LOONGARCH_PACKAGE: &str = "whuse-loongarch64-virt";
const LOONGARCH_BOOTROM_PACKAGE: &str = "whuse-loongarch64-bootrom";

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

fn objcopy_to_binary(input: &PathBuf, output: &PathBuf) -> ExitCode {
    let bundled = bundled_rust_objcopy();
    let path_objcopy = ["llvm-objcopy", "rust-objcopy", "objcopy"]
        .into_iter()
        .find(|candidate| Command::new(candidate).arg("--version").output().is_ok())
        .map(PathBuf::from);
    let Some(objcopy) = bundled.clone().or(path_objcopy) else {
        eprintln!("failed to locate llvm-objcopy, rust-objcopy, or objcopy");
        return ExitCode::from(1);
    };

    let mut command = Command::new(&objcopy);
    if bundled.as_ref() == Some(&objcopy) {
        if let Some(ld_library_path) = bundled_rustc_lib_dir() {
            command.env("LD_LIBRARY_PATH", prepend_env_path("LD_LIBRARY_PATH", &ld_library_path));
        }
    }
    let status = command
        .args(["-O", "binary"])
        .arg(input)
        .arg(output)
        .status();
    match status {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute {}: {err}", objcopy.display());
            ExitCode::from(1)
        }
    }
}

fn bundled_rust_objcopy() -> Option<PathBuf> {
    let sysroot = rustc_sysroot()?;
    let rustlib = sysroot.join("lib").join("rustlib");
    for entry in fs::read_dir(rustlib).ok()? {
        let entry = entry.ok()?;
        let objcopy = entry.path().join("bin").join("rust-objcopy");
        if objcopy.exists() {
            return Some(objcopy);
        }
    }
    None
}

fn bundled_rustc_lib_dir() -> Option<PathBuf> {
    let lib_dir = rustc_sysroot()?.join("lib");
    lib_dir.exists().then_some(lib_dir)
}

fn rustc_sysroot() -> Option<PathBuf> {
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let output = Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sysroot = String::from_utf8(output.stdout).ok()?;
    let sysroot = sysroot.trim();
    (!sysroot.is_empty()).then(|| PathBuf::from(sysroot))
}

fn prepend_env_path(var: &str, prefix: &PathBuf) -> OsString {
    let mut combined = OsString::from(prefix);
    if let Some(existing) = env::var_os(var) {
        combined.push(":");
        combined.push(existing);
    }
    combined
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
    let bootrom_status = build_kernel(LOONGARCH_BOOTROM_PACKAGE, LOONGARCH_TARGET);
    if bootrom_status != ExitCode::SUCCESS {
        return bootrom_status;
    }
    let build_status = build_kernel(LOONGARCH_PACKAGE, LOONGARCH_TARGET);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }

    let bootrom_elf = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(LOONGARCH_BOOTROM_PACKAGE);
    let bootrom_bin = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(format!("{LOONGARCH_BOOTROM_PACKAGE}.bin"));
    let kernel_elf = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(LOONGARCH_PACKAGE);
    let kernel_bin = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(format!("{LOONGARCH_PACKAGE}.bin"));
    let bootrom_objcopy = objcopy_to_binary(&bootrom_elf, &bootrom_bin);
    if bootrom_objcopy != ExitCode::SUCCESS {
        return bootrom_objcopy;
    }
    let kernel_objcopy = objcopy_to_binary(&kernel_elf, &kernel_bin);
    if kernel_objcopy != ExitCode::SUCCESS {
        return kernel_objcopy;
    }
    let disk = env::var("WHUSE_DISK_IMAGE").ok();

    let mut command = Command::new("qemu-system-loongarch64");
    command.args([
        "-machine",
        "virt",
        "-cpu",
        "la464",
        "-m",
        "1G",
        "-smp",
        "1",
        "-nographic",
        "-serial",
        "stdio",
        "-monitor",
        "none",
        "-bios",
    ]);
    command.arg(&bootrom_bin);
    command.arg("-device");
    command.arg(format!(
        "loader,file={},addr=0x90000000,force-raw=on",
        kernel_bin.display()
    ));
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
