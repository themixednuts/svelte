use std::fs;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};

use crate::CompilerSuite;

const IGNORED_CHILDREN: [&str; 2] = ["_output", "_actual.json"];

#[derive(Debug, Clone)]
pub struct FixtureCase {
    pub name: String,
    pub path: Utf8PathBuf,
}

impl FixtureCase {
    pub fn read_required_text(&self, relative_path: &str) -> io::Result<String> {
        let path = self.path.join(relative_path);
        fs::read_to_string(path)
    }

    pub fn read_optional_text(&self, relative_path: &str) -> io::Result<Option<String>> {
        let path = self.path.join(relative_path);
        if path.exists() {
            return fs::read_to_string(path).map(Some);
        }
        Ok(None)
    }

    #[must_use]
    pub fn has_file(&self, relative_path: &str) -> bool {
        self.path.join(relative_path).exists()
    }
}

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

pub fn discover_suite_cases(
    repo_root: &Utf8Path,
    suite: CompilerSuite,
) -> io::Result<Vec<FixtureCase>> {
    discover_suite_cases_by_name(repo_root, suite.as_str())
}

pub fn discover_suite_cases_by_name(
    repo_root: &Utf8Path,
    suite_name: &str,
) -> io::Result<Vec<FixtureCase>> {
    let suite_samples = repo_root
        .join("packages")
        .join("svelte")
        .join("tests")
        .join(suite_name)
        .join("samples");

    let mut cases = Vec::new();

    for entry in fs::read_dir(&suite_samples)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        if !file_type.is_dir() {
            continue;
        }

        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
            continue;
        };

        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }

        if has_only_ignored_children(&path)? {
            continue;
        }

        cases.push(FixtureCase { name, path });
    }

    cases.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(cases)
}

fn has_only_ignored_children(path: &Utf8Path) -> io::Result<bool> {
    let mut found_any = false;

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        found_any = true;
        let child = entry.file_name().to_string_lossy().to_string();
        if !IGNORED_CHILDREN.contains(&child.as_str()) {
            return Ok(false);
        }
    }

    Ok(found_any)
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
