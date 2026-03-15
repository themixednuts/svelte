use std::io;

use camino::{Utf8Path, Utf8PathBuf};

pub fn detect_repo_root() -> io::Result<Utf8PathBuf> {
    if let Ok(path) = std::env::var("SVELTE_REPO_ROOT") {
        let root = Utf8PathBuf::from(path);
        return ensure_repo_root(&root);
    }

    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in manifest_dir.ancestors() {
        if let Some(root) = resolve_repo_root(candidate) {
            return Ok(root);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "unable to detect repository root containing packages/svelte/tests",
    ))
}

fn ensure_repo_root(root: &Utf8Path) -> io::Result<Utf8PathBuf> {
    if let Some(resolved) = resolve_repo_root(root) {
        return Ok(resolved);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "provided SVELTE_REPO_ROOT does not contain packages/svelte/tests or svelte/packages/svelte/tests: {root}"
        ),
    ))
}

fn resolve_repo_root(candidate: &Utf8Path) -> Option<Utf8PathBuf> {
    if candidate
        .join("packages")
        .join("svelte")
        .join("tests")
        .is_dir()
    {
        return Some(candidate.to_path_buf());
    }

    let nested = candidate.join("svelte");
    if nested.join("packages").join("svelte").join("tests").is_dir() {
        return Some(nested);
    }

    None
}
