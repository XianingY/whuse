fn main() {
    println!("cargo:rerun-if-changed=linker.ld");
    println!("cargo:rustc-link-arg=-Tplatform/loongarch64-bootrom/linker.ld");
}
