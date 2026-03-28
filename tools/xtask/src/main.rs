use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

const RISCV_TARGET: &str = "riscv64gc-unknown-none-elf";
const RISCV_PACKAGE: &str = "whuse-riscv64-virt";
const LOONGARCH_TARGET: &str = "loongarch64-unknown-none";
const LOONGARCH_PACKAGE: &str = "whuse-loongarch64-virt";
const LOONGARCH_BOOTROM_PACKAGE: &str = "whuse-loongarch64-bootrom";
const CONTEST_DOCKER_IMAGE: &str = "docker.educg.net/cg/os-contest:20260104";
const CONTEST_TOOLCHAIN: &str = "nightly-2025-01-18";
const OSCOMP_PROFILE_REPO_PATH: &str = "tools/oscomp/profile/default.txt";
const OSCOMP_PROFILE_IMAGE_PATH: &str = "/whuse-oscomp-profile";
const OSCOMP_ALLOWED_PROFILES: &[&str] = &[
    "full",
    "basic",
    "busybox",
    "iozone",
    "libctest",
    "libc-bench",
    "lmbench",
    "lua",
    "ltp",
    "unixbench",
    "netperf",
    "iperf",
    "cyclic",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QemuMode {
    Host,
    Contest,
}

#[derive(Clone, Debug)]
enum ContestRuntime {
    Docker(String),
    Local,
}

impl QemuMode {
    fn from_env(default: QemuMode) -> QemuMode {
        match env::var("WHUSE_QEMU_MODE")
            .unwrap_or_else(|_| {
                match default {
                    QemuMode::Host => "host",
                    QemuMode::Contest => "contest",
                }
                .to_string()
            })
            .as_str()
        {
            "host" => QemuMode::Host,
            "contest" => QemuMode::Contest,
            other => {
                eprintln!("unknown WHUSE_QEMU_MODE={other}, fallback to {:?}", default);
                default
            }
        }
    }
}

impl ContestRuntime {
    fn detect() -> ContestRuntime {
        let contest_image = env::var("WHUSE_OSCOMP_DOCKER_IMAGE")
            .unwrap_or_else(|_| CONTEST_DOCKER_IMAGE.to_string());
        if command_available("docker", &["version"]) {
            ContestRuntime::Docker(contest_image)
        } else {
            ContestRuntime::Local
        }
    }

    fn label(&self) -> &'static str {
        match self {
            ContestRuntime::Docker(_) => "docker",
            ContestRuntime::Local => "local",
        }
    }
}

fn main() -> ExitCode {
    let command = env::args().nth(1).unwrap_or_else(|| "build".to_string());
    match command.as_str() {
        "build" | "build-riscv" => build_riscv_artifact(),
        "build-loongarch" => build_loongarch_artifact(),
        "image-riscv" => build_rootfs_image("riscv64"),
        "image-loongarch" => build_rootfs_image("loongarch64"),
        "check" => cargo(&["check", "--workspace"]),
        "qemu" | "qemu-riscv" => qemu_riscv(),
        "qemu-riscv-contest" => qemu_riscv_mode(QemuMode::Contest),
        "qemu-loongarch" => qemu_loongarch(),
        "qemu-loongarch-contest" => qemu_loongarch_mode(QemuMode::Contest),
        "oscomp-images" => prepare_oscomp_images(),
        "oscomp-riscv" => oscomp_riscv(),
        "oscomp-loongarch" => oscomp_loongarch(),
        "contest-selfcheck" => contest_selfcheck(),
        other => {
            eprintln!("unknown xtask command: {other}");
            ExitCode::from(2)
        }
    }
}

fn build_kernel(package: &str, target: &str) -> ExitCode {
    cargo(&["build", "-p", package, "--target", target])
}

fn build_riscv_artifact() -> ExitCode {
    let status = build_kernel(RISCV_PACKAGE, RISCV_TARGET);
    if status != ExitCode::SUCCESS {
        return status;
    }
    let built_kernel = PathBuf::from("target")
        .join(RISCV_TARGET)
        .join("debug")
        .join(RISCV_PACKAGE);
    match package_kernel_artifact(&built_kernel, "kernel-rv") {
        Ok(path) => {
            println!("packaged RISC-V contest kernel {}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn build_loongarch_artifact() -> ExitCode {
    let status = build_kernel(LOONGARCH_PACKAGE, LOONGARCH_TARGET);
    if status != ExitCode::SUCCESS {
        return status;
    }
    let built_kernel = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(LOONGARCH_PACKAGE);
    match package_kernel_artifact(&built_kernel, "kernel-la") {
        Ok(path) => {
            println!("packaged LoongArch contest kernel {}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
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
            command.env(
                "LD_LIBRARY_PATH",
                prepend_env_path("LD_LIBRARY_PATH", &ld_library_path),
            );
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

fn rustc_binary() -> OsString {
    env::var_os("RUSTC")
        .or_else(|| {
            rustc_sysroot().map(|sysroot| sysroot.join("bin").join("rustc").into_os_string())
        })
        .unwrap_or_else(|| "rustc".into())
}

fn bundled_rust_lld() -> Option<PathBuf> {
    let sysroot = rustc_sysroot()?;
    let rustlib = sysroot.join("lib").join("rustlib");
    for entry in fs::read_dir(rustlib).ok()? {
        let entry = entry.ok()?;
        let lld = entry.path().join("bin").join("rust-lld");
        if lld.exists() {
            return Some(lld);
        }
    }
    None
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

fn command_available(binary: &str, args: &[&str]) -> bool {
    Command::new(binary).args(args).output().is_ok()
}

fn repo_root() -> PathBuf {
    if let Ok(cwd) = env::current_dir() {
        if cwd.join("Cargo.toml").exists()
            && cwd.join("tools").join("xtask").join("Cargo.toml").exists()
        {
            return cwd;
        }
    }
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
    repo_root()
        .join("target")
        .join("rootfs")
        .join(format!("{arch}-stage"))
}

fn rootfs_image_path(arch: &str) -> PathBuf {
    repo_root()
        .join("target")
        .join("rootfs")
        .join(format!("{arch}.ext4"))
}

fn package_kernel_artifact(source: &Path, output_name: &str) -> Result<PathBuf, String> {
    let output = repo_root().join(output_name);
    fs::copy(source, &output)
        .map(|_| output.clone())
        .map_err(|err| {
            format!(
                "failed to package kernel artifact {} -> {}: {err}",
                source.display(),
                output.display()
            )
        })
}

fn package_riscv_kernel_artifact(source: &Path, output_name: &str) -> Result<PathBuf, String> {
    let target_dir = repo_root().join("target").join("xtask").join("riscv-raw");
    fs::create_dir_all(&target_dir)
        .map_err(|err| format!("failed to create {}: {err}", target_dir.display()))?;

    let staged_raw = target_dir.join(format!("{output_name}.raw"));
    let raw_status = objcopy_to_binary(&source.to_path_buf(), &staged_raw);
    if raw_status != ExitCode::SUCCESS {
        return Err(format!(
            "failed to package raw riscv kernel payload {} -> {}",
            source.display(),
            staged_raw.display()
        ));
    }
    let output = repo_root().join(output_name);
    fs::copy(&staged_raw, &output).map_err(|err| {
        format!(
            "failed to copy {} -> {}: {err}",
            staged_raw.display(),
            output.display()
        )
    })?;

    Ok(output)
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
    fs::write(
        stage.join("etc").join("issue"),
        format!("whuse {arch} ext4 rootfs\n"),
    )?;
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
    let image_status = prepare_oscomp_images();
    if image_status != ExitCode::SUCCESS {
        return image_status;
    }
    qemu_riscv_with_disk_and_mode(
        Some(oscomp_image_path("rv")),
        QemuMode::from_env(QemuMode::Contest),
    )
}

fn oscomp_loongarch() -> ExitCode {
    let image_status = prepare_oscomp_images();
    if image_status != ExitCode::SUCCESS {
        return image_status;
    }
    qemu_loongarch_with_disk_and_mode(
        Some(oscomp_image_path("la")),
        QemuMode::from_env(QemuMode::Contest),
    )
}

fn qemu_riscv() -> ExitCode {
    qemu_riscv_mode(QemuMode::from_env(QemuMode::Host))
}

fn qemu_riscv_mode(mode: QemuMode) -> ExitCode {
    qemu_riscv_with_disk_and_mode(
        env::var("WHUSE_DISK_IMAGE")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                let image = oscomp_image_path("rv");
                image.exists().then_some(image)
            })
            .or_else(|| {
                let image = rootfs_image_path("riscv64");
                image.exists().then_some(image)
            }),
        mode,
    )
}

fn qemu_riscv_with_disk_and_mode(disk: Option<PathBuf>, mode: QemuMode) -> ExitCode {
    let build_status = build_kernel(RISCV_PACKAGE, RISCV_TARGET);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }
    let extra_disk = extra_disk_path("disk.img");
    if mode == QemuMode::Host {
        for disk in [disk.as_ref(), extra_disk.as_ref()].into_iter().flatten() {
            if let Some((pid, cmdline)) = detect_qemu_disk_holder(disk) {
                eprintln!(
                    "disk image {} is currently in use by pid {} ({})",
                    disk.display(),
                    pid,
                    cmdline
                );
                eprintln!("stop the running qemu process (or use a different image) and retry");
                return ExitCode::from(1);
            }
        }
    }

    let built_kernel = PathBuf::from("target")
        .join(RISCV_TARGET)
        .join("debug")
        .join(RISCV_PACKAGE);
    let packaged_kernel = match package_kernel_artifact(&built_kernel, "kernel-rv") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let kernel = packaged_kernel;
    let args = build_qemu_riscv_args(&kernel, disk.as_ref(), extra_disk.as_ref());
    let used_paths = collect_used_paths(&[Some(kernel.clone()), disk.clone(), extra_disk.clone()]);
    run_qemu("qemu-system-riscv64", &args, &used_paths, mode)
}

fn qemu_loongarch() -> ExitCode {
    qemu_loongarch_mode(QemuMode::from_env(QemuMode::Host))
}

fn qemu_loongarch_mode(mode: QemuMode) -> ExitCode {
    qemu_loongarch_with_disk_and_mode(
        env::var("WHUSE_DISK_IMAGE")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                let image = oscomp_image_path("la");
                image.exists().then_some(image)
            })
            .or_else(|| {
                let image = rootfs_image_path("loongarch64");
                image.exists().then_some(image)
            }),
        mode,
    )
}

fn qemu_loongarch_with_disk_and_mode(disk: Option<PathBuf>, mode: QemuMode) -> ExitCode {
    let build_status = build_kernel(LOONGARCH_PACKAGE, LOONGARCH_TARGET);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }
    let extra_disk = extra_disk_path("disk-la.img");
    if mode == QemuMode::Host {
        for disk in [disk.as_ref(), extra_disk.as_ref()].into_iter().flatten() {
            if let Some((pid, cmdline)) = detect_qemu_disk_holder(disk) {
                eprintln!(
                    "disk image {} is currently in use by pid {} ({})",
                    disk.display(),
                    pid,
                    cmdline
                );
                eprintln!("stop the running qemu process (or use a different image) and retry");
                return ExitCode::from(1);
            }
        }
    }

    let kernel_elf = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(LOONGARCH_PACKAGE);
    let packaged_kernel = match package_kernel_artifact(&kernel_elf, "kernel-la") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    if mode == QemuMode::Contest {
        let args =
            build_qemu_loongarch_contest_args(&packaged_kernel, disk.as_ref(), extra_disk.as_ref());
        let used_paths =
            collect_used_paths(&[Some(packaged_kernel), disk.clone(), extra_disk.clone()]);
        return run_qemu("qemu-system-loongarch64", &args, &used_paths, mode);
    }

    let bootrom_status = build_kernel(LOONGARCH_BOOTROM_PACKAGE, LOONGARCH_TARGET);
    if bootrom_status != ExitCode::SUCCESS {
        return bootrom_status;
    }
    let bootrom_elf = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(LOONGARCH_BOOTROM_PACKAGE);
    let bootrom_bin = PathBuf::from("target")
        .join(LOONGARCH_TARGET)
        .join("debug")
        .join(format!("{LOONGARCH_BOOTROM_PACKAGE}.bin"));
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

    let args = build_qemu_loongarch_host_args(
        &bootrom_bin,
        &kernel_bin,
        disk.as_ref(),
        extra_disk.as_ref(),
    );
    let used_paths = collect_used_paths(&[
        Some(bootrom_bin.clone()),
        Some(kernel_bin.clone()),
        disk.clone(),
        extra_disk.clone(),
    ]);
    run_qemu("qemu-system-loongarch64", &args, &used_paths, mode)
}

fn build_qemu_riscv_args(
    kernel: &Path,
    disk: Option<&PathBuf>,
    extra_disk: Option<&PathBuf>,
) -> Vec<String> {
    let memory = env::var("WHUSE_QEMU_RISCV_MEM").unwrap_or_else(|_| "1G".to_string());
    let mut args = vec![
        "-machine".to_string(),
        "virt".to_string(),
        "-m".to_string(),
        memory,
        "-smp".to_string(),
        "1".to_string(),
        "-nographic".to_string(),
        "-bios".to_string(),
        "default".to_string(),
        "-kernel".to_string(),
        kernel.display().to_string(),
        "-no-reboot".to_string(),
        "-rtc".to_string(),
        "base=utc".to_string(),
    ];
    if let Some(disk) = disk {
        args.push("-drive".to_string());
        args.push(format!(
            "file={},if=none,format=raw,id=x0,file.locking=off",
            disk.display()
        ));
        args.push("-device".to_string());
        args.push("virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0".to_string());
        args.push("-device".to_string());
        args.push("virtio-net-device,netdev=net".to_string());
        args.push("-netdev".to_string());
        args.push("user,id=net".to_string());
    }
    if let Some(extra) = extra_disk {
        args.push("-drive".to_string());
        args.push(format!(
            "file={},if=none,format=raw,id=x1,file.locking=off",
            extra.display()
        ));
        args.push("-device".to_string());
        args.push("virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1".to_string());
    }
    args
}

fn build_qemu_loongarch_host_args(
    bootrom_bin: &Path,
    kernel_bin: &Path,
    disk: Option<&PathBuf>,
    extra_disk: Option<&PathBuf>,
) -> Vec<String> {
    let memory = env::var("WHUSE_QEMU_LOONGARCH_MEM").unwrap_or_else(|_| "1G".to_string());
    let mut args = vec![
        "-machine".to_string(),
        "virt".to_string(),
        "-cpu".to_string(),
        "la464".to_string(),
        "-m".to_string(),
        memory,
        "-smp".to_string(),
        "1".to_string(),
        "-nographic".to_string(),
        "-serial".to_string(),
        "stdio".to_string(),
        "-monitor".to_string(),
        "none".to_string(),
        "-bios".to_string(),
        bootrom_bin.display().to_string(),
        "-device".to_string(),
        format!(
            "loader,file={},addr=0x90000000,force-raw=on",
            kernel_bin.display()
        ),
        "-no-reboot".to_string(),
        "-rtc".to_string(),
        "base=utc".to_string(),
    ];
    if let Some(disk) = disk {
        args.push("-drive".to_string());
        args.push(format!(
            "file={},if=none,format=raw,id=x0,file.locking=off",
            disk.display()
        ));
        args.push("-device".to_string());
        args.push("virtio-blk-pci,drive=x0".to_string());
    }
    if let Some(extra) = extra_disk {
        args.push("-drive".to_string());
        args.push(format!(
            "file={},if=none,format=raw,id=x1,file.locking=off",
            extra.display()
        ));
        args.push("-device".to_string());
        args.push("virtio-blk-pci,drive=x1".to_string());
    }
    args.push("-device".to_string());
    args.push("virtio-net-pci,netdev=net0".to_string());
    args.push("-netdev".to_string());
    args.push("user,id=net0".to_string());
    args
}

fn build_qemu_loongarch_contest_args(
    kernel: &Path,
    disk: Option<&PathBuf>,
    extra_disk: Option<&PathBuf>,
) -> Vec<String> {
    let memory = env::var("WHUSE_QEMU_LOONGARCH_MEM").unwrap_or_else(|_| "1G".to_string());
    let mut args = vec![
        "-machine".to_string(),
        "virt".to_string(),
        "-kernel".to_string(),
        kernel.display().to_string(),
        "-m".to_string(),
        memory,
        "-nographic".to_string(),
        "-smp".to_string(),
        "1".to_string(),
        "-no-reboot".to_string(),
        "-rtc".to_string(),
        "base=utc".to_string(),
    ];
    if let Some(disk) = disk {
        args.push("-drive".to_string());
        args.push(format!(
            "file={},if=none,format=raw,id=x0,file.locking=off",
            disk.display()
        ));
        args.push("-device".to_string());
        args.push("virtio-blk-pci,drive=x0".to_string());
    }
    args.push("-device".to_string());
    args.push("virtio-net-pci,netdev=net0".to_string());
    args.push("-netdev".to_string());
    args.push("user,id=net0".to_string());
    if let Some(extra) = extra_disk {
        args.push("-drive".to_string());
        args.push(format!(
            "file={},if=none,format=raw,id=x1,file.locking=off",
            extra.display()
        ));
        args.push("-device".to_string());
        args.push("virtio-blk-pci,drive=x1".to_string());
    }
    args
}

fn extra_disk_path(default_name: &str) -> Option<PathBuf> {
    if let Ok(value) = env::var("WHUSE_EXTRA_DISK_IMAGE") {
        let path = PathBuf::from(value);
        return Some(if path.is_absolute() {
            path
        } else {
            repo_root().join(path)
        });
    }
    let default = repo_root().join(default_name);
    default.exists().then_some(default)
}

fn collect_used_paths(paths: &[Option<PathBuf>]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| path.as_ref().cloned())
        .collect()
}

fn run_qemu(binary: &str, args: &[String], used_paths: &[PathBuf], mode: QemuMode) -> ExitCode {
    match mode {
        QemuMode::Host => match Command::new(binary).args(args).status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(err) => {
                eprintln!("failed to execute {binary}: {err}");
                ExitCode::from(1)
            }
        },
        QemuMode::Contest => run_qemu_in_contest_docker(binary, args, used_paths),
    }
}

fn run_qemu_in_contest_docker(binary: &str, args: &[String], used_paths: &[PathBuf]) -> ExitCode {
    let image =
        env::var("WHUSE_OSCOMP_DOCKER_IMAGE").unwrap_or_else(|_| CONTEST_DOCKER_IMAGE.to_string());
    let root = repo_root();
    let root_canonical = fs::canonicalize(&root).unwrap_or(root);
    let mount_root = format!("{}:/work", root_canonical.display());
    let extra_mounts = collect_extra_mounts(used_paths, &root_canonical);

    let mut command = Command::new("docker");
    command
        .arg("run")
        .arg("--rm")
        .arg("--privileged")
        .arg("--network")
        .arg("host")
        .arg("-v")
        .arg(mount_root)
        .arg("-w")
        .arg("/work");
    for mount in extra_mounts {
        command.arg("-v").arg(mount);
    }
    command.arg(image).arg(binary);
    for arg in args {
        command.arg(remap_qemu_arg_for_container(arg, &root_canonical));
    }

    match command.status() {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(err) => {
            eprintln!("failed to execute contest docker qemu runner: {err}");
            ExitCode::from(1)
        }
    }
}

fn collect_extra_mounts(paths: &[PathBuf], repo_root: &Path) -> Vec<String> {
    let mut mounts = BTreeSet::new();
    for path in paths {
        if path.as_os_str().is_empty() {
            continue;
        }
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if canonical.starts_with(repo_root) {
            continue;
        }
        if let Some(parent) = canonical.parent() {
            mounts.insert(format!("{}:{}", parent.display(), parent.display()));
        }
    }
    mounts.into_iter().collect()
}

fn remap_qemu_arg_for_container(arg: &str, repo_root: &Path) -> String {
    if let Some(file_index) = arg.find("file=") {
        let start = file_index + "file=".len();
        let end = arg[start..]
            .find(',')
            .map(|idx| start + idx)
            .unwrap_or(arg.len());
        let mut out = arg.to_string();
        let original = &arg[start..end];
        let mapped = remap_path_for_container(original, repo_root);
        out.replace_range(start..end, &mapped);
        return out;
    }
    if arg.starts_with('/') {
        return remap_path_for_container(arg, repo_root);
    }
    arg.to_string()
}

fn remap_path_for_container(path: &str, repo_root: &Path) -> String {
    let candidate = PathBuf::from(path);
    let canonical = fs::canonicalize(&candidate).unwrap_or(candidate.clone());
    if canonical.starts_with(repo_root) {
        if let Ok(relative) = canonical.strip_prefix(repo_root) {
            return Path::new("/work").join(relative).display().to_string();
        }
    }
    canonical.display().to_string()
}

fn oscomp_image_path(tag: &str) -> PathBuf {
    repo_root()
        .join("target")
        .join("oscomp")
        .join(format!("sdcard-{tag}.img"))
}

fn testsuits_root() -> PathBuf {
    env::var("WHUSE_OSCOMP_TESTSUITS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            repo_root()
                .parent()
                .expect("workspace has parent directory")
                .join("testsuits-for-oskernel")
        })
}

fn prepare_oscomp_images() -> ExitCode {
    let testsuits = testsuits_root();
    if !testsuits.exists() {
        eprintln!(
            "testsuits directory does not exist: {} (set WHUSE_OSCOMP_TESTSUITS_DIR)",
            testsuits.display()
        );
        return ExitCode::from(1);
    }

    if env::var("WHUSE_OSCOMP_SKIP_BUILD").is_err() {
        if !build_oscomp_sdcard(&testsuits) {
            eprintln!(
                "warning: oscomp sdcard build failed; will try to use existing images under {}",
                testsuits.display()
            );
        }
    }

    let src_rv = match ensure_oscomp_source_image(&testsuits, "rv") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let src_la = match ensure_oscomp_source_image(&testsuits, "la") {
        Ok(path) => path,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };

    let target_dir = repo_root().join("target").join("oscomp");
    if let Err(err) = fs::create_dir_all(&target_dir) {
        eprintln!(
            "failed to create target oscomp directory {}: {err}",
            target_dir.display()
        );
        return ExitCode::from(1);
    }
    let dst_rv = oscomp_image_path("rv");
    let dst_la = oscomp_image_path("la");
    if let Err(err) = refresh_oscomp_image(&src_rv, &dst_rv) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    if let Err(err) = refresh_oscomp_image(&src_la, &dst_la) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    if let Err(err) = purge_runtime_oscomp_overrides(&dst_rv) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    if let Err(err) = purge_runtime_oscomp_overrides(&dst_la) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    let repo_profile = match load_repo_oscomp_profile() {
        Ok(profile) => profile,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = debugfs_write_text(&dst_rv, OSCOMP_PROFILE_IMAGE_PATH, &repo_profile) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    if let Err(err) = debugfs_write_text(&dst_la, OSCOMP_PROFILE_IMAGE_PATH, &repo_profile) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }

    let rv_ok = validate_oscomp_full_image(&dst_rv, "riscv64");
    let la_ok = validate_oscomp_full_image(&dst_la, "loongarch64");
    if !rv_ok || !la_ok {
        return ExitCode::from(1);
    }
    println!(
        "prepared oscomp images:\n  {}\n  {}",
        dst_rv.display(),
        dst_la.display()
    );
    ExitCode::SUCCESS
}

fn contest_selfcheck() -> ExitCode {
    let mut ok = true;
    let root = repo_root();
    let runtime = ContestRuntime::detect();

    let cargo_config = root.join("cargo_config.toml");
    if cargo_config.exists() {
        println!("contest-selfcheck: cargo-config={}", cargo_config.display());
    } else {
        eprintln!(
            "contest-selfcheck: missing {} (contest clone filters .cargo; keep non-hidden cargo config in repo root)",
            cargo_config.display()
        );
        ok = false;
    }

    let vendor_dir = root.join("vendor");
    if vendor_dir.exists() {
        println!("contest-selfcheck: vendor-dir={}", vendor_dir.display());
    } else {
        eprintln!(
            "contest-selfcheck: missing {} (contest builds must not depend on online registry downloads)",
            vendor_dir.display()
        );
        ok = false;
    }

    let expected_artifacts = ["kernel-rv", "kernel-la"];
    for artifact in expected_artifacts {
        let path = root.join(artifact);
        if path.exists() {
            println!("contest-selfcheck: found artifact {}", path.display());
            if artifact == "kernel-rv" {
                if is_elf_artifact(&path, 0xf3) {
                    println!("contest-selfcheck: kernel-rv-format=elf");
                } else {
                    println!("contest-selfcheck: kernel-rv-format=raw");
                }
            }
        } else {
            eprintln!(
                "contest-selfcheck: missing artifact {} (run `make all` first)",
                path.display()
            );
            ok = false;
        }
    }

    let testsuits = testsuits_root();
    if testsuits.exists() {
        println!("contest-selfcheck: testsuits={}", testsuits.display());
        if testsuits.join(".git").exists() {
            if let Ok(branch) = git_output(&testsuits, &["rev-parse", "--abbrev-ref", "HEAD"]) {
                println!("contest-selfcheck: testsuits-branch={}", branch);
                if branch.trim() != "pre-2025" {
                    eprintln!(
                        "contest-selfcheck: testsuits branch is {}, expected pre-2025",
                        branch
                    );
                    ok = false;
                }
            }
        }
    } else {
        eprintln!(
            "contest-selfcheck: testsuits missing at {} (set WHUSE_OSCOMP_TESTSUITS_DIR)",
            testsuits.display()
        );
        ok = false;
    }

    println!("contest-selfcheck: runtime={}", runtime.label());
    match contest_toolchain_version(&runtime, CONTEST_TOOLCHAIN) {
        Ok(version) => {
            println!("contest-selfcheck: rust-toolchain={version}");
        }
        Err(err) => {
            eprintln!(
                "contest-selfcheck: contest image missing preinstalled toolchain {}: {err}",
                CONTEST_TOOLCHAIN
            );
            ok = false;
        }
    }
    match contest_installed_targets(&runtime, CONTEST_TOOLCHAIN) {
        Ok(installed) => {
            println!("contest-selfcheck: rust-targets={}", installed.join(","));
            for target in [RISCV_TARGET, LOONGARCH_TARGET] {
                if !installed
                    .iter()
                    .any(|installed_target| installed_target == target)
                {
                    eprintln!(
                        "contest-selfcheck: contest image toolchain {} is missing target {}",
                        CONTEST_TOOLCHAIN, target
                    );
                    ok = false;
                }
            }
        }
        Err(err) => {
            eprintln!(
                "contest-selfcheck: failed to inspect targets for {}: {err}",
                CONTEST_TOOLCHAIN
            );
            ok = false;
        }
    }
    match contest_qemu_version(&runtime, "qemu-system-riscv64") {
        Ok(version) => {
            println!("contest-selfcheck: riscv-qemu={}", version);
            if !contest_qemu_version_supported(&version) {
                eprintln!("contest-selfcheck: expected qemu 9.2.1 or 10.0.2 in contest image");
                ok = false;
            }
        }
        Err(err) => {
            eprintln!("contest-selfcheck: failed to probe riscv qemu version: {err}");
            ok = false;
        }
    }
    match contest_qemu_version(&runtime, "qemu-system-loongarch64") {
        Ok(version) => {
            println!("contest-selfcheck: loongarch-qemu={}", version);
            if !contest_qemu_version_supported(&version) {
                eprintln!("contest-selfcheck: expected qemu 9.2.1 or 10.0.2 in contest image");
                ok = false;
            }
        }
        Err(err) => {
            eprintln!("contest-selfcheck: failed to probe loongarch qemu version: {err}");
            ok = false;
        }
    }

    let rv_args = build_qemu_riscv_args(
        Path::new("kernel-rv"),
        Some(&PathBuf::from("sdcard-rv.img")),
        Some(&PathBuf::from("disk.img")),
    );
    let rv_line = rv_args.join(" ");
    println!("contest-selfcheck: riscv-args={}", rv_line);
    if !line_contains_all(
        &rv_line,
        &[
            "-machine virt",
            "-kernel kernel-rv",
            "-no-reboot",
            "-rtc base=utc",
            "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
            "virtio-net-device,netdev=net",
            "user,id=net",
            "virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1",
        ],
    ) {
        eprintln!("contest-selfcheck: riscv contest args drift from expected profile");
        ok = false;
    }

    let la_args = build_qemu_loongarch_contest_args(
        Path::new("kernel-la"),
        Some(&PathBuf::from("sdcard-la.img")),
        Some(&PathBuf::from("disk-la.img")),
    );
    let la_line = la_args.join(" ");
    println!("contest-selfcheck: loongarch-args={}", la_line);
    if !line_contains_all(
        &la_line,
        &[
            "-kernel kernel-la",
            "-no-reboot",
            "-rtc base=utc",
            "virtio-blk-pci,drive=x0",
            "virtio-net-pci,netdev=net0",
            "user,id=net0",
            "virtio-blk-pci,drive=x1",
        ],
    ) {
        eprintln!("contest-selfcheck: loongarch contest args drift from expected profile");
        ok = false;
    }

    for (tag, arch) in [("rv", "riscv64"), ("la", "loongarch64")] {
        let image = oscomp_image_path(tag);
        if image.exists() {
            if !validate_oscomp_full_image(&image, arch) {
                ok = false;
            }
        } else {
            eprintln!(
                "contest-selfcheck: oscomp image missing {} (run `cargo run --manifest-path tools/xtask/Cargo.toml -- oscomp-images` first)",
                image.display()
            );
        }
    }

    let rv_kernel = root.join("kernel-rv");
    let rv_image = oscomp_image_path("rv");
    if rv_kernel.exists() {
        match contest_riscv_boot_smoke(
            &runtime,
            &rv_kernel,
            rv_image.exists().then_some(rv_image.as_path()),
        ) {
            Ok(true) => println!("contest-selfcheck: kernel-rv-boot-smoke=ok"),
            Ok(false) => {
                eprintln!(
                    "contest-selfcheck: kernel-rv boot smoke did not reach contest scoring markers"
                );
                ok = false;
            }
            Err(err) => {
                eprintln!("contest-selfcheck: kernel-rv boot smoke failed: {err}");
                ok = false;
            }
        }
    }
    let la_kernel = root.join("kernel-la");
    let la_image = oscomp_image_path("la");
    if la_kernel.exists() && la_image.exists() {
        match contest_loongarch_boot_smoke(&runtime, &la_kernel, &la_image) {
            Ok(true) => println!("contest-selfcheck: kernel-la-boot-smoke=ok"),
            Ok(false) => {
                eprintln!(
                    "contest-selfcheck: kernel-la boot smoke did not reach contest scoring markers"
                );
                ok = false;
            }
            Err(err) => {
                eprintln!("contest-selfcheck: kernel-la boot smoke failed: {err}");
                ok = false;
            }
        }
    }

    if ok {
        println!("contest-selfcheck: PASS");
        ExitCode::SUCCESS
    } else {
        eprintln!("contest-selfcheck: FAIL");
        ExitCode::from(1)
    }
}

fn docker_qemu_version(image: &str, binary: &str) -> Result<String, String> {
    let output = Command::new("docker")
        .args(["run", "--rm", image, binary, "--version"])
        .output()
        .map_err(|err| format!("failed to execute docker run: {err}"))?;
    if !output.status.success() {
        return Err(format!("docker run exit code {:?}", output.status.code()));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let line = text.lines().next().unwrap_or_default().trim().to_string();
    if line.is_empty() {
        return Err("empty version output".to_string());
    }
    Ok(line)
}

fn docker_bash_output(image: &str, script: &str) -> Result<String, String> {
    let output = Command::new("docker")
        .args(["run", "--rm", image, "bash", "-lc", script])
        .output()
        .map_err(|err| format!("failed to execute docker bash: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "docker bash exit code {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn docker_toolchain_version(image: &str, toolchain: &str) -> Result<String, String> {
    let text = docker_bash_output(
        image,
        &format!(
            "rustup run {toolchain} cargo --version && rustup run {toolchain} rustc --version"
        ),
    )?;
    let line = text.lines().next().unwrap_or_default().trim().to_string();
    if line.is_empty() {
        return Err("empty cargo version output".to_string());
    }
    Ok(line)
}

fn docker_installed_targets(image: &str, toolchain: &str) -> Result<Vec<String>, String> {
    let target_toolchain = format!("{toolchain}-x86_64-unknown-linux-gnu");
    let text = docker_bash_output(
        image,
        &format!("rustup target list --toolchain {target_toolchain} --installed"),
    )?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn local_bash_output(script: &str) -> Result<String, String> {
    let output = Command::new("bash")
        .args(["-lc", script])
        .output()
        .map_err(|err| format!("failed to execute local bash: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "local bash exit code {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn local_toolchain_version(toolchain: &str) -> Result<String, String> {
    let text = local_bash_output(&format!(
        "rustup run {toolchain} cargo --version && rustup run {toolchain} rustc --version"
    ))?;
    let line = text.lines().next().unwrap_or_default().trim().to_string();
    if line.is_empty() {
        return Err("empty cargo version output".to_string());
    }
    Ok(line)
}

fn local_installed_targets(toolchain: &str) -> Result<Vec<String>, String> {
    let target_toolchain = format!("{toolchain}-x86_64-unknown-linux-gnu");
    let text = local_bash_output(&format!(
        "rustup target list --toolchain {target_toolchain} --installed"
    ))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn local_qemu_version(binary: &str) -> Result<String, String> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|err| format!("failed to execute local {binary}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "local {binary} exit code {:?}",
            output.status.code()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let line = text.lines().next().unwrap_or_default().trim().to_string();
    if line.is_empty() {
        return Err("empty version output".to_string());
    }
    Ok(line)
}

fn contest_toolchain_version(runtime: &ContestRuntime, toolchain: &str) -> Result<String, String> {
    match runtime {
        ContestRuntime::Docker(image) => docker_toolchain_version(image, toolchain),
        ContestRuntime::Local => local_toolchain_version(toolchain),
    }
}

fn contest_installed_targets(
    runtime: &ContestRuntime,
    toolchain: &str,
) -> Result<Vec<String>, String> {
    match runtime {
        ContestRuntime::Docker(image) => docker_installed_targets(image, toolchain),
        ContestRuntime::Local => local_installed_targets(toolchain),
    }
}

fn contest_qemu_version(runtime: &ContestRuntime, binary: &str) -> Result<String, String> {
    match runtime {
        ContestRuntime::Docker(image) => docker_qemu_version(image, binary),
        ContestRuntime::Local => local_qemu_version(binary),
    }
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|err| format!("failed to execute git {:?}: {err}", args))?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} exit code {:?}",
            args,
            output.status.code()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn line_contains_all(line: &str, tokens: &[&str]) -> bool {
    tokens.iter().all(|token| line.contains(token))
}

fn contest_qemu_version_supported(version: &str) -> bool {
    version.contains("9.2.1") || version.contains("10.0.2")
}

fn is_elf_artifact(path: &Path, expected_machine: u16) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    if bytes.len() < 20 || &bytes[..4] != b"\x7fELF" {
        return false;
    }
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    machine == expected_machine
}

fn contest_smoke_has_progress(text: &str) -> bool {
    text.contains("whuse-oscomp-script-start")
        || text.contains("#### OS COMP TEST GROUP START")
        || text.contains("testcase ")
}

fn collect_command_output(command: &mut Command, desc: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|err| format!("failed to execute {desc}: {err}"))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

fn make_temp_disk_copy(prefix: &str, source: &Path) -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("failed to read clock for temp image: {err}"))?
        .as_nanos();
    let temp = env::temp_dir().join(format!("{prefix}-{}-{nanos}.img", std::process::id()));
    let status = Command::new("cp")
        .args([
            "--reflink=auto",
            "--sparse=always",
            source.to_string_lossy().as_ref(),
            temp.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|err| {
            format!(
                "failed to execute cp for selfcheck smoke {} -> {}: {err}",
                source.display(),
                temp.display()
            )
        })?;
    if !status.success() {
        return Err(format!(
            "cp failed while creating selfcheck smoke image {} -> {} (exit code {:?})",
            source.display(),
            temp.display(),
            status.code()
        ));
    }
    Ok(temp)
}

fn contest_riscv_boot_smoke(
    runtime: &ContestRuntime,
    kernel: &Path,
    disk: Option<&Path>,
) -> Result<bool, String> {
    let repo = repo_root();
    let kernel_rel = kernel.strip_prefix(&repo).unwrap_or(kernel);
    let temp_disk = disk
        .map(|path| make_temp_disk_copy("whuse-selfcheck-rv", path))
        .transpose()?;
    let docker_disk = PathBuf::from("/tmp/whuse-selfcheck-rv.img");
    let disk_buf = temp_disk
        .as_deref()
        .or(disk)
        .map(|path| path.strip_prefix(&repo).unwrap_or(path).to_path_buf());
    let args = match runtime {
        ContestRuntime::Docker(_) => {
            build_qemu_riscv_args(kernel_rel, temp_disk.as_ref().map(|_| &docker_disk), None)
        }
        ContestRuntime::Local => build_qemu_riscv_args(kernel_rel, disk_buf.as_ref(), None),
    };
    let text = match runtime {
        ContestRuntime::Docker(image) => {
            let joined = args.join(" ");
            collect_command_output(
                Command::new("docker").args([
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/work", repo.display()),
                    "-v",
                    &format!(
                        "{}:/tmp/whuse-selfcheck-rv.img",
                        temp_disk
                            .as_ref()
                            .ok_or_else(|| "missing temp rv disk for docker smoke".to_string())?
                            .display()
                    ),
                    "-w",
                    "/work",
                    image,
                    "bash",
                    "-lc",
                    &format!("timeout 60s qemu-system-riscv64 {joined}"),
                ]),
                "docker riscv boot smoke",
            )?
        }
        ContestRuntime::Local => collect_command_output(
            Command::new("timeout")
                .arg("60s")
                .arg("qemu-system-riscv64")
                .args(&args),
            "local riscv boot smoke",
        )?,
    };
    if let Some(temp_disk) = temp_disk {
        let _ = fs::remove_file(temp_disk);
    }
    Ok(text.contains("whuse: booting on riscv64-virt") && contest_smoke_has_progress(&text))
}

fn contest_loongarch_boot_smoke(
    runtime: &ContestRuntime,
    kernel: &Path,
    disk: &Path,
) -> Result<bool, String> {
    let repo = repo_root();
    let kernel_rel = kernel.strip_prefix(&repo).unwrap_or(kernel);
    let temp_disk = make_temp_disk_copy("whuse-selfcheck-la", disk)?;
    let docker_disk = PathBuf::from("/tmp/whuse-selfcheck-la.img");
    let args = match runtime {
        ContestRuntime::Docker(_) => {
            build_qemu_loongarch_contest_args(kernel_rel, Some(&docker_disk), None)
        }
        ContestRuntime::Local => {
            build_qemu_loongarch_contest_args(kernel_rel, Some(&temp_disk), None)
        }
    };
    let text = match runtime {
        ContestRuntime::Docker(image) => {
            let joined = args.join(" ");
            collect_command_output(
                Command::new("docker").args([
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/work", repo.display()),
                    "-v",
                    &format!("{}:/tmp/whuse-selfcheck-la.img", temp_disk.display()),
                    "-w",
                    "/work",
                    image,
                    "bash",
                    "-lc",
                    &format!("timeout 60s qemu-system-loongarch64 {joined}"),
                ]),
                "docker loongarch boot smoke",
            )?
        }
        ContestRuntime::Local => collect_command_output(
            Command::new("timeout")
                .arg("60s")
                .arg("qemu-system-loongarch64")
                .args(&args),
            "local loongarch boot smoke",
        )?,
    };
    let _ = fs::remove_file(temp_disk);
    Ok(text.contains("whuse: booting on loongarch64-virt") && contest_smoke_has_progress(&text))
}

fn refresh_oscomp_image(src: &PathBuf, dst: &PathBuf) -> Result<(), String> {
    if same_file_metadata(src, dst)? {
        return Ok(());
    }
    fs::copy(src, dst).map(|_| ()).map_err(|err| {
        format!(
            "failed to copy {} -> {}: {err}",
            src.display(),
            dst.display()
        )
    })
}

fn same_file_metadata(src: &PathBuf, dst: &PathBuf) -> Result<bool, String> {
    if src == dst {
        return Ok(true);
    }

    let src_meta = fs::metadata(src)
        .map_err(|err| format!("failed to stat source image {}: {err}", src.display()))?;
    let dst_meta = match fs::metadata(dst) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(format!(
                "failed to stat target image {}: {err}",
                dst.display()
            ));
        }
    };

    if src_meta.len() != dst_meta.len() {
        return Ok(false);
    }

    let src_modified = src_meta
        .modified()
        .map_err(|err| format!("failed to read mtime for {}: {err}", src.display()))?;
    let dst_modified = dst_meta
        .modified()
        .map_err(|err| format!("failed to read mtime for {}: {err}", dst.display()))?;

    Ok(dst_modified >= src_modified)
}

fn validate_oscomp_profile_value(value: &str) -> Result<&'static str, String> {
    let trimmed = value.trim();
    OSCOMP_ALLOWED_PROFILES
        .iter()
        .copied()
        .find(|candidate| *candidate == trimmed)
        .ok_or_else(|| {
            format!(
                "invalid oscomp profile {:?}; expected one of: {}",
                trimmed,
                OSCOMP_ALLOWED_PROFILES.join(", ")
            )
        })
}

fn load_repo_oscomp_profile() -> Result<String, String> {
    let path = repo_root().join(OSCOMP_PROFILE_REPO_PATH);
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    validate_oscomp_profile_value(&contents).map(|profile| profile.to_string())
}

fn build_oscomp_sdcard(testsuits: &PathBuf) -> bool {
    let docker_image = env::var("WHUSE_OSCOMP_DOCKER_IMAGE")
        .unwrap_or_else(|_| "docker.educg.net/cg/os-contest:20260104".to_string());
    let host_first = env::var("WHUSE_OSCOMP_HOST_FIRST")
        .map(|value| value == "1")
        .unwrap_or(false);

    if !host_first {
        if run_make_sdcard_docker(testsuits, &docker_image) {
            return true;
        }
        eprintln!(
            "docker make sdcard failed in {}, trying host fallback",
            testsuits.display()
        );
    }

    if run_make_sdcard_host(testsuits) {
        return true;
    }

    if host_first {
        eprintln!(
            "host make sdcard failed in {}, trying docker fallback",
            testsuits.display()
        );
        return run_make_sdcard_docker(testsuits, &docker_image);
    }
    false
}

fn run_make_sdcard_host(testsuits: &PathBuf) -> bool {
    let status = Command::new("make")
        .arg("sdcard")
        .current_dir(testsuits)
        .status();
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!(
                "host make sdcard failed in {} (exit code {:?})",
                testsuits.display(),
                status.code()
            );
            false
        }
        Err(err) => {
            eprintln!(
                "failed to execute host make sdcard in {} ({err})",
                testsuits.display()
            );
            false
        }
    }
}

fn run_make_sdcard_docker(testsuits: &PathBuf, docker_image: &str) -> bool {
    let mount_root = fs::canonicalize(testsuits).unwrap_or_else(|_| testsuits.clone());
    let mount_arg = format!("{}:/code", mount_root.display());
    eprintln!(
        "building oscomp image in docker {} with workspace {}",
        docker_image,
        mount_root.display()
    );
    let status = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-v",
            mount_arg.as_str(),
            "--entrypoint",
            "bash",
            "-w",
            "/code",
            "--privileged",
            docker_image,
            "-lc",
            "make sdcard",
        ])
        .status();
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!(
                "docker make sdcard failed in {} (exit code {:?})",
                mount_root.display(),
                status.code()
            );
            false
        }
        Err(err) => {
            eprintln!(
                "failed to execute docker make sdcard in {} ({err})",
                mount_root.display()
            );
            false
        }
    }
}

fn ensure_oscomp_source_image(testsuits: &PathBuf, tag: &str) -> Result<PathBuf, String> {
    let raw = testsuits.join(format!("sdcard-{tag}.img"));
    if raw.exists() {
        return Ok(raw);
    }
    let compressed = testsuits.join(format!("sdcard-{tag}.img.xz"));
    if !compressed.exists() {
        return Err(format!(
            "missing oscomp image source {}, and compressed archive {}",
            raw.display(),
            compressed.display()
        ));
    }
    let status = Command::new("xz")
        .args(["-dk", compressed.to_string_lossy().as_ref()])
        .status()
        .map_err(|err| format!("failed to execute xz for {}: {err}", compressed.display()))?;
    if !status.success() {
        return Err(format!(
            "failed to decompress {} (exit code {:?})",
            compressed.display(),
            status.code()
        ));
    }
    if raw.exists() {
        Ok(raw)
    } else {
        Err(format!(
            "decompressed archive but image still missing: {}",
            raw.display()
        ))
    }
}

fn validate_oscomp_full_image(image: &PathBuf, arch: &str) -> bool {
    let musl_listing = match debugfs_list(image, "/musl") {
        Ok(listing) => listing,
        Err(err) => {
            eprintln!(
                "failed to run debugfs for {} ({}): {err}",
                arch,
                image.display()
            );
            return false;
        }
    };
    let basic_listing = match debugfs_list(image, "/musl/basic") {
        Ok(listing) => listing,
        Err(err) => {
            eprintln!(
                "failed to read /musl/basic via debugfs for {} ({}): {err}",
                arch,
                image.display()
            );
            return false;
        }
    };
    let glibc_listing = match debugfs_list(image, "/glibc") {
        Ok(listing) => listing,
        Err(err) => {
            eprintln!(
                "failed to read /glibc via debugfs for {} ({}): {err}",
                arch,
                image.display()
            );
            return false;
        }
    };
    let glibc_basic_listing = match debugfs_list(image, "/glibc/basic") {
        Ok(listing) => listing,
        Err(err) => {
            eprintln!(
                "failed to read /glibc/basic via debugfs for {} ({}): {err}",
                arch,
                image.display()
            );
            return false;
        }
    };

    let leaked_runtime_configs = [
        ".whuse_oscomp_only_step",
        ".whuse_ltp_profile",
        ".whuse_ltp_whitelist",
        ".whuse_ltp_blacklist",
        ".whuse_ltp_step_timeout",
        ".whuse_ltp_case_timeout",
        "ltp_score_whitelist.host.txt",
        "ltp_score_blacklist.host.txt",
    ];
    let leaked: Vec<_> = leaked_runtime_configs
        .iter()
        .copied()
        .filter(|entry| debugfs_exists(image, &format!("/musl/{entry}")))
        .collect();
    if !leaked.is_empty() {
        eprintln!(
            "oscomp image {} ({}) still contains runtime-only config files under /musl: {:?}",
            arch,
            image.display(),
            leaked
        );
        return false;
    }
    let profile = match debugfs_read_to_string(image, OSCOMP_PROFILE_IMAGE_PATH) {
        Ok(profile) => profile,
        Err(err) => {
            eprintln!(
                "oscomp image {} ({}) is missing stable profile {} ({err})",
                arch,
                image.display(),
                OSCOMP_PROFILE_IMAGE_PATH
            );
            return false;
        }
    };
    if let Err(err) = validate_oscomp_profile_value(&profile) {
        eprintln!(
            "oscomp image {} ({}) has invalid {}: {}",
            arch,
            image.display(),
            OSCOMP_PROFILE_IMAGE_PATH,
            err
        );
        return false;
    }

    let required_musl = [
        "busybox",
        "basic_testcode.sh",
        "busybox_testcode.sh",
        "libcbench_testcode.sh",
        "libc-bench",
        "iozone_testcode.sh",
        "iozone",
        "lua_testcode.sh",
        "lua",
        "lmbench_testcode.sh",
        "libctest_testcode.sh",
        "runtest.exe",
        "entry-static.exe",
        "entry-dynamic.exe",
        "ltp_testcode.sh",
    ];
    let required_glibc = [
        "busybox",
        "basic_testcode.sh",
        "busybox_testcode.sh",
        "libcbench_testcode.sh",
        "libc-bench",
        "iozone_testcode.sh",
        "iozone",
        "lua_testcode.sh",
        "lua",
        "lmbench_testcode.sh",
        "libctest_testcode.sh",
        "ltp_testcode.sh",
    ];
    let mut missing = Vec::new();
    for entry in required_musl {
        if !musl_listing.contains(entry) {
            missing.push(format!("/musl/{entry}"));
        }
    }
    for entry in required_glibc {
        if !glibc_listing.contains(entry) {
            missing.push(format!("/glibc/{entry}"));
        }
    }
    if !musl_listing.contains("test_all.sh") && !basic_listing.contains("run-all.sh") {
        missing.push("/musl/test_all.sh(or /musl/basic/run-all.sh)".to_string());
    }
    if !glibc_listing.contains("test_all.sh") && !glibc_basic_listing.contains("run-all.sh") {
        missing.push("/glibc/test_all.sh(or /glibc/basic/run-all.sh)".to_string());
    }
    if !missing.is_empty() {
        eprintln!(
            "oscomp image {} ({}) is incomplete; missing {:?}",
            arch,
            image.display(),
            missing
        );
        return false;
    }
    true
}

fn purge_runtime_oscomp_overrides(image: &PathBuf) -> Result<(), String> {
    for path in [
        "/musl/.whuse_oscomp_only_step",
        "/musl/.whuse_ltp_profile",
        "/musl/.whuse_ltp_whitelist",
        "/musl/.whuse_ltp_blacklist",
        "/musl/.whuse_ltp_step_timeout",
        "/musl/.whuse_ltp_case_timeout",
        "/musl/ltp_score_whitelist.host.txt",
        "/musl/ltp_score_blacklist.host.txt",
    ] {
        debugfs_remove(image, path)?;
    }
    Ok(())
}

fn debugfs_remove(image: &PathBuf, path: &str) -> Result<(), String> {
    let status = Command::new("debugfs")
        .args([
            "-w",
            "-R",
            &format!("rm {path}"),
            image.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|err| format!("failed to execute debugfs for rm {path}: {err}"))?;
    if status.success() {
        return Ok(());
    }
    Ok(())
}

fn debugfs_write_text(image: &PathBuf, path: &str, value: &str) -> Result<(), String> {
    let temp_path = env::temp_dir().join(format!(
        "whuse-oscomp-profile-{}-{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| format!("failed to read system clock: {err}"))?
            .as_nanos()
    ));
    fs::write(&temp_path, format!("{value}\n"))
        .map_err(|err| format!("failed to write temporary profile {}: {err}", temp_path.display()))?;
    debugfs_remove(image, path)?;
    let status = Command::new("debugfs")
        .args([
            "-w",
            "-R",
            &format!("write {} {path}", temp_path.to_string_lossy()),
            image.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|err| format!("failed to execute debugfs for write {path}: {err}"))?;
    let _ = fs::remove_file(&temp_path);
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to write {} into {} (exit code {:?})",
            path,
            image.display(),
            status.code()
        ))
    }
}

fn debugfs_list(image: &PathBuf, path: &str) -> Result<String, String> {
    let output = Command::new("debugfs")
        .args([
            "-R",
            &format!("ls -l {path}"),
            image.to_string_lossy().as_ref(),
        ])
        .output()
        .map_err(|err| format!("failed to execute debugfs: {err}"))?;
    if !output.status.success() {
        return Err(format!("debugfs exit code {:?}", output.status.code()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn debugfs_read_to_string(image: &PathBuf, path: &str) -> Result<String, String> {
    let output = Command::new("debugfs")
        .args([
            "-R",
            &format!("cat {path}"),
            image.to_string_lossy().as_ref(),
        ])
        .output()
        .map_err(|err| format!("failed to execute debugfs cat {path}: {err}"))?;
    if !output.status.success() {
        return Err(format!("debugfs exit code {:?}", output.status.code()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn debugfs_exists(image: &PathBuf, path: &str) -> bool {
    let Ok(output) = Command::new("debugfs")
        .args([
            "-R",
            &format!("stat {path}"),
            image.to_string_lossy().as_ref(),
        ])
        .output()
    else {
        return false;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.status.success()
        && !text.contains("File not found")
        && !text.contains("not found by ext2_lookup")
}

fn detect_qemu_disk_holder(disk: &PathBuf) -> Option<(u32, String)> {
    let canonical_disk = fs::canonicalize(disk).unwrap_or_else(|_| disk.clone());
    let canonical_str = canonical_disk.to_string_lossy().into_owned();
    let raw_str = disk.to_string_lossy().into_owned();
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let pid_str = file_name.to_string_lossy();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        let cmdline_path = entry.path().join("cmdline");
        let Ok(raw_cmdline) = fs::read(cmdline_path) else {
            continue;
        };
        if raw_cmdline.is_empty() {
            continue;
        }
        let args = raw_cmdline
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect::<Vec<_>>();
        if args.is_empty() {
            continue;
        }
        let joined = args.join(" ");
        if !joined.contains("qemu-system-") {
            continue;
        }
        if joined.contains(&canonical_str) || joined.contains(&raw_str) {
            return Some((pid, joined));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        build_qemu_loongarch_contest_args, build_qemu_riscv_args, validate_oscomp_profile_value,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn riscv_contest_args_match_expected_profile() {
        let args = build_qemu_riscv_args(
            Path::new("kernel-rv"),
            Some(&PathBuf::from("sdcard-rv.img")),
            Some(&PathBuf::from("disk.img")),
        )
        .join(" ");
        for token in [
            "-machine virt",
            "-kernel kernel-rv",
            "-no-reboot",
            "-rtc base=utc",
            "virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0",
            "virtio-net-device,netdev=net",
            "user,id=net",
            "virtio-blk-device,drive=x1,bus=virtio-mmio-bus.1",
        ] {
            assert!(args.contains(token), "missing token: {token}");
        }
    }

    #[test]
    fn loongarch_contest_args_match_expected_profile() {
        let args = build_qemu_loongarch_contest_args(
            Path::new("kernel-la"),
            Some(&PathBuf::from("sdcard-la.img")),
            Some(&PathBuf::from("disk-la.img")),
        )
        .join(" ");
        for token in [
            "-machine virt",
            "-kernel kernel-la",
            "-no-reboot",
            "-rtc base=utc",
            "virtio-blk-pci,drive=x0",
            "virtio-net-pci,netdev=net0",
            "user,id=net0",
            "virtio-blk-pci,drive=x1",
        ] {
            assert!(args.contains(token), "missing token: {token}");
        }
    }

    #[test]
    fn validates_known_oscomp_subset_profiles() {
        for profile in ["full", "basic", "busybox", "ltp", "cyclic"] {
            assert_eq!(validate_oscomp_profile_value(profile).unwrap(), profile);
            assert_eq!(
                validate_oscomp_profile_value(&format!("  {profile}\n")).unwrap(),
                profile
            );
        }
    }

    #[test]
    fn rejects_unknown_oscomp_subset_profiles() {
        assert!(validate_oscomp_profile_value("").is_err());
        assert!(validate_oscomp_profile_value("nope").is_err());
        assert!(validate_oscomp_profile_value("basic_testcode.sh").is_err());
    }
}
