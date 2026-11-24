// On macOS, compiles the Swift AirDrop helper and embeds it in the binary.
// Elsewhere, embeds an empty file and AirDrop is reported as unavailable.
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=airdrop.swift");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("airdrop-helper");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let ok = Command::new("swiftc")
            .args(["-O", "airdrop.swift", "-o"])
            .arg(&out)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            panic!("swiftc failed. Install Xcode Command Line Tools: xcode-select --install");
        }
    } else {
        std::fs::write(&out, b"").unwrap();
    }
}
