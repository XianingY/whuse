use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_TIMEOUT_PROFILE");
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_REAL_PHASE");
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_GATE_LIBCTEST_SCOPE");
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_FULL_MAX_GROUP");
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_IOZONE_PROFILE");
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_IOZONE_FULL_SCOPE");

    let timeout_profile = match env::var("WHUSE_STAGE2_TIMEOUT_PROFILE")
        .unwrap_or_else(|_| String::from("real"))
        .as_str()
    {
        "chain-fast" => "chain-fast",
        _ => "real",
    };
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let default_real_phase = "full";
    let real_phase = match env::var("WHUSE_STAGE2_REAL_PHASE")
        .unwrap_or_else(|_| default_real_phase.to_string())
        .as_str()
    {
        "gate" => "gate",
        _ => "full",
    };
    let gate_libctest_scope = match env::var("WHUSE_STAGE2_GATE_LIBCTEST_SCOPE")
        .unwrap_or_else(|_| String::from("full"))
        .as_str()
    {
        "smoke" => "smoke",
        _ => "full",
    };
    let full_max_group = match env::var("WHUSE_STAGE2_FULL_MAX_GROUP")
        .unwrap_or_else(|_| String::from("all"))
        .as_str()
    {
        "time-test" => "time-test",
        "basic" => "basic",
        "busybox" => "busybox",
        "iozone" => "iozone",
        "libctest" => "libctest",
        "libc-bench" => "libc-bench",
        "lmbench" => "lmbench",
        "lua" => "lua",
        "unixbench" => "unixbench",
        "netperf" => "netperf",
        "iperf" => "iperf",
        "ltp" => "ltp",
        "cyclic" => "cyclic",
        _ => "all",
    };
    let default_iozone_profile = if target_arch == "riscv64" {
        "full"
    } else {
        "smoke"
    };
    let iozone_profile = match env::var("WHUSE_STAGE2_IOZONE_PROFILE")
        .unwrap_or_else(|_| default_iozone_profile.to_string())
        .as_str()
    {
        "full" => "full",
        _ => "smoke",
    };
    let iozone_full_scope = match env::var("WHUSE_STAGE2_IOZONE_FULL_SCOPE")
        .unwrap_or_else(|_| String::from("full"))
        .as_str()
    {
        "probe" => "probe",
        _ => "full",
    };

    println!("cargo:rustc-env=WHUSE_STAGE2_TIMEOUT_PROFILE_DEFAULT={timeout_profile}");
    println!("cargo:rustc-env=WHUSE_STAGE2_REAL_PHASE_DEFAULT={real_phase}");
    println!("cargo:rustc-env=WHUSE_STAGE2_REAL_GATE_LIBCTEST_SCOPE_DEFAULT={gate_libctest_scope}");
    println!("cargo:rustc-env=WHUSE_STAGE2_REAL_FULL_MAX_GROUP_DEFAULT={full_max_group}");
    println!("cargo:rustc-env=WHUSE_STAGE2_IOZONE_PROFILE_DEFAULT={iozone_profile}");
    println!("cargo:rustc-env=WHUSE_STAGE2_IOZONE_FULL_SCOPE_DEFAULT={iozone_full_scope}");
}
