use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
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
        "image-riscv" => build_rootfs_image("riscv64"),
        "image-loongarch" => build_rootfs_image("loongarch64"),
        "check" => cargo(&["check", "--workspace"]),
        "qemu" | "qemu-riscv" => qemu_riscv(),
        "qemu-loongarch" => qemu_loongarch(),
        "oscomp-riscv" => oscomp_riscv(),
        "oscomp-loongarch" => oscomp_loongarch(),
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
    let cargo = env::var_os("CARGO")
        .or_else(|| {
            rustc_sysroot().and_then(|sysroot| {
                let cargo = sysroot.join("bin").join("cargo");
                cargo.exists().then_some(cargo.into_os_string())
            })
        })
        .or_else(|| {
            env::var_os("HOME").and_then(|home| {
                let cargo = PathBuf::from(home)
                    .join(".rustup")
                    .join("toolchains")
                    .join("stable-x86_64-unknown-linux-gnu")
                    .join("bin")
                    .join("cargo");
                cargo.exists().then_some(cargo.into_os_string())
            })
        })
        .unwrap_or_else(|| "cargo".into());
    match Command::new(cargo).args(args).status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute cargo: {err}");
            ExitCode::from(1)
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under tools/")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn rootfs_source_dir() -> PathBuf {
    repo_root().join("tools").join("rootfs").join("common")
}

fn rootfs_stage_dir(arch: &str) -> PathBuf {
    repo_root().join("target").join("rootfs").join(format!("{arch}-stage"))
}

fn rootfs_image_path(arch: &str) -> PathBuf {
    repo_root().join("target").join("rootfs").join(format!("{arch}.ext4"))
}

fn build_rootfs_image(arch: &str) -> ExitCode {
    let stage = rootfs_stage_dir(arch);
    let image = rootfs_image_path(arch);
    if let Err(err) = prepare_rootfs_stage(arch, &stage) {
        eprintln!("failed to prepare rootfs stage: {err}");
        return ExitCode::from(1);
    }
    if let Some(parent) = image.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            eprintln!("failed to create rootfs output directory: {err}");
            return ExitCode::from(1);
        }
    }
    let size = env::var("WHUSE_ROOTFS_SIZE_MB")
        .map(|value| {
            if value.ends_with('K') || value.ends_with('M') || value.ends_with('G') {
                value
            } else {
                format!("{value}M")
            }
        })
        .unwrap_or_else(|_| "64M".to_string());
    if let Err(err) = Command::new("truncate")
        .args(["-s", size.as_str(), image.to_string_lossy().as_ref()])
        .status()
    {
        eprintln!("failed to execute truncate: {err}");
        return ExitCode::from(1);
    }
    let status = Command::new("mke2fs")
        .args([
            "-t",
            "ext4",
            "-d",
            stage.to_string_lossy().as_ref(),
            "-F",
            image.to_string_lossy().as_ref(),
        ])
        .status();
    match status {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute mke2fs: {err}");
            ExitCode::from(1)
        }
    }
}

fn prepare_rootfs_stage(arch: &str, stage: &PathBuf) -> io::Result<()> {
    if stage.exists() {
        fs::remove_dir_all(stage)?;
    }
    fs::create_dir_all(stage)?;
    copy_tree(&rootfs_source_dir(), stage)?;
    fs::create_dir_all(stage.join("dev"))?;
    fs::create_dir_all(stage.join("proc"))?;
    fs::create_dir_all(stage.join("tmp"))?;
    fs::write(stage.join("etc").join("issue"), format!("whuse {arch} ext4 rootfs\n"))?;
    Ok(())
}

fn copy_tree(src: &PathBuf, dst: &PathBuf) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir_all(&to)?;
            copy_tree(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn oscomp_riscv() -> ExitCode {
    let image_status = build_rootfs_image("riscv64");
    if image_status != ExitCode::SUCCESS {
        return image_status;
    }
    qemu_riscv_with_disk(Some(rootfs_image_path("riscv64")))
}

fn oscomp_loongarch() -> ExitCode {
    let image_status = build_rootfs_image("loongarch64");
    if image_status != ExitCode::SUCCESS {
        return image_status;
    }
    qemu_loongarch_with_disk(Some(rootfs_image_path("loongarch64")))
}

fn qemu_riscv() -> ExitCode {
    qemu_riscv_with_disk(env::var("WHUSE_DISK_IMAGE").ok().map(PathBuf::from).or_else(|| {
        let image = rootfs_image_path("riscv64");
        image.exists().then_some(image)
    }))
}

fn qemu_riscv_with_disk(disk: Option<PathBuf>) -> ExitCode {
    let build_status = build_kernel(RISCV_PACKAGE, RISCV_TARGET);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }

    let kernel = PathBuf::from("target")
        .join(RISCV_TARGET)
        .join("debug")
        .join(RISCV_PACKAGE);

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
        command.arg(format!("file={},if=none,format=raw,id=x0", disk.display()));
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
    qemu_loongarch_with_disk(env::var("WHUSE_DISK_IMAGE").ok().map(PathBuf::from).or_else(|| {
        let image = rootfs_image_path("loongarch64");
        image.exists().then_some(image)
    }))
}

fn qemu_loongarch_with_disk(disk: Option<PathBuf>) -> ExitCode {
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
        command.arg(format!("file={},if=none,format=raw,id=x0", disk.display()));
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
