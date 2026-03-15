use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{
    compress_directory, create_instrumentation_facade, has_server_instrumentation_file,
    instrument_entrypoint,
};

fn repo_root() -> Utf8PathBuf {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|candidate| candidate.join("kit").join("packages").join("kit").is_dir())
        .expect("workspace root")
        .to_path_buf()
}

fn temp_dir(label: &str) -> Utf8PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let dir = repo_root()
        .join("tmp")
        .join(format!("svelte-kit-adapt-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

#[test]
fn creates_instrumentation_facade_with_aliases_for_reserved_exports() {
    let facade = create_instrumentation_facade(
        "instrumentation.js",
        "start.js",
        &[
            "default".to_string(),
            "class".to_string(),
            "answer".to_string(),
        ],
    );

    assert!(facade.contains("import './instrumentation.js';"));
    assert!(
        facade.contains("const { default: _0, class: _1, answer } = await import('./start.js');")
    );
    assert!(facade.contains("export { _0 as default, _1 as class, answer };"));
}

#[test]
fn instruments_entrypoint_and_preserves_sourcemap() {
    let cwd = temp_dir("instrument");
    let entrypoint = cwd.join("server").join("index.js");
    let instrumentation = cwd.join("instrumentation.server.js");
    let start = cwd.join("server").join("boot.js");

    write_file(&entrypoint, "export const answer = 42;\n");
    write_file(&Utf8PathBuf::from(format!("{entrypoint}.map")), "{}");
    write_file(&instrumentation, "console.log('instrument');\n");

    instrument_entrypoint(
        &entrypoint,
        &instrumentation,
        Some(&start),
        &["default".to_string(), "class".to_string()],
    )
    .expect("instrument entrypoint");

    let rewritten = fs::read_to_string(&entrypoint).expect("read rewritten entrypoint");
    let boot = fs::read_to_string(&start).expect("read moved start");
    let boot_map = Utf8PathBuf::from(format!("{start}.map"));

    assert!(rewritten.contains("import './../instrumentation.server.js';"));
    assert!(rewritten.contains("await import('./boot.js');"));
    assert!(rewritten.contains("export { _0 as default, _1 as class };"));
    assert_eq!(boot, "export const answer = 42;\n");
    assert!(boot_map.is_file());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn detects_server_instrumentation_file_in_output_tree() {
    let cwd = temp_dir("has-instrumentation");
    let out_dir = cwd.join(".svelte-kit");
    let instrumentation = out_dir
        .join("output")
        .join("server")
        .join("instrumentation.server.js");

    assert!(!has_server_instrumentation_file(&out_dir));
    write_file(&instrumentation, "console.log('instrument');\n");
    assert!(has_server_instrumentation_file(&out_dir));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn compresses_supported_files_with_gzip_and_brotli() {
    let cwd = temp_dir("compress");
    let html = cwd.join("index.html");
    let js = cwd.join("assets").join("app.js");
    let png = cwd.join("image.png");

    write_file(&html, "<html><body>Hello</body></html>");
    write_file(&js, "console.log('hi');");
    write_file(&png, "not compressed");

    compress_directory(&cwd).expect("compress directory");

    assert!(Utf8PathBuf::from(format!("{html}.gz")).is_file());
    assert!(Utf8PathBuf::from(format!("{html}.br")).is_file());
    assert!(Utf8PathBuf::from(format!("{js}.gz")).is_file());
    assert!(Utf8PathBuf::from(format!("{js}.br")).is_file());
    assert!(!Utf8PathBuf::from(format!("{png}.gz")).exists());
    assert!(!Utf8PathBuf::from(format!("{png}.br")).exists());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
