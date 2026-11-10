fn main() {
    build_tag();
    build_profile();
    tauri_build::build()
}

/// `sparkbox_debug_profile`: cargo is building the debug profile. `debug_assertions` decides
/// whether this is the test build, with its fence and own identity, or the release build. It can
/// be set apart from the profile (CARGO_PROFILE_DEV_DEBUG_ASSERTIONS, RUSTFLAGS, a config file), so
/// live/exit.rs refuses to compile when the two disagree. That check sits in the crate because
/// only the crate sees the flag rustc really got: CARGO_CFG_DEBUG_ASSERTIONS here says what the
/// profile asked for, and misses RUSTFLAGS.
fn build_profile() {
    println!("cargo:rustc-check-cfg=cfg(sparkbox_debug_profile)");
    if std::env::var("PROFILE").is_ok_and(|p| p == "debug") {
        println!("cargo:rustc-cfg=sparkbox_debug_profile");
    }
}

/// SPARKBOX_BUILD_TAG: `git describe --tags --always --dirty` of the tree this was built from,
/// else "untagged". The controller lease and the evidence manifest name it, so a node and a
/// capture show which build acted.
///
/// The tree is the one cargo builds now (CARGO_MANIFEST_DIR read when the script runs, not when
/// it was compiled): two checkouts that share a target folder share this script's binary.
fn build_tag() {
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").map(std::path::PathBuf::from).unwrap_or_default();
    let root = manifest.join("..");
    let git = root.join(".git");
    let mut watched = vec![git.join("HEAD"), git.join("refs"), git.join("packed-refs")];
    // The branch HEAD points to moves with every commit.
    if let Some(r) = std::fs::read_to_string(git.join("HEAD")).ok().and_then(|h| h.strip_prefix("ref: ").map(|r| r.trim().to_string())) {
        watched.push(git.join(r));
    }
    for p in watched.iter().filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    let tag = std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || "-._+".contains(c)))
        .unwrap_or_else(|| "untagged".into());
    println!("cargo:rustc-env=SPARKBOX_BUILD_TAG={tag}");
}
