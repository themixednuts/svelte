use std::path::Path;

use camino::Utf8Path;
use serde_json::json;
use svelte_kit::{
    Error, ViteAliasFind, ViteUtilsError, app_server_module_id, env_dynamic_private_module_id,
    env_dynamic_public_module_id, env_static_private_module_id, env_static_public_module_id,
    error_for_missing_config, get_config_aliases, normalize_vite_id, posixify,
    service_worker_module_id, strip_virtual_prefix, validate_config, vite_not_found_response,
};

fn relative_replacement(replacement: &str) -> String {
    let cwd = std::env::current_dir().expect("cwd should be readable");
    Path::new(replacement)
        .strip_prefix(&cwd)
        .ok()
        .map(|path| posixify(path.to_string_lossy().as_ref()))
        .unwrap_or_else(|| posixify(replacement))
}

#[test]
fn transforms_kit_alias_to_vite_aliases() {
    let input = json!({
        "kit": {
            "alias": {
                "simpleKey": "simple/value",
                "key": "value",
                "key/*": "value/*",
                "$regexChar": "windows\\path",
                "$regexChar/*": "windows\\path\\*"
            }
        }
    });
    let config = validate_config(&input, Utf8Path::new(".")).expect("config should validate");

    let transformed = get_config_aliases(&config.kit)
        .expect("aliases should build")
        .into_iter()
        .map(|entry| {
            let find = match entry.find {
                ViteAliasFind::Literal(value) | ViteAliasFind::Pattern(value) => value,
            };
            (find, relative_replacement(&entry.replacement))
        })
        .collect::<Vec<_>>();
    let mut transformed = transformed;
    transformed.sort();

    let mut expected = vec![
        ("$lib".to_string(), "src/lib".to_string()),
        ("simpleKey".to_string(), "simple/value".to_string()),
        ("/^key$/".to_string(), "value".to_string()),
        ("/^key\\/(.+)$/".to_string(), "value/$1".to_string()),
        ("/^\\$regexChar$/".to_string(), "windows/path".to_string()),
        (
            "/^\\$regexChar\\/(.+)$/".to_string(),
            "windows/path/$1".to_string(),
        ),
    ];
    expected.sort();

    assert_eq!(transformed, expected);
}

#[test]
fn formats_simple_missing_config_error() {
    let error = error_for_missing_config("feature", "kit.adapter", "true");
    assert_eq!(
        error.to_string(),
        "To enable feature, add the following to your `svelte.config.js`:\n\nkit: {\n  adapter: true\n}"
    );
    assert!(matches!(
        error,
        Error::ViteUtils(ViteUtilsError::MissingConfig { feature_name, .. })
        if feature_name == "feature"
    ));
}

#[test]
fn formats_nested_missing_config_error() {
    assert_eq!(
        error_for_missing_config(
            "instrumentation.server.js",
            "kit.experimental.instrumentation.server",
            "true"
        )
        .to_string(),
        "To enable instrumentation.server.js, add the following to your `svelte.config.js`:\n\nkit: {\n  experimental: {\n    instrumentation: {\n      server: true\n    }\n  }\n}"
    );
}

#[test]
fn formats_deeply_nested_missing_config_error() {
    assert_eq!(
        error_for_missing_config("deep feature", "a.b.c.d.e", "\"value\"").to_string(),
        "To enable deep feature, add the following to your `svelte.config.js`:\n\na: {\n  b: {\n    c: {\n      d: {\n        e: \"value\"\n      }\n    }\n  }\n}"
    );
}

#[test]
fn formats_special_character_feature_names() {
    assert_eq!(
        error_for_missing_config("special-feature.js", "kit.special", "{ enabled: true }")
            .to_string(),
        "To enable special-feature.js, add the following to your `svelte.config.js`:\n\nkit: {\n  special: { enabled: true }\n}"
    );
}

#[test]
fn normalizes_virtual_and_project_ids() {
    let cwd = "E:/Projects/svelte";
    let lib = "E:/Projects/svelte/src/lib";

    assert_eq!(
        normalize_vite_id(
            app_server_module_id("E:/Projects/svelte/kit/packages/kit/src".into()).as_str(),
            lib,
            cwd
        ),
        "$app/server"
    );
    assert_eq!(
        normalize_vite_id(env_static_private_module_id(), lib, cwd),
        "$env/static/private"
    );
    assert_eq!(
        normalize_vite_id(env_static_public_module_id(), lib, cwd),
        "$env/static/public"
    );
    assert_eq!(
        normalize_vite_id(env_dynamic_private_module_id(), lib, cwd),
        "$env/dynamic/private"
    );
    assert_eq!(
        normalize_vite_id(env_dynamic_public_module_id(), lib, cwd),
        "$env/dynamic/public"
    );
    assert_eq!(
        normalize_vite_id(service_worker_module_id(), lib, cwd),
        "$service-worker"
    );
    assert_eq!(
        normalize_vite_id("E:/Projects/svelte/src/lib/server/thing.ts?x=y", lib, cwd),
        "$lib/server/thing.ts"
    );
    assert_eq!(
        normalize_vite_id("E:/Projects/svelte/src/routes/+page.ts", lib, cwd),
        "src/routes/+page.ts"
    );
}

#[test]
fn strips_virtual_prefix_only_when_present() {
    assert_eq!(
        strip_virtual_prefix("\0virtual:$env/static/public"),
        "$env/static/public"
    );
    assert_eq!(strip_virtual_prefix("plain/module"), "plain/module");
}

#[test]
fn renders_vite_not_found_redirect_and_messages() {
    let redirect = vite_not_found_response("/", "text/html", "/base");
    assert_eq!(redirect.status, 307);
    assert_eq!(redirect.header("location"), Some("/base"));

    let html = vite_not_found_response("/x", "text/html", "/base");
    assert_eq!(html.status, 404);
    assert!(html.body.as_deref().unwrap_or_default().contains("/base/x"));

    let text = vite_not_found_response("/x", "text/plain", "/base");
    assert_eq!(text.status, 404);
    assert!(text.body.as_deref().unwrap_or_default().contains("/base/x"));
}
