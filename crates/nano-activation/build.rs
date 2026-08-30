use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let lock = root.join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git/HEAD").display()
    );
    let commit = git(root, &["rev-parse", "HEAD"]);
    let dirty = !git(root, &["status", "--porcelain", "--untracked-files=no"]).is_empty();
    let lock_sha = hex(&Sha256::digest(
        fs::read(lock).expect("read workspace Cargo.lock"),
    ));
    println!("cargo:rustc-env=NANO_SOURCE_COMMIT_SHA={commit}");
    println!("cargo:rustc-env=NANO_CARGO_LOCK_SHA256={lock_sha}");
    println!("cargo:rustc-env=NANO_SOURCE_DIRTY={dirty}");
}

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git identity command failed");
    String::from_utf8(output.stdout)
        .expect("git output utf8")
        .trim()
        .to_owned()
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(H[(b >> 4) as usize] as char);
        out.push(H[(b & 15) as usize] as char);
    }
    out
}
