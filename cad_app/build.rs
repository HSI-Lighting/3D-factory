//! Stamp the binary with the commit it was built from.
//!
//! Two rounds of this project's debugging were lost to a repair being run against a STALE BUILD:
//! the older binary reported "no shallow cuts found" — correct for its own logic, and
//! indistinguishable from success — while everyone assumed the new code was running. A session
//! dump that names its build makes that unanswerable-wrong instead of a guess.
//!
//! Best-effort. No git, no repo, or a stripped checkout just yields "unknown"; a build must never
//! fail over a version string.

use std::process::Command;

fn main() {
    let short = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    // A build with uncommitted edits is NOT the commit it names, and saying so matters: half the
    // confusion this exists to prevent came from running a binary that was one edit ahead.
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty());

    println!("cargo:rustc-env=SIMLUX_BUILD={short}{}", if dirty { "+dirty" } else { "" });
    // Re-run when HEAD moves, so the stamp cannot go stale on an incremental build.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
}
