use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("user-init lives under crates/");
    let rootfs_dir = repo_root.join("tools").join("rootfs").join("common");
    println!("cargo:rerun-if-changed={}", rootfs_dir.display());

    let mut entries = Vec::new();
    collect_entries(&rootfs_dir, Path::new(""), &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut generated = String::from("pub static ROOTFS_ENTRIES: &[(&str, &[u8])] = &[\n");
    for (path, data) in entries {
        generated.push_str("    (");
        generated.push('"');
        generated.push_str(&path);
        generated.push_str("\", &[");
        for (index, byte) in data.iter().enumerate() {
            if index != 0 {
                generated.push_str(", ");
            }
            generated.push_str(&byte.to_string());
        }
        generated.push_str("]),\n");
    }
    generated.push_str("];\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("generated_rootfs.rs"), generated).expect("write generated rootfs");
}

fn collect_entries(root: &Path, relative: &Path, entries: &mut Vec<(String, Vec<u8>)>) {
    let dir = root.join(relative);
    for entry in fs::read_dir(&dir).expect("read rootfs dir") {
        let entry = entry.expect("rootfs entry");
        let file_type = entry.file_type().expect("rootfs file type");
        let rel_path = relative.join(entry.file_name());
        if file_type.is_dir() {
            collect_entries(root, &rel_path, entries);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let path = format!("/{}", rel_path.to_string_lossy().replace('\\', "/"));
        let data = fs::read(entry.path()).expect("read rootfs file");
        entries.push((path, data));
    }
}
