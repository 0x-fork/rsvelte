use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    napi_build::setup();
    embed_build_commit();
}

/// Embed the commit this cdylib was compiled from, so a staged binding can be
/// attributed by asking the binary instead of inspecting the tree around it.
fn embed_build_commit() {
    let (commit, dirty) = match git_describe() {
        Some(v) => v,
        // No git (published crate, tarball, vendored build) is not an error; it
        // is an honest "unknown" that consumers must treat as unattributable.
        None => (String::from("unknown"), false),
    };
    println!("cargo:rustc-env=RSVELTE_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=RSVELTE_BUILD_DIRTY={}", u8::from(dirty));
    for path in rerun_paths() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_describe() -> Option<(String, bool)> {
    let commit = git(&["rev-parse", "HEAD"])?;
    if commit.is_empty() {
        return None;
    }
    // Only crates/ can change what this library does, so a dirty docs tree must
    // not mark the build unattributable. `:/` anchors the pathspec at the
    // repository root: git resolves a bare path against the cwd, which here is
    // the crate directory, so `crates` would silently match nothing.
    let dirty = !git(&["status", "--porcelain", "--", ":/crates"])?.is_empty();
    Some((commit, dirty))
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
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

/// Files whose change means HEAD moved. `.git` is a *file* inside a worktree,
/// so resolve it rather than assuming a directory.
fn rerun_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Some(git_dir) = git(&["rev-parse", "--git-dir"]) else {
        return paths;
    };
    let git_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(git_dir);
    let head = git_dir.join("HEAD");
    if head.exists() {
        paths.push(head);
    }
    // A branch checkout moves the ref file; a detached HEAD moves HEAD itself.
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        let common = git(&["rev-parse", "--git-common-dir"])
            .map(|d| Path::new(env!("CARGO_MANIFEST_DIR")).join(d))
            .unwrap_or_else(|| git_dir.clone());
        let ref_file = common.join(&reference);
        if ref_file.exists() {
            paths.push(ref_file);
        }
        let packed = common.join("packed-refs");
        if packed.exists() {
            paths.push(packed);
        }
    }
    paths
}
