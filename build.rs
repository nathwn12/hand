//! Build-time provenance stamp: the exact commit and profile this binary was
//! built from, compiled in so `hand --version` reports them from any path.
//!
//! `hand --version` prints these values, so a deployed `hand.exe` can be
//! compared against `git rev-parse HEAD` instead of guessed at from its mtime
//! or the directory someone installed it in. A `-dirty` marker means a *tracked*
//! file held uncommitted changes at build time. Untracked files are excluded on
//! purpose: another process's receipts or scratch would otherwise mark every
//! build dirty, which is a false provenance claim about code that never entered
//! the binary.
//!
//! The rerun set is explicit: a commit moves HEAD without touching any
//! working-tree mtime, so cargo's default would reuse a stale stamp. The loose
//! ref, the reflog and `packed-refs` cover every way git moves the commit; a
//! source tarball with no `.git` stamps `unknown` instead of failing the build.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        println!("cargo:rerun-if-changed=.git/{reference}");
    }
    println!("cargo:rerun-if-changed=.git/logs/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=crates");

    let sha = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=HAND_GIT_SHA={sha}");
    println!("cargo:rustc-env=HAND_GIT_DIRTY={}", u8::from(dirty));
    println!("cargo:rustc-env=HAND_BUILD_PROFILE={profile}");
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
