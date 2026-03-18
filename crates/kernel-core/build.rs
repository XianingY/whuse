use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_TIMEOUT_PROFILE");
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_REAL_PHASE");
    println!("cargo:rerun-if-env-changed=WHUSE_STAGE2_GATE_LIBCTEST_SCOPE");

    let timeout_profile = match env::var("WHUSE_STAGE2_TIMEOUT_PROFILE")
        .unwrap_or_else(|_| String::from("real"))
        .as_str()
    {
        "chain-fast" => "chain-fast",
        _ => "real",
    };
    let real_phase = match env::var("WHUSE_STAGE2_REAL_PHASE")
        .unwrap_or_else(|_| String::from("full"))
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

    println!("cargo:rustc-env=WHUSE_STAGE2_TIMEOUT_PROFILE_DEFAULT={timeout_profile}");
    println!("cargo:rustc-env=WHUSE_STAGE2_REAL_PHASE_DEFAULT={real_phase}");
    println!("cargo:rustc-env=WHUSE_STAGE2_REAL_GATE_LIBCTEST_SCOPE_DEFAULT={gate_libctest_scope}");
}
