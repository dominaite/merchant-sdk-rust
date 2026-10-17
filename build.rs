//! Records the compiler version for the User-Agent, which reads
//! `dominaite-rust/<version> (rust <rustc version>)`. No dependency: it asks the
//! same `rustc` Cargo builds with, and falls back to `unknown`.

use std::env;
use std::process::Command;

fn main() {
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        // "rustc 1.85.0 (4d91de4e4 2025-02-17)" -> "1.85.0"
        .and_then(|text| text.split_whitespace().nth(1).map(str::to_string))
        .filter(|version| {
            !version.is_empty()
                && version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=DOMINAITE_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");
}
