use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use svelte_kit::{BuildManifestChunk, assets_base, resolve_manifest_symlink, validate_config};

fn chunk(file: &str) -> BuildManifestChunk {
    BuildManifestChunk {
        file: file.to_string(),
        imports: Vec::new(),
        dynamic_imports: Vec::new(),
        css: Vec::new(),
        assets: Vec::new(),
    }
}

#[test]
fn assets_base_prefers_assets_then_base_then_relative_dot() {
    let with_assets = validate_config(
        &serde_json::json!({ "kit": { "paths": { "assets": "https://cdn.example.com" } } }),
        &Utf8PathBuf::from("E:/Projects/svelte"),
    )
    .expect("config should validate");
    assert_eq!(assets_base(&with_assets.kit), "https://cdn.example.com/");

    let with_base = validate_config(
        &serde_json::json!({ "kit": { "paths": { "base": "/app" } } }),
        &Utf8PathBuf::from("E:/Projects/svelte"),
    )
    .expect("config should validate");
    assert_eq!(assets_base(&with_base.kit), "/app/");

    let defaulted = validate_config(
        &serde_json::json!({}),
        &Utf8PathBuf::from("E:/Projects/svelte"),
    )
    .expect("config should validate");
    assert_eq!(assets_base(&defaulted.kit), "./");
}

#[test]
fn resolve_manifest_symlink_returns_direct_match() {
    let manifest = BTreeMap::from([("entry.js".to_string(), chunk("chunks/entry.js"))]);
    let resolved = resolve_manifest_symlink(&manifest, "entry.js").expect("entry should resolve");
    assert_eq!(resolved.0, "entry.js");
    assert_eq!(resolved.1.file, "chunks/entry.js");
}

#[test]
fn resolve_manifest_symlink_errors_for_missing_file() {
    let manifest = BTreeMap::<String, BuildManifestChunk>::new();
    let error =
        resolve_manifest_symlink(&manifest, "missing.js").expect_err("missing file should error");
    assert!(error.to_string().contains("Could not find file"));
}
