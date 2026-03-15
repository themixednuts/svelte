use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{
    PrerenderedResolution, build_preview_plan, preview_paths, preview_protocol,
    preview_root_redirect, resolve_prerendered_request,
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
        .join(format!("svelte-kit-preview-{label}-{unique}"));
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
fn computes_preview_protocol_and_paths() {
    assert_eq!(preview_protocol(true), "https");
    assert_eq!(preview_protocol(false), "http");

    let mapped = preview_paths("/base", "https://cdn.example.com");
    assert_eq!(mapped.base, "/base");
    assert_eq!(mapped.assets, "/_svelte_kit_assets");

    let local = preview_paths("/base", "");
    assert_eq!(local.assets, "/base");

    let plan = build_preview_plan(
        &Utf8PathBuf::from(".svelte-kit"),
        "/base",
        "https://cdn.example.com",
        true,
    );
    assert_eq!(plan.protocol, "https");
    assert_eq!(plan.assets, "/_svelte_kit_assets");
    assert_eq!(
        plan.server_dir,
        Utf8PathBuf::from(".svelte-kit/output/server")
    );
    assert_eq!(
        plan.prerendered_dependencies_dir,
        Utf8PathBuf::from(".svelte-kit/output/prerendered/dependencies")
    );
    assert_eq!(plan.expected_server_files.len(), 2);
}

#[test]
fn redirects_base_root_to_slash_variant() {
    assert_eq!(
        preview_root_redirect("/base", "/base", "?q=1"),
        Some("/base/?q=1".to_string())
    );
    assert_eq!(preview_root_redirect("", "/", ""), None);
}

#[test]
fn resolves_prerendered_files_and_redirects() {
    let cwd = temp_dir("resolve");
    let prerendered = cwd.join("output").join("prerendered");
    write_file(
        &prerendered.join("pages").join("about.html"),
        "<h1>about</h1>",
    );
    write_file(
        &prerendered.join("pages").join("docs").join("index.html"),
        "<h1>docs</h1>",
    );
    write_file(
        &prerendered
            .join("data")
            .join("_app")
            .join("remote")
            .join("feed"),
        "{}",
    );

    match resolve_prerendered_request(&cwd, "_app", "/about", "") {
        PrerenderedResolution::File(file) => {
            assert!(file.file.ends_with("about.html"));
            assert_eq!(file.content_type_path, "/about");
        }
        other => panic!("expected file, got {other:?}"),
    }

    assert_eq!(
        resolve_prerendered_request(&cwd, "_app", "/docs", "?lang=en"),
        PrerenderedResolution::Redirect("/docs/?lang=en".to_string())
    );
    assert_eq!(
        resolve_prerendered_request(&cwd, "_app", "/about/", ""),
        PrerenderedResolution::Redirect("/about".to_string())
    );

    match resolve_prerendered_request(&cwd, "_app", "/_app/remote/feed", "") {
        PrerenderedResolution::File(file) => {
            assert!(file.file.ends_with("_app/remote/feed"));
        }
        other => panic!("expected remote file, got {other:?}"),
    }

    assert_eq!(
        resolve_prerendered_request(&cwd, "_app", "/missing", ""),
        PrerenderedResolution::Missing
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
