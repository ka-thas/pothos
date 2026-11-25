// On macOS, compiles the Swift helpers (AirDrop, publish) and embeds them in the binary.
// Elsewhere, embeds empty files and those features report as unavailable.
use std::{env, path::PathBuf, process::Command};

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    for (src, name) in [("airdrop.swift", "airdrop-helper"), ("publish.swift", "publish-helper")] {
        println!("cargo:rerun-if-changed={src}");
        let out = out_dir.join(name);
        if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
            let ok = Command::new("swiftc")
                .args(["-O", src, "-o"])
                .arg(&out)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                panic!("swiftc failed on {src}. Install Xcode Command Line Tools: xcode-select --install");
            }
        } else {
            std::fs::write(&out, b"").unwrap();
        }
    }
}
