use std::collections::BTreeMap;

use camino::Utf8Path;
use serde_json::json;
use svelte_kit::{
    Error, ServiceWorkerBuildEntry, ViteGuardError, collect_service_worker_build_files,
    create_service_worker_module, render_service_worker_module,
    resolve_service_worker_virtual_module, service_worker_build_invocation_plan,
    service_worker_build_plan, service_worker_entry_output_filename,
    service_worker_runtime_asset_url, should_rename_service_worker_output, validate_config,
};

#[test]
fn collects_unique_service_worker_build_files() {
    let manifest = BTreeMap::from([
        (
            "entry".to_string(),
            ServiceWorkerBuildEntry {
                file: "entry.js".to_string(),
                css: vec!["entry.css".to_string()],
                assets: vec!["logo.svg".to_string()],
            },
        ),
        (
            "chunk".to_string(),
            ServiceWorkerBuildEntry {
                file: "chunk.js".to_string(),
                css: vec!["entry.css".to_string(), "chunk.css".to_string()],
                assets: vec!["font.woff2".to_string()],
            },
        ),
    ]);

    assert_eq!(
        collect_service_worker_build_files(&manifest),
        vec![
            "chunk.css".to_string(),
            "chunk.js".to_string(),
            "entry.css".to_string(),
            "entry.js".to_string(),
            "font.woff2".to_string(),
            "logo.svg".to_string()
        ]
    );
}

#[test]
fn renders_service_worker_module_contents() {
    let source = render_service_worker_module(
        "location.pathname.split('/').slice(0, -1).join('/')",
        &["entry.js".to_string(), "entry.css".to_string()],
        &["favicon.png".to_string()],
        &["/".to_string(), "/blog".to_string()],
        "v1",
    );

    assert!(source.contains("export const base ="));
    assert!(source.contains("base + \"/entry.js\""));
    assert!(source.contains("base + \"/favicon.png\""));
    assert!(source.contains("base + \"/blog\""));
    assert!(source.contains("export const version = \"v1\";"));
}

#[test]
fn creates_service_worker_virtual_module_from_config_assets() {
    let config = validate_config(
        &json!({
            "kit": {
                "paths": { "base": "/base" },
                "version": { "name": "v2" }
            }
        }),
        Utf8Path::new("."),
    )
    .expect("config should validate");

    let source = create_service_worker_module(
        &config.kit,
        &[
            "favicon.png".to_string(),
            ".DS_Store".to_string(),
            "robots.txt".to_string(),
        ],
    );

    assert!(source.contains("This module can only be imported inside a service worker"));
    assert!(source.contains("\"/base/favicon.png\""));
    assert!(source.contains("\"/base/robots.txt\""));
    assert!(!source.contains(".DS_Store"));
    assert!(source.contains("export const version = \"v2\";"));
}

#[test]
fn resolves_service_worker_virtual_modules() {
    let public_env = serde_json::Map::from_iter([(
        "PUBLIC_FOO".to_string(),
        serde_json::Value::String("bar".to_string()),
    )]);
    let service_worker_code = "export const version = 'v1';";

    assert_eq!(
        resolve_service_worker_virtual_module(
            "\0virtual:service-worker",
            service_worker_code,
            &public_env,
            "E:/Projects/svelte/src/lib",
            "E:/Projects/svelte"
        )
        .expect("service worker module should resolve"),
        service_worker_code
    );

    let env_module = resolve_service_worker_virtual_module(
        "\0virtual:env/static/public",
        service_worker_code,
        &public_env,
        "E:/Projects/svelte/src/lib",
        "E:/Projects/svelte",
    )
    .expect("public env module should resolve");
    assert!(env_module.contains("export const PUBLIC_FOO = \"bar\";"));
}

#[test]
fn rejects_non_public_virtual_modules_in_service_worker_build() {
    let error = resolve_service_worker_virtual_module(
        "\0virtual:env/dynamic/private",
        "noop",
        &serde_json::Map::new(),
        "E:/Projects/svelte/src/lib",
        "E:/Projects/svelte",
    )
    .expect_err("private env import should be rejected");

    assert!(
        error
            .to_string()
            .contains("Cannot import $env/dynamic/private")
    );
    assert!(matches!(
        error,
        Error::ViteGuard(ViteGuardError::ServiceWorkerImport { normalized })
        if normalized == "$env/dynamic/private"
    ));
}

#[test]
fn service_worker_output_naming_matches_rolldown_split() {
    assert_eq!(
        service_worker_entry_output_filename(true),
        "service-worker.js"
    );
    assert_eq!(
        service_worker_entry_output_filename(false),
        "service-worker.mjs"
    );
    assert!(!should_rename_service_worker_output(true));
    assert!(should_rename_service_worker_output(false));
}

#[test]
fn service_worker_build_plan_matches_expected_output_shape() {
    let config = validate_config(
        &json!({
            "kit": {
                "appDir": "_app",
                "alias": {
                    "@pkg": "src/pkg"
                }
            }
        }),
        Utf8Path::new("."),
    )
    .expect("config should validate");

    let plan =
        service_worker_build_plan(&config.kit, "E:/Projects/svelte/.svelte-kit/output", false)
            .expect("plan should build");

    assert_eq!(plan.out_dir, "E:/Projects/svelte/.svelte-kit/output/client");
    assert_eq!(plan.entry_file_name, "service-worker.mjs");
    assert_eq!(
        plan.asset_file_name_pattern,
        "_app/immutable/assets/[name].[hash][extname]"
    );
    assert!(plan.inline_dynamic_imports);
    assert!(plan.aliases.iter().any(|alias| match &alias.find {
        svelte_kit::ViteAliasFind::Literal(value) => value == "$lib",
        _ => false,
    }));
}

#[test]
fn service_worker_runtime_asset_url_matches_upstream_expression() {
    assert_eq!(
        service_worker_runtime_asset_url("foo.js"),
        "new URL(\"foo.js\", location.href).pathname"
    );
}

#[test]
fn service_worker_build_invocation_plan_combines_outputs() {
    let config = validate_config(
        &json!({
            "kit": {
                "appDir": "_app",
                "paths": { "base": "/base" },
                "version": { "name": "v1" }
            }
        }),
        Utf8Path::new("."),
    )
    .expect("config should validate");

    let build_entries = BTreeMap::from([(
        "entry".to_string(),
        ServiceWorkerBuildEntry {
            file: "entry.js".to_string(),
            css: vec!["entry.css".to_string()],
            assets: vec!["font.woff2".to_string()],
        },
    )]);
    let public_env = serde_json::Map::from_iter([(
        "PUBLIC_FOO".to_string(),
        serde_json::Value::String("bar".to_string()),
    )]);

    let plan = service_worker_build_invocation_plan(
        &config.kit,
        ".svelte-kit/output",
        &build_entries,
        &["favicon.png".to_string()],
        &["/base".to_string(), "/base/blog".to_string()],
        "v1",
        &public_env,
        false,
    )
    .expect("invocation plan should build");

    assert_eq!(plan.build_plan.entry_file_name, "service-worker.mjs");
    assert!(plan.service_worker_module.contains("base + \"/entry.js\""));
    assert!(plan.service_worker_module.contains("base + \"/blog\""));
    assert!(
        plan.public_env_module
            .contains("export const PUBLIC_FOO = \"bar\";")
    );
    assert_eq!(
        plan.rename_from.as_deref(),
        Some(".svelte-kit/output/client/service-worker.mjs")
    );
    assert_eq!(
        plan.rename_to.as_deref(),
        Some(".svelte-kit/output/client/service-worker.js")
    );
}
