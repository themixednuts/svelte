use std::fs;
use std::io;
use std::process::{Command, Stdio};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use svelte_compiler::VERSION;

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cases_dir = manifest_dir
        .join("tests")
        .join("fixtures")
        .join("js-output");
    let repo_root = detect_js_repo_root(&manifest_dir)?;
    ensure_js_workspace_ready(&repo_root)?;
    let helper_script = manifest_dir
        .join("examples")
        .join("support")
        .join("compile_with_js.mjs");

    for case_dir in read_case_dirs(&cases_dir)? {
        let case_name = case_dir
            .file_name()
            .ok_or_else(|| io::Error::other("case directory missing name"))?;
        let snapshot_case = load_snapshot_case(&case_dir)?;
        let source = fs::read_to_string(case_dir.join(snapshot_case.kind.source_file_name()))?;
        let request = JsCompilerRequest {
            repo_root: repo_root.clone(),
            kind: snapshot_case.kind,
            source,
            options: snapshot_case.options.clone(),
        };

        let expected = run_js_compiler(&helper_script, request)?;
        let snapshot = JsOutputSnapshot {
            metadata: SnapshotMetadata {
                schema_version: SNAPSHOT_SCHEMA_VERSION,
                generated_at: expected.metadata.generated_at,
                rust_crate_version: env!("CARGO_PKG_VERSION").to_owned(),
                rust_svelte_version: VERSION.to_owned(),
                js_package_version: expected.metadata.js_package_version,
                node_version: expected.metadata.node_version,
                case_name: case_name.to_owned(),
            },
            case: snapshot_case,
            expected: expected.output,
        };

        let output = serde_json::to_string_pretty(&snapshot)?;
        fs::write(case_dir.join("expected.json"), output)?;
        println!("updated {}", case_dir);
    }

    Ok(())
}

fn read_case_dirs(root: &Utf8Path) -> io::Result<Vec<Utf8PathBuf>> {
    let mut cases = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let path = Utf8PathBuf::from_path_buf(entry.path())
                .map_err(|_| io::Error::other("non-utf8 case path"))?;
            cases.push(path);
        }
    }
    cases.sort();
    Ok(cases)
}

fn load_snapshot_case(case_dir: &Utf8Path) -> Result<SnapshotCase, Box<dyn std::error::Error>> {
    let config = fs::read_to_string(case_dir.join("case.json"))?;
    Ok(serde_json::from_str(&config)?)
}

fn detect_js_repo_root(manifest_dir: &Utf8Path) -> io::Result<Utf8PathBuf> {
    if let Ok(path) = std::env::var("SVELTE_REPO_ROOT") {
        let root = Utf8PathBuf::from(path);
        ensure_js_repo_root(&root)?;
        return Ok(root);
    }

    for candidate in manifest_dir.ancestors() {
        if has_js_fixture_root(candidate) {
            return Ok(candidate.to_path_buf());
        }
        let nested = candidate.join("svelte");
        if has_js_fixture_root(&nested) {
            return Ok(nested);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "unable to detect Svelte JS repository root containing packages/svelte/tests",
    ))
}

fn ensure_js_repo_root(root: &Utf8Path) -> io::Result<()> {
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

fn ensure_js_workspace_ready(repo_root: &Utf8Path) -> io::Result<()> {
    if repo_root.join("node_modules").is_dir() {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "missing JavaScript workspace dependencies under {repo_root}; run `pnpm install --frozen-lockfile` in that repository before regenerating JS snapshots"
        ),
    ))
}

fn run_js_compiler(
    helper_script: &Utf8Path,
    request: JsCompilerRequest,
) -> Result<JsCompilerResponse, Box<dyn std::error::Error>> {
    let request_json = serde_json::to_string(&request)?;
    let output = Command::new("node")
        .arg(helper_script.as_str())
        .arg(request_json)
        .stderr(Stdio::inherit())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("node exited with status {}", output.status)).into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SnapshotKind {
    Component,
    Module,
}

impl SnapshotKind {
    const fn source_file_name(self) -> &'static str {
        match self {
            Self::Component => "input.svelte",
            Self::Module => "input.js",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotCase {
    kind: SnapshotKind,
    options: SnapshotCompileOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SnapshotCompileOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    css: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "is_false")]
    dev: bool,
    #[serde(default)]
    #[serde(skip_serializing_if = "is_false")]
    preserve_whitespace: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    disclose_version: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runes: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotMetadata {
    schema_version: u32,
    generated_at: String,
    rust_crate_version: String,
    rust_svelte_version: String,
    js_package_version: String,
    node_version: String,
    case_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsOutputSnapshot {
    metadata: SnapshotMetadata,
    case: SnapshotCase,
    expected: ComparableCompileOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableCompileOutput {
    js: ComparableOutputArtifact,
    #[serde(default)]
    css: Option<ComparableCssOutputArtifact>,
    warnings: Box<[ComparableWarning]>,
    metadata: ComparableCompileMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableOutputArtifact {
    code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableCssOutputArtifact {
    code: String,
    #[serde(default)]
    has_global: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableWarning {
    code: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableCompileMetadata {
    runes: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JsCompilerRequest {
    repo_root: Utf8PathBuf,
    kind: SnapshotKind,
    source: String,
    options: SnapshotCompileOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsCompilerResponse {
    metadata: JsCompilerResponseMetadata,
    output: ComparableCompileOutput,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsCompilerResponseMetadata {
    generated_at: String,
    js_package_version: String,
    node_version: String,
}

const fn is_false(value: &bool) -> bool {
    !*value
}
