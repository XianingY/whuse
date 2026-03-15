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
        "oscomp-images" => prepare_oscomp_images(),
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
    qemu_riscv_with_disk(Some(oscomp_image_path("rv")))
}

fn oscomp_loongarch() -> ExitCode {
    let image_status = prepare_oscomp_images();
    if image_status != ExitCode::SUCCESS {
        return image_status;
    }
    qemu_loongarch_with_disk(Some(oscomp_image_path("la")))
}

fn qemu_riscv() -> ExitCode {
    qemu_riscv_with_disk(
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
    )
}

fn qemu_riscv_with_disk(disk: Option<PathBuf>) -> ExitCode {
    let build_status = build_kernel(RISCV_PACKAGE, RISCV_TARGET);
    if build_status != ExitCode::SUCCESS {
        return build_status;
    }
    if let Some(disk) = disk.as_ref() {
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
    qemu_loongarch_with_disk(
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
    )
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
    if let Some(disk) = disk.as_ref() {
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
    if let Err(err) = fs::copy(&src_rv, &dst_rv) {
        eprintln!(
            "failed to copy {} -> {}: {err}",
            src_rv.display(),
            dst_rv.display()
        );
        return ExitCode::from(1);
    }
    if let Err(err) = fs::copy(&src_la, &dst_la) {
        eprintln!(
            "failed to copy {} -> {}: {err}",
            src_la.display(),
            dst_la.display()
        );
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

fn build_oscomp_sdcard(testsuits: &PathBuf) -> bool {
    let status = Command::new("make")
        .arg("sdcard")
        .current_dir(testsuits)
        .status();
    match status {
        Ok(status) if status.success() => return true,
        Ok(status) => eprintln!(
            "host make sdcard failed in {} (exit code {:?}), trying docker fallback",
            testsuits.display(),
            status.code()
        ),
        Err(err) => eprintln!(
            "failed to execute host make sdcard in {} ({err}), trying docker fallback",
            testsuits.display()
        ),
    }

    let docker_image = env::var("WHUSE_OSCOMP_DOCKER_IMAGE")
        .unwrap_or_else(|_| "docker.educg.net/cg/os-contest:20250614".to_string());
    let mount_arg = format!("{}:/code", testsuits.display());
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
            docker_image.as_str(),
            "-lc",
            "make sdcard",
        ])
        .status();
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!(
                "docker make sdcard failed in {} (exit code {:?})",
                testsuits.display(),
                status.code()
            );
            false
        }
        Err(err) => {
            eprintln!(
                "failed to execute docker make sdcard in {} ({err})",
                testsuits.display()
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

    let required_musl = [
        "busybox",
        "busybox_testcode.sh",
        "busybox_cmd.txt",
        "iozone_testcode.sh",
        "libctest_testcode.sh",
        "libc-bench",
        "lmbench_testcode.sh",
        "lua_testcode.sh",
        "unixbench_testcode.sh",
        "netperf_testcode.sh",
        "iperf_testcode.sh",
        "cyclictest_testcode.sh",
    ];
    let mut missing = Vec::new();
    for entry in required_musl {
        if !musl_listing.contains(entry) {
            missing.push(format!("/musl/{entry}"));
        }
    }
    if !musl_listing.contains("test_all.sh") && !basic_listing.contains("run-all.sh") {
        missing.push("/musl/test_all.sh(or /musl/basic/run-all.sh)".to_string());
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
