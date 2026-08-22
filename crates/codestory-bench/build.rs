use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let rustc = env::var_os("RUSTC").expect("Cargo-selected RUSTC");
    let output = Command::new(&rustc)
        .arg("-vV")
        .output()
        .expect("run Cargo-selected rustc -vV");
    assert!(
        output.status.success(),
        "Cargo-selected rustc -vV failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rustc_vv = String::from_utf8(output.stdout).expect("rustc -vV is UTF-8");
    assert!(!rustc_vv.trim().is_empty(), "rustc -vV is empty");
    let profile = env::var("PROFILE").expect("Cargo PROFILE");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    fs::write(out_dir.join("codestory-proof-rustc-vv.txt"), rustc_vv)
        .expect("write embedded rustc identity");
    fs::write(out_dir.join("codestory-proof-build-profile.txt"), profile)
        .expect("write embedded build profile");
}
