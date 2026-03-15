use std::fs;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use svelte_compiler::{CompileOptions, CssOutputMode, GenerateTarget, compile, compile_module};

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[test]
fn rust_compiler_matches_recorded_js_output_examples() {
    let fixtures_root = Utf8Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("js-output");

    let mut case_dirs = read_case_dirs(&fixtures_root).expect("read js output example cases");
    assert!(
        !case_dirs.is_empty(),
        "expected at least one js output example"
    );

    for case_dir in case_dirs.drain(..) {
        let snapshot = load_snapshot(&case_dir).expect("load js output snapshot");
        assert_eq!(
            snapshot.metadata.schema_version, SNAPSHOT_SCHEMA_VERSION,
            "snapshot schema version mismatch for {case_dir}"
        );

        let source_path = case_dir.join(snapshot.case.kind.source_file_name());
        let source = fs::read_to_string(&source_path).expect("read source fixture");
        let actual = compile_case(&source, &snapshot.case);

        assert_eq!(
            actual,
            snapshot.expected,
            "Rust compiler output drifted from recorded JS output for {}",
            case_dir.file_name().expect("case directory name"),
        );
    }
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

fn load_snapshot(case_dir: &Utf8Path) -> Result<JsOutputSnapshot, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(case_dir.join("expected.json"))?;
    Ok(serde_json::from_str(&contents)?)
}

fn compile_case(source: &str, case: &SnapshotCase) -> ComparableCompileOutput {
    match case.kind {
        SnapshotKind::Component => {
            let result =
                compile(source, case.options.to_rust_compile_options()).expect("compile component");
            ComparableCompileOutput::from_compile_result(result)
        }
        SnapshotKind::Module => {
            let result = compile_module(source, case.options.to_rust_compile_options())
                .expect("compile module");
            ComparableCompileOutput::from_compile_result(result)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotCase {
    kind: SnapshotKind,
    options: SnapshotCompileOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SnapshotCompileOptions {
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    generate: Option<String>,
    #[serde(default)]
    css: Option<String>,
    #[serde(default)]
    dev: bool,
    #[serde(default)]
    preserve_whitespace: bool,
    #[serde(default)]
    disclose_version: Option<bool>,
    #[serde(default)]
    runes: Option<bool>,
}

impl SnapshotCompileOptions {
    fn to_rust_compile_options(&self) -> CompileOptions {
        CompileOptions {
            filename: self.filename.as_ref().map(Utf8PathBuf::from),
            generate: match self.generate.as_deref() {
                Some("server") => GenerateTarget::Server,
                Some("none") => GenerateTarget::None,
                _ => GenerateTarget::Client,
            },
            css: match self.css.as_deref() {
                Some("injected") => CssOutputMode::Injected,
                _ => CssOutputMode::External,
            },
            dev: self.dev,
            preserve_whitespace: self.preserve_whitespace,
            disclose_version: self.disclose_version.unwrap_or(true),
            runes: self.runes,
            ..CompileOptions::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotMetadata {
    schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsOutputSnapshot {
    metadata: SnapshotMetadata,
    case: SnapshotCase,
    expected: ComparableCompileOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableCompileOutput {
    js: ComparableOutputArtifact,
    #[serde(default)]
    css: Option<ComparableCssOutputArtifact>,
    warnings: Box<[ComparableWarning]>,
    metadata: ComparableCompileMetadata,
}

impl ComparableCompileOutput {
    fn from_compile_result(result: svelte_compiler::CompileResult) -> Self {
        Self {
            js: ComparableOutputArtifact {
                code: normalize_line_endings(&result.js.code),
            },
            css: result.css.map(|css| ComparableCssOutputArtifact {
                code: normalize_line_endings(&css.code),
                has_global: css.has_global,
            }),
            warnings: result
                .warnings
                .iter()
                .map(|warning| ComparableWarning {
                    code: warning.code.to_string(),
                    message: warning.message.to_string(),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            metadata: ComparableCompileMetadata {
                runes: result.metadata.runes,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableOutputArtifact {
    code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableCssOutputArtifact {
    code: String,
    #[serde(default)]
    has_global: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableWarning {
    code: String,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComparableCompileMetadata {
    runes: bool,
}

fn normalize_line_endings(input: &str) -> String {
    let normalized = input.replace("\r\n", "\n");
    normalized
        .strip_suffix('\n')
        .unwrap_or(&normalized)
        .to_owned()
}
