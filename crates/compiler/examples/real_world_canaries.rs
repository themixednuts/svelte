use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[path = "support/batch_compile.rs"]
mod batch_compile;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use svelte_compiler::{CompileOptions, GenerateTarget, VERSION, compile, compile_module};

const REPORT_SCHEMA_VERSION: u32 = 1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = manifest_dir
        .join("examples")
        .join("support")
        .join("real_world_canaries.json");
    let manifest = load_manifest(&manifest_path)?;
    let workspace_root = manifest_dir
        .parent()
        .and_then(camino::Utf8Path::parent)
        .ok_or_else(|| io::Error::other("failed to detect workspace root"))?;
    let checkout_root = workspace_root.join("tmp").join("real-world-canaries");
    let filter = std::env::var("SVELTE_REAL_WORLD_FILTER").ok();
    let override_max_files = std::env::var("SVELTE_REAL_WORLD_MAX_FILES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());

    fs::create_dir_all(&checkout_root)?;

    let mut repo_reports = Vec::new();
    for repo in manifest.repositories {
        if !matches_filter(&filter, &repo.name) {
            continue;
        }

        let started_at = Instant::now();
        let checkout_dir = checkout_root.join(&repo.name);
        checkout_repo(checkout_dir.as_std_path(), &repo)?;
        let file_limit = override_max_files.unwrap_or(repo.max_files);
        let files = discover_candidate_files(
            checkout_dir.as_std_path(),
            repo.subdir.as_deref(),
            file_limit,
        )?;
        let file_reports = compile_candidates(checkout_dir.as_std_path(), &files)?;

        repo_reports.push(RealWorldRepoReport {
            name: repo.name,
            url: repo.url,
            commit: repo.commit,
            subdir: repo.subdir,
            scanned_files: files.len(),
            duration_ms: started_at.elapsed().as_millis(),
            files: file_reports.into_boxed_slice(),
        });
    }

    let report = RealWorldCanaryReport {
        metadata: RealWorldCanaryMetadata {
            schema_version: REPORT_SCHEMA_VERSION,
            generated_at_unix_ms: current_timestamp(),
            rust_crate_version: env!("CARGO_PKG_VERSION").to_owned(),
            rust_svelte_version: VERSION.to_owned(),
        },
        repositories: repo_reports.into_boxed_slice(),
    };

    let report_path = checkout_root.join("report.json");
    fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;
    println!("wrote {}", report_path);
    Ok(())
}

fn load_manifest(path: &camino::Utf8Path) -> Result<RealWorldManifest, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&contents)?)
}

fn matches_filter(filter: &Option<String>, name: &str) -> bool {
    filter
        .as_deref()
        .map(|pattern| name.contains(pattern))
        .unwrap_or(true)
}

fn current_timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_millis()
}

fn checkout_repo(checkout_dir: &Path, repo: &RealWorldRepository) -> io::Result<()> {
    if !checkout_dir.join(".git").is_dir() {
        run(Command::new("git")
            .args(["clone", "--filter=blob:none", "--no-checkout", &repo.url])
            .arg(checkout_dir))?;
    }

    run(Command::new("git").arg("-C").arg(checkout_dir).args([
        "fetch",
        "--depth",
        "1",
        "origin",
        &repo.commit,
    ]))?;
    run(Command::new("git")
        .arg("-C")
        .arg(checkout_dir)
        .args(["checkout", "--force", &repo.commit]))
}

fn discover_candidate_files(
    checkout_dir: &Path,
    subdir: Option<&str>,
    max_files: usize,
) -> io::Result<Vec<PathBuf>> {
    let root = subdir
        .map(|path| checkout_dir.join(path))
        .unwrap_or_else(|| checkout_dir.to_path_buf());
    let mut files = Vec::new();
    visit_candidate_files(&root, &mut files, max_files)?;
    files.sort();
    Ok(files)
}

fn visit_candidate_files(
    root: &Path,
    files: &mut Vec<PathBuf>,
    max_files: usize,
) -> io::Result<()> {
    if files.len() >= max_files {
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if should_skip_dir(&path) {
                continue;
            }
            visit_candidate_files(&path, files, max_files)?;
            if files.len() >= max_files {
                return Ok(());
            }
            continue;
        }

        if file_type.is_file() && is_candidate_file(&path) {
            files.push(path);
            if files.len() >= max_files {
                return Ok(());
            }
        }
    }

    Ok(())
}

fn should_skip_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(
            ".git"
                | "node_modules"
                | "dist"
                | "build"
                | ".svelte-kit"
                | ".vercel"
                | ".output"
                | "coverage"
                | "target"
        )
    )
}

fn is_candidate_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.ends_with(".svelte") || name.ends_with(".svelte.js") || name.ends_with(".svelte.ts")
}

fn compile_candidates(
    checkout_dir: &Path,
    files: &[PathBuf],
) -> io::Result<Vec<RealWorldFileReport>> {
    batch_compile::map_in_parallel(files.to_vec(), |file| {
        compile_candidate(checkout_dir, &file)
    })
    .into_iter()
    .collect()
}

fn compile_candidate(checkout_dir: &Path, file: &Path) -> io::Result<RealWorldFileReport> {
    let relative = file
        .strip_prefix(checkout_dir)
        .map_err(|_| io::Error::other("failed to strip checkout prefix"))?;
    let source = fs::read_to_string(file)?;
    let relative_utf8 = Utf8PathBuf::from_path_buf(relative.to_path_buf())
        .map_err(|_| io::Error::other("non-utf8 relative file path"))?;
    let file_kind = classify_file_kind(&relative_utf8);
    let targets = targets_for_kind(file_kind);
    let mut runs = Vec::with_capacity(targets.len());

    for &target in targets {
        let started_at = Instant::now();
        let outcome = run_compile_for_target(&source, &relative_utf8, file_kind, target);
        runs.push(RealWorldCompileRun {
            target,
            duration_ms: started_at.elapsed().as_millis(),
            outcome,
        });
    }

    Ok(RealWorldFileReport {
        path: relative_utf8,
        kind: file_kind,
        runs: runs.into_boxed_slice(),
    })
}

fn classify_file_kind(path: &camino::Utf8Path) -> CanaryFileKind {
    let text = path.as_str();
    if text.ends_with(".svelte.js") || text.ends_with(".svelte.ts") {
        CanaryFileKind::Module
    } else {
        CanaryFileKind::Component
    }
}

fn targets_for_kind(kind: CanaryFileKind) -> &'static [CanaryTarget] {
    match kind {
        CanaryFileKind::Component | CanaryFileKind::Module => {
            &[CanaryTarget::Client, CanaryTarget::Server]
        }
    }
}

fn run_compile_for_target(
    source: &str,
    relative_path: &camino::Utf8Path,
    kind: CanaryFileKind,
    target: CanaryTarget,
) -> CanaryCompileOutcome {
    let options = CompileOptions {
        filename: Some(relative_path.to_path_buf()),
        generate: target.into(),
        ..CompileOptions::default()
    };

    let result = match kind {
        CanaryFileKind::Component => compile(source, options),
        CanaryFileKind::Module => compile_module(source, options),
    };

    match result {
        Ok(result) => CanaryCompileOutcome::Success {
            warnings: result.warnings.len(),
            runes: result.metadata.runes,
        },
        Err(error) => CanaryCompileOutcome::Failure {
            message: error.to_string(),
        },
    }
}

fn run(command: &mut Command) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "command exited with status {status}"
    )))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RealWorldManifest {
    repositories: Box<[RealWorldRepository]>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RealWorldRepository {
    name: String,
    url: String,
    commit: String,
    #[serde(default)]
    subdir: Option<String>,
    max_files: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RealWorldCanaryReport {
    metadata: RealWorldCanaryMetadata,
    repositories: Box<[RealWorldRepoReport]>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RealWorldCanaryMetadata {
    schema_version: u32,
    generated_at_unix_ms: u128,
    rust_crate_version: String,
    rust_svelte_version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RealWorldRepoReport {
    name: String,
    url: String,
    commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    subdir: Option<String>,
    scanned_files: usize,
    duration_ms: u128,
    files: Box<[RealWorldFileReport]>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RealWorldFileReport {
    path: Utf8PathBuf,
    kind: CanaryFileKind,
    runs: Box<[RealWorldCompileRun]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CanaryFileKind {
    Component,
    Module,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RealWorldCompileRun {
    target: CanaryTarget,
    duration_ms: u128,
    #[serde(flatten)]
    outcome: CanaryCompileOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CanaryTarget {
    Client,
    Server,
}

impl From<CanaryTarget> for GenerateTarget {
    fn from(value: CanaryTarget) -> Self {
        match value {
            CanaryTarget::Client => GenerateTarget::Client,
            CanaryTarget::Server => GenerateTarget::Server,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum CanaryCompileOutcome {
    Success { warnings: usize, runes: bool },
    Failure { message: String },
}
