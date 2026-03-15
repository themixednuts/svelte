use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde_json::json;
use svelte_kit::{
    BuildData, BuilderPrerendered, BuilderServerMetadata, Hooks, KitManifest, ManifestNode,
    ManifestNodeKind, ManifestRoute, ValidatedConfig, adapt_project, invoke_adapter,
    validate_config,
};

fn config_with_adapter() -> ValidatedConfig {
    validate_config(
        &json!({
            "kit": {
                "adapter": {
                    "name": "adapter-static",
                    "adapt": "() => null"
                }
            }
        }),
        &Utf8PathBuf::from("E:/Projects/svelte"),
    )
    .expect("config should validate")
}

fn minimal_manifest() -> KitManifest {
    KitManifest {
        assets: Vec::new(),
        hooks: Hooks::default(),
        matchers: BTreeMap::new(),
        manifest_routes: vec![ManifestRoute {
            id: "/".to_string(),
            pattern: regex::Regex::new("^/$").expect("regex"),
            params: Vec::new(),
            page: None,
            endpoint: None,
        }],
        nodes: vec![ManifestNode {
            kind: ManifestNodeKind::Page,
            component: None,
            universal: None,
            server: None,
            parent_id: None,
            universal_page_options: None,
            server_page_options: None,
            page_options: None,
        }],
        routes: Vec::new(),
    }
}

#[test]
fn adapt_project_builds_facade_and_uses_adapter_name() {
    let config = config_with_adapter();
    let manifest = minimal_manifest();
    let server_manifest = BTreeMap::new();
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: Utf8PathBuf::from("E:/Projects/svelte/.svelte-kit"),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };

    let result = adapt_project(
        &config,
        &build_data,
        &BuilderServerMetadata::default(),
        &BuilderPrerendered::default(),
        &[],
        |builder| Ok(builder.routes().len()),
    )
    .expect("adapt should succeed");

    assert_eq!(result.adapter_name, "adapter-static");
    assert_eq!(result.output, 1);
}

#[test]
fn adapt_project_requires_configured_adapter() {
    let config = validate_config(&json!({}), &Utf8PathBuf::from("E:/Projects/svelte"))
        .expect("config should validate");
    let manifest = minimal_manifest();
    let server_manifest = BTreeMap::new();
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: Utf8PathBuf::from("E:/Projects/svelte/.svelte-kit"),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };

    let error = adapt_project(
        &config,
        &build_data,
        &BuilderServerMetadata::default(),
        &BuilderPrerendered::default(),
        &[],
        |_| Ok(()),
    )
    .expect_err("adapter should be required");

    assert!(error.to_string().contains("configured adapter"));
}

#[test]
fn invoke_adapter_returns_top_level_status_messages() {
    let config = config_with_adapter();
    let manifest = minimal_manifest();
    let server_manifest = BTreeMap::new();
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: Utf8PathBuf::from("E:/Projects/svelte/.svelte-kit"),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };

    let result = invoke_adapter(
        &config,
        &build_data,
        &BuilderServerMetadata::default(),
        &BuilderPrerendered::default(),
        &[],
        |builder| Ok(builder.routes().len()),
    )
    .expect("invoke should succeed");

    assert_eq!(result.adapter_name, "adapter-static");
    assert_eq!(result.output, 1);
    assert_eq!(result.status.using_message, "> Using adapter-static");
    assert_eq!(result.status.success_message, "done");
}
