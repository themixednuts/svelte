use std::fs;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};

#[path = "../repo/mod.rs"]
mod repo;

#[derive(Debug, Clone)]
pub struct FixtureCase {
    pub name: String,
    pub path: Utf8PathBuf,
}

impl FixtureCase {
    pub fn read_required_text(&self, relative_path: &str) -> io::Result<String> {
        fs::read_to_string(self.path.join(relative_path))
    }
}

pub fn load_suite_cases(suite_name: &str) -> io::Result<Vec<FixtureCase>> {
    let repo_root = repo::detect_repo_root()?;
    discover_suite_cases(&repo_root, suite_name)
}

fn discover_suite_cases(repo_root: &Utf8Path, suite_name: &str) -> io::Result<Vec<FixtureCase>> {
    let suite_samples = repo_root
        .join("packages")
        .join("svelte")
        .join("tests")
        .join(suite_name)
        .join("samples");

    let mut cases = Vec::new();
    for entry in fs::read_dir(&suite_samples)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
            continue;
        };

        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }

        cases.push(FixtureCase { name, path });
    }

    cases.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(cases)
}
