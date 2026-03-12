use std::io;

use camino::{Utf8Path, Utf8PathBuf};

pub fn detect_repo_root() -> io::Result<Utf8PathBuf> {
    if let Ok(path) = std::env::var("SVELTE_REPO_ROOT") {
        let root = Utf8PathBuf::from(path);
        ensure_repo_root(&root)?;
        return Ok(root);
    }

    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in manifest_dir.ancestors() {
        if has_js_fixture_root(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "unable to detect repository root containing packages/svelte/tests",
    ))
}

fn ensure_repo_root(root: &Utf8Path) -> io::Result<()> {
    if has_js_fixture_root(root) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("provided SVELTE_REPO_ROOT does not contain packages/svelte/tests: {root}"),
    ))
}

fn has_js_fixture_root(candidate: &Utf8Path) -> bool {
    candidate
        .join("packages")
        .join("svelte")
        .join("tests")
        .is_dir()
}
