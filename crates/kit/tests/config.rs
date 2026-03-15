use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use serde_json::json;
use svelte_kit::{
    BundleStrategy, ConfigError, CspMode, Error as KitError, JsSourceKind, ManifestConfig,
    PreloadStrategy, PrerenderPolicy, RouterResolution, RouterType, ServiceWorkerFilesFilter,
    TypeScriptConfigHook, load_config, load_error_page, load_project, load_template,
    validate_config,
};

fn cwd() -> Utf8PathBuf {
    Utf8PathBuf::from("E:/config-fixture")
}

fn repo_root() -> Utf8PathBuf {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|candidate| candidate.join("kit").join("packages").join("kit").is_dir())
        .expect("workspace root")
        .to_path_buf()
}

fn upstream_config_fixture(name: &str) -> Utf8PathBuf {
    repo_root()
        .join("kit")
        .join("packages")
        .join("kit")
        .join("src")
        .join("core")
        .join("config")
        .join("fixtures")
        .join(name)
}

fn temp_dir(label: &str) -> Utf8PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let dir = repo_root()
        .join("tmp")
        .join(format!("svelte-kit-config-{label}-{unique}"));
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
fn fills_in_defaults() {
    let config = validate_config(&json!({}), &cwd()).expect("validate config");

    assert!(!config.compiler_options.experimental.async_);
    assert_eq!(config.extensions, vec![".svelte"]);
    assert!(config.kit.alias.is_empty());
    assert_eq!(config.kit.app_dir, "_app");
    assert_eq!(config.kit.csp.mode, CspMode::Auto);
    assert!(config.kit.csp.directives.string_lists.is_empty());
    assert!(!config.kit.csp.directives.upgrade_insecure_requests);
    assert!(!config.kit.csp.directives.block_all_mixed_content);
    assert!(config.kit.adapter.is_none());
    assert!(config.kit.csrf.check_origin);
    assert!(config.kit.csrf.trusted_origins.is_empty());
    assert!(!config.kit.embedded);
    assert_eq!(config.kit.env.dir, "E:/config-fixture");
    assert_eq!(config.kit.env.public_prefix, "PUBLIC_");
    assert!(!config.kit.experimental.instrumentation.server);
    assert!(!config.kit.experimental.remote_functions);
    assert!(!config.kit.experimental.tracing.server);
    assert!(!config.kit.experimental.fork_preloads);
    assert_eq!(
        config.kit.files.src,
        Utf8PathBuf::from("E:/config-fixture/src")
    );
    assert_eq!(
        config.kit.files.assets,
        Utf8PathBuf::from("E:/config-fixture/static")
    );
    assert_eq!(
        config.kit.files.hooks.client,
        Utf8PathBuf::from("E:/config-fixture/src/hooks.client")
    );
    assert_eq!(
        config.kit.files.routes,
        Utf8PathBuf::from("E:/config-fixture/src/routes")
    );
    assert_eq!(config.kit.module_extensions, vec![".js", ".ts"]);
    assert_eq!(
        config.kit.out_dir,
        Utf8PathBuf::from("E:/config-fixture/.svelte-kit")
    );
    assert_eq!(
        config.kit.output.preload_strategy,
        PreloadStrategy::ModulePreload
    );
    assert_eq!(config.kit.output.bundle_strategy, BundleStrategy::Split);
    assert_eq!(config.kit.prerender.entries, vec!["*"]);
    assert_eq!(
        config.kit.prerender.handle_http_error,
        PrerenderPolicy::Fail
    );
    assert_eq!(
        config.kit.prerender.handle_missing_id,
        PrerenderPolicy::Fail
    );
    assert_eq!(
        config.kit.prerender.handle_entry_generator_mismatch,
        PrerenderPolicy::Fail
    );
    assert_eq!(
        config.kit.prerender.handle_unseen_routes,
        PrerenderPolicy::Fail
    );
    assert_eq!(config.kit.prerender.origin, "http://sveltekit-prerender");
    assert_eq!(config.kit.router.type_, RouterType::Pathname);
    assert_eq!(config.kit.router.resolution, RouterResolution::Client);
    assert!(config.kit.service_worker.includes("foo.js"));
    assert!(!config.kit.service_worker.includes(".DS_Store"));
    assert!(config.kit.service_worker.register);
    assert!(config.kit.service_worker.options.is_none());
    assert_eq!(
        config.kit.typescript.config,
        svelte_kit::TypeScriptConfigHook::Identity
    );
    assert!(!config.kit.version.name.is_empty());
}

#[test]
fn accepts_compiler_options_async() {
    let config = validate_config(
        &json!({
            "compilerOptions": {
                "experimental": {
                    "async": true
                }
            }
        }),
        &cwd(),
    )
    .expect("validate config");

    assert!(config.compiler_options.experimental.async_);
}

#[test]
fn loads_default_upstream_js_config() {
    let cwd = upstream_config_fixture("default");
    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.extensions, vec![".svelte"]);
    assert_eq!(config.kit.files.src, cwd.join("src"));
    assert_eq!(config.kit.files.routes, cwd.join("src").join("routes"));
    assert_eq!(config.kit.files.assets, cwd.join("static"));
    assert!(!config.kit.version.name.is_empty());
}

#[test]
fn loads_default_upstream_ts_config() {
    let cwd = upstream_config_fixture("typescript");
    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.extensions, vec![".svelte"]);
    assert_eq!(config.kit.files.src, cwd.join("src"));
    assert_eq!(config.kit.files.routes, cwd.join("src").join("routes"));
}

#[test]
fn prefers_js_config_when_both_js_and_ts_are_present() {
    let cwd = upstream_config_fixture("multiple");
    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.extensions, vec![".svelte"]);
    assert_eq!(config.kit.files.src, cwd.join("src"));
}

#[test]
fn errors_when_loaded_config_default_export_is_not_an_object() {
    let cwd = upstream_config_fixture("export-string");
    let error = load_config(&cwd).expect_err("string export should error");

    assert_eq!(
        error.to_string(),
        "The Svelte config file must have a configuration object as its default export. See https://svelte.dev/docs/kit/configuration"
    );
}

#[test]
fn load_config_respects_custom_src_fixture() {
    let cwd = upstream_config_fixture("custom-src");
    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));
    assert_eq!(config.kit.files.lib, cwd.join("source").join("lib"));
    assert_eq!(config.kit.files.routes, cwd.join("source").join("routes"));
}

#[test]
fn fills_in_partial_blanks() {
    let config = validate_config(
        &json!({
            "kit": {
                "files": { "assets": "public" },
                "version": { "name": "0" }
            }
        }),
        &cwd(),
    )
    .expect("validate config");

    assert_eq!(
        config.kit.files.assets,
        Utf8PathBuf::from("E:/config-fixture/public")
    );
    assert_eq!(
        config.kit.files.routes,
        Utf8PathBuf::from("E:/config-fixture/src/routes")
    );
    assert_eq!(config.kit.version.name, "0");
}

#[test]
fn validates_alias_csrf_and_service_worker_options() {
    let config = validate_config(
        &json!({
            "kit": {
                "alias": {
                    "$lib": "src/lib",
                    "foo": "bar"
                },
                "csrf": {
                    "checkOrigin": false,
                    "trustedOrigins": ["https://a.example", "https://b.example"]
                },
                "embedded": true,
                "serviceWorker": {
                    "register": false,
                    "options": {
                        "type": "module",
                        "scope": "/app"
                    }
                }
            }
        }),
        &cwd(),
    )
    .expect("validate config");

    assert_eq!(config.kit.alias.get("$lib"), Some(&"src/lib".to_string()));
    assert!(!config.kit.csrf.check_origin);
    assert_eq!(
        config.kit.csrf.trusted_origins,
        vec![
            "https://a.example".to_string(),
            "https://b.example".to_string()
        ]
    );
    assert!(config.kit.embedded);
    assert!(!config.kit.service_worker.register);
    assert_eq!(
        config
            .kit
            .service_worker
            .options
            .as_ref()
            .and_then(|options| options.get("type")),
        Some(&json!("module"))
    );
}

#[test]
fn accepts_null_and_object_adapter_values() {
    let config = validate_config(
        &json!({
            "kit": { "adapter": null }
        }),
        &cwd(),
    )
    .expect("validate config");
    assert!(config.kit.adapter.is_none());

    let config = validate_config(
        &json!({
            "kit": {
                "adapter": {
                    "name": "static-adapter"
                }
            }
        }),
        &cwd(),
    )
    .expect("validate config");
    assert_eq!(
        config
            .kit
            .adapter
            .as_ref()
            .and_then(|adapter| adapter.raw.get("name")),
        Some(&json!("static-adapter"))
    );
}

#[test]
fn validates_csp_configuration() {
    let config = validate_config(
        &json!({
            "kit": {
                "csp": {
                    "mode": "nonce",
                    "directives": {
                        "default-src": ["self"],
                        "script-src": ["self", "unsafe-inline"],
                        "upgrade-insecure-requests": true
                    },
                    "reportOnly": {
                        "report-uri": ["/csp-report"],
                        "block-all-mixed-content": true
                    }
                }
            }
        }),
        &cwd(),
    )
    .expect("validate config");

    assert_eq!(config.kit.csp.mode, CspMode::Nonce);
    assert_eq!(
        config.kit.csp.directives.string_lists.get("default-src"),
        Some(&vec!["self".to_string()])
    );
    assert_eq!(
        config.kit.csp.directives.string_lists.get("script-src"),
        Some(&vec!["self".to_string(), "unsafe-inline".to_string()])
    );
    assert!(config.kit.csp.directives.upgrade_insecure_requests);
    assert_eq!(
        config.kit.csp.report_only.string_lists.get("report-uri"),
        Some(&vec!["/csp-report".to_string()])
    );
    assert!(config.kit.csp.report_only.block_all_mixed_content);
}

#[test]
fn uses_src_prefix_for_other_file_defaults() {
    let config = validate_config(
        &json!({
            "kit": {
                "files": { "src": "source" }
            }
        }),
        &cwd(),
    )
    .expect("validate config");

    assert_eq!(
        config.kit.files.src,
        Utf8PathBuf::from("E:/config-fixture/source")
    );
    assert_eq!(
        config.kit.files.lib,
        Utf8PathBuf::from("E:/config-fixture/source/lib")
    );
    assert_eq!(
        config.kit.files.params,
        Utf8PathBuf::from("E:/config-fixture/source/params")
    );
}

#[test]
fn rejects_invalid_alias_values() {
    let error = validate_config(
        &json!({
            "kit": { "alias": { "$lib": 1 } }
        }),
        &cwd(),
    )
    .expect_err("invalid alias should error");

    assert_eq!(
        error.to_string(),
        "config.kit.alias.$lib should be a string, if specified"
    );
}

#[test]
fn rejects_invalid_csp_values() {
    let error = validate_config(
        &json!({
            "kit": { "csp": { "mode": "bad" } }
        }),
        &cwd(),
    )
    .expect_err("invalid csp mode should error");
    assert_eq!(
        error.to_string(),
        "config.kit.csp.mode should be one of \"auto\", \"hash\" or \"nonce\""
    );

    let error = validate_config(
        &json!({
            "kit": { "csp": { "directives": { "default-src": true } } }
        }),
        &cwd(),
    )
    .expect_err("invalid csp directive should error");
    assert_eq!(
        error.to_string(),
        "config.kit.csp.directives.default-src must be an array of strings, if specified"
    );

    let error = validate_config(
        &json!({
            "kit": { "csp": { "directives": { "potato": [] } } }
        }),
        &cwd(),
    )
    .expect_err("unknown csp directive should error");
    assert_eq!(
        error.to_string(),
        "Unexpected option config.kit.csp.directives.potato"
    );
}

#[test]
fn ignores_unknown_top_level_options() {
    let config = validate_config(
        &json!({
            "onwarn": "ignored"
        }),
        &cwd(),
    )
    .expect("validate config");

    assert_eq!(config.extensions, vec![".svelte"]);
}

#[test]
fn errors_on_invalid_values() {
    let error = validate_config(
        &json!({
            "kit": { "appDir": 42 }
        }),
        &cwd(),
    )
    .expect_err("invalid appDir should error");

    assert_eq!(
        error.to_string(),
        "config.kit.appDir should be a string, if specified"
    );
    assert!(matches!(
        error,
        KitError::Config(ConfigError::ExpectedString { ref keypath })
            if keypath == "config.kit.appDir"
    ));
}

#[test]
fn errors_on_invalid_nested_values() {
    let error = validate_config(
        &json!({
            "kit": { "files": { "potato": "blah" } }
        }),
        &cwd(),
    )
    .expect_err("unknown file option should error");

    assert_eq!(
        error.to_string(),
        "Unexpected option config.kit.files.potato"
    );
}

#[test]
fn rejects_unknown_kit_options() {
    let error = validate_config(
        &json!({
            "kit": { "potato": true }
        }),
        &cwd(),
    )
    .expect_err("unknown kit option should error");

    assert_eq!(error.to_string(), "Unexpected option config.kit.potato");
}

#[test]
fn rejects_misnested_top_level_options_under_kit() {
    let error = validate_config(
        &json!({
            "kit": { "extensions": [".funk"] }
        }),
        &cwd(),
    )
    .expect_err("misnested extensions should error");

    assert_eq!(
        error.to_string(),
        "Unexpected option config.kit.extensions (did you mean config.extensions?)"
    );
}

#[test]
fn rejects_unknown_nested_object_options() {
    let error = validate_config(
        &json!({
            "kit": { "router": { "potato": true } }
        }),
        &cwd(),
    )
    .expect_err("unknown router option should error");
    assert_eq!(
        error.to_string(),
        "Unexpected option config.kit.router.potato"
    );

    let error = validate_config(
        &json!({
            "kit": { "experimental": { "tracing": { "potato": true } } }
        }),
        &cwd(),
    )
    .expect_err("unknown tracing option should error");
    assert_eq!(
        error.to_string(),
        "Unexpected option config.kit.experimental.tracing.potato"
    );

    let error = validate_config(
        &json!({
            "kit": { "env": { "potato": true } }
        }),
        &cwd(),
    )
    .expect_err("unknown env option should error");
    assert_eq!(error.to_string(), "Unexpected option config.kit.env.potato");
}

#[test]
fn errors_on_extension_without_leading_dot() {
    let error = validate_config(
        &json!({
            "extensions": ["blah"]
        }),
        &cwd(),
    )
    .expect_err("invalid extension should error");

    assert_eq!(
        error.to_string(),
        "Each member of config.extensions must start with '.' — saw 'blah'"
    );
}

#[test]
fn errors_on_non_alphanumeric_extension() {
    let error = validate_config(
        &json!({
            "extensions": [".svelte-md!"]
        }),
        &cwd(),
    )
    .expect_err("invalid extension should error");

    assert_eq!(
        error.to_string(),
        "File extensions must be alphanumeric — saw '.svelte-md!'"
    );
}

#[test]
fn fails_if_app_dir_is_blank() {
    let error = validate_config(
        &json!({
            "kit": { "appDir": "" }
        }),
        &cwd(),
    )
    .expect_err("blank appDir should error");

    assert_eq!(error.to_string(), "config.kit.appDir cannot be empty");
}

#[test]
fn fails_if_paths_base_is_invalid() {
    let error = validate_config(
        &json!({
            "kit": { "paths": { "base": "github-pages/" } }
        }),
        &cwd(),
    )
    .expect_err("invalid base path should error");

    assert_eq!(
        error.to_string(),
        "config.kit.paths.base option must either be the empty string or a root-relative path that starts but doesn't end with '/'. See https://svelte.dev/docs/kit/configuration#paths"
    );
}

#[test]
fn fails_if_paths_assets_is_relative() {
    let error = validate_config(
        &json!({
            "kit": { "paths": { "assets": "foo" } }
        }),
        &cwd(),
    )
    .expect_err("relative assets path should error");

    assert_eq!(
        error.to_string(),
        "config.kit.paths.assets option must be an absolute path, if specified. See https://svelte.dev/docs/kit/configuration#paths"
    );
}

#[test]
fn fails_if_paths_assets_has_trailing_slash() {
    let error = validate_config(
        &json!({
            "kit": { "paths": { "assets": "https://cdn.example.com/stuff/" } }
        }),
        &cwd(),
    )
    .expect_err("trailing slash assets path should error");

    assert_eq!(
        error.to_string(),
        "config.kit.paths.assets option must not end with '/'. See https://svelte.dev/docs/kit/configuration#paths"
    );
}

#[test]
fn fails_if_prerender_entries_are_invalid() {
    let error = validate_config(
        &json!({
            "kit": { "prerender": { "entries": ["foo"] } }
        }),
        &cwd(),
    )
    .expect_err("invalid prerender entry should error");

    assert_eq!(
        error.to_string(),
        "Each member of config.kit.prerender.entries must be either '*' or an absolute path beginning with '/' — saw 'foo'"
    );
}

#[test]
fn validates_prerender_policies_and_origin() {
    let config = validate_config(
        &json!({
            "kit": {
                "prerender": {
                    "handleHttpError": "warn",
                    "handleMissingId": "ignore",
                    "handleEntryGeneratorMismatch": "warn",
                    "handleUnseenRoutes": "ignore",
                    "origin": "https://example.com"
                }
            }
        }),
        &cwd(),
    )
    .expect("validate config");

    assert_eq!(
        config.kit.prerender.handle_http_error,
        PrerenderPolicy::Warn
    );
    assert_eq!(
        config.kit.prerender.handle_missing_id,
        PrerenderPolicy::Ignore
    );
    assert_eq!(
        config.kit.prerender.handle_entry_generator_mismatch,
        PrerenderPolicy::Warn
    );
    assert_eq!(
        config.kit.prerender.handle_unseen_routes,
        PrerenderPolicy::Ignore
    );
    assert_eq!(config.kit.prerender.origin, "https://example.com");
}

#[test]
fn rejects_invalid_prerender_origin() {
    let error = validate_config(
        &json!({
            "kit": { "prerender": { "origin": "not-a-url" } }
        }),
        &cwd(),
    )
    .expect_err("invalid prerender origin should error");

    assert_eq!(
        error.to_string(),
        "config.kit.prerender.origin must be a valid origin"
    );

    let error = validate_config(
        &json!({
            "kit": { "prerender": { "origin": "https://example.com/foo" } }
        }),
        &cwd(),
    )
    .expect_err("non-origin prerender url should error");

    assert_eq!(
        error.to_string(),
        "config.kit.prerender.origin must be a valid origin (https://example.com rather than https://example.com/foo)"
    );
}

#[test]
fn rejects_invalid_prerender_policy_values() {
    let error = validate_config(
        &json!({
            "kit": { "prerender": { "handleHttpError": "nope" } }
        }),
        &cwd(),
    )
    .expect_err("invalid handleHttpError policy should error");

    assert_eq!(
        error.to_string(),
        "config.kit.prerender.handleHttpError should be \"fail\", \"warn\", \"ignore\" or a custom function"
    );
}

#[test]
fn rejects_server_resolution_with_hash_router() {
    let error = validate_config(
        &json!({
            "kit": {
                "router": { "type": "hash", "resolution": "server" }
            }
        }),
        &cwd(),
    )
    .expect_err("hash+server router should error");

    assert_eq!(
        error.to_string(),
        "The `router.resolution` option cannot be 'server' if `router.type` is 'hash'"
    );
}

#[test]
fn rejects_server_resolution_with_non_split_bundle_strategy() {
    let error = validate_config(
        &json!({
            "kit": {
                "router": { "resolution": "server" },
                "output": { "bundleStrategy": "inline" }
            }
        }),
        &cwd(),
    )
    .expect_err("server router with inline bundle should error");

    assert_eq!(
        error.to_string(),
        "The `router.resolution` option cannot be 'server' if `output.bundleStrategy` is 'inline' or 'single'"
    );
}

#[test]
fn accepts_valid_tracing_values() {
    let enabled = validate_config(
        &json!({
            "kit": { "experimental": { "tracing": { "server": true } } }
        }),
        &cwd(),
    )
    .expect("validate config");
    assert!(enabled.kit.experimental.tracing.server);

    let disabled = validate_config(
        &json!({
            "kit": { "experimental": { "tracing": { "server": false } } }
        }),
        &cwd(),
    )
    .expect("validate config");
    assert!(!disabled.kit.experimental.tracing.server);

    let defaults = validate_config(
        &json!({
            "kit": { "experimental": { "tracing": null } }
        }),
        &cwd(),
    )
    .expect("validate config");
    assert!(!defaults.kit.experimental.tracing.server);
}

#[test]
fn rejects_invalid_tracing_values() {
    let error = validate_config(
        &json!({
            "kit": { "experimental": { "tracing": true } }
        }),
        &cwd(),
    )
    .expect_err("boolean tracing should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.tracing should be an object"
    );

    let error = validate_config(
        &json!({
            "kit": { "experimental": { "tracing": "server" } }
        }),
        &cwd(),
    )
    .expect_err("string tracing should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.tracing should be an object"
    );

    let error = validate_config(
        &json!({
            "kit": { "experimental": { "tracing": { "server": "invalid" } } }
        }),
        &cwd(),
    )
    .expect_err("invalid tracing.server should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.tracing.server should be true or false, if specified"
    );
}

#[test]
fn rejects_invalid_fork_preloads_values() {
    let error = validate_config(
        &json!({
            "kit": { "experimental": { "forkPreloads": "true" } }
        }),
        &cwd(),
    )
    .expect_err("string forkPreloads should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.forkPreloads should be true or false, if specified"
    );

    let error = validate_config(
        &json!({
            "kit": { "experimental": { "forkPreloads": 1 } }
        }),
        &cwd(),
    )
    .expect_err("numeric forkPreloads should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.forkPreloads should be true or false, if specified"
    );
}

#[test]
fn rejects_invalid_instrumentation_values() {
    let error = validate_config(
        &json!({
            "kit": { "experimental": { "instrumentation": true } }
        }),
        &cwd(),
    )
    .expect_err("boolean instrumentation should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.instrumentation should be an object"
    );

    let error = validate_config(
        &json!({
            "kit": { "experimental": { "instrumentation": { "server": "invalid" } } }
        }),
        &cwd(),
    )
    .expect_err("invalid instrumentation.server should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.instrumentation.server should be true or false, if specified"
    );
}

#[test]
fn rejects_invalid_remote_functions_values() {
    let error = validate_config(
        &json!({
            "kit": { "experimental": { "remoteFunctions": "true" } }
        }),
        &cwd(),
    )
    .expect_err("string remoteFunctions should error");
    assert_eq!(
        error.to_string(),
        "config.kit.experimental.remoteFunctions should be true or false, if specified"
    );
}

#[test]
fn rejects_invalid_service_worker_options() {
    let error = validate_config(
        &json!({
            "kit": { "serviceWorker": { "options": true } }
        }),
        &cwd(),
    )
    .expect_err("invalid serviceWorker.options should error");
    assert_eq!(
        error.to_string(),
        "config.kit.serviceWorker.options should be an object"
    );
}

#[test]
fn rejects_invalid_service_worker_files_values() {
    let error = validate_config(
        &json!({
            "kit": { "serviceWorker": { "files": true } }
        }),
        &cwd(),
    )
    .expect_err("invalid serviceWorker.files should error");
    assert_eq!(
        error.to_string(),
        "config.kit.serviceWorker.files should be a function, if specified"
    );
}

#[test]
fn rejects_invalid_typescript_config_values() {
    let error = validate_config(
        &json!({
            "kit": { "typescript": { "config": true } }
        }),
        &cwd(),
    )
    .expect_err("invalid typescript.config should error");
    assert_eq!(
        error.to_string(),
        "config.kit.typescript.config should be a function, if specified"
    );
}

#[test]
fn rejects_invalid_adapter_values() {
    let error = validate_config(
        &json!({
            "kit": { "adapter": "adapter-static" }
        }),
        &cwd(),
    )
    .expect_err("invalid adapter should error");
    assert_eq!(
        error.to_string(),
        "config.kit.adapter should be an object with an \"adapt\" method. See https://svelte.dev/docs/kit/adapters"
    );
}

#[test]
fn builds_manifest_config_from_validated_config() {
    let config = validate_config(
        &json!({
            "extensions": [".svelte", ".funk"],
            "kit": {
                "files": {
                    "src": "source",
                    "assets": "public",
                    "hooks": {
                        "client": "hooks/client",
                        "server": "hooks/server",
                        "universal": "hooks/universal"
                    }
                },
                "moduleExtensions": [".js", ".ts", ".mts"]
            }
        }),
        &cwd(),
    )
    .expect("validate config");

    let manifest = ManifestConfig::from_validated_config(&config, cwd());

    assert_eq!(
        manifest.routes_dir,
        Utf8PathBuf::from("E:/config-fixture/source/routes")
    );
    assert_eq!(
        manifest.assets_dir,
        Utf8PathBuf::from("E:/config-fixture/public")
    );
    assert_eq!(
        manifest.hooks_client,
        Utf8PathBuf::from("E:/config-fixture/hooks/client")
    );
    assert_eq!(manifest.component_extensions, vec![".svelte", ".funk"]);
    assert_eq!(manifest.module_extensions, vec![".js", ".ts", ".mts"]);
}

#[test]
fn loads_valid_app_template() {
    let cwd = temp_dir("template-valid");
    let app_template = cwd.join("src").join("app.html");
    write_file(
        &app_template,
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );

    let config = validate_config(&json!({}), &cwd).expect("validate config");
    let template = load_template(&cwd, &config).expect("load template");

    assert!(template.contains("%sveltekit.head%"));
    assert!(template.contains("%sveltekit.body%"));

    fs::remove_dir_all(cwd).expect("remove temp dir");
}

#[test]
fn errors_when_app_template_is_missing() {
    let cwd = temp_dir("template-missing");
    let config = validate_config(&json!({}), &cwd).expect("validate config");

    let error = load_template(&cwd, &config).expect_err("missing template should error");
    assert_eq!(error.to_string(), "src/app.html does not exist");

    fs::remove_dir_all(cwd).expect("remove temp dir");
}

#[test]
fn errors_when_app_template_is_missing_required_tags() {
    let cwd = temp_dir("template-tags");
    let app_template = cwd.join("src").join("app.html");
    write_file(&app_template, "<html><body>%sveltekit.body%</body></html>");

    let config = validate_config(&json!({}), &cwd).expect("validate config");
    let error = load_template(&cwd, &config).expect_err("missing head tag should error");

    assert_eq!(
        error.to_string(),
        "src/app.html is missing %sveltekit.head%"
    );

    fs::remove_dir_all(cwd).expect("remove temp dir");
}

#[test]
fn errors_when_template_env_variable_uses_private_prefix() {
    let cwd = temp_dir("template-env-prefix");
    let app_template = cwd.join("src").join("app.html");
    write_file(
        &app_template,
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%%sveltekit.env.SECRET_KEY%</body></html>",
    );

    let config = validate_config(&json!({}), &cwd).expect("validate config");
    let error = load_template(&cwd, &config).expect_err("private env placeholder should error");

    assert_eq!(
        error.to_string(),
        "Environment variables in src/app.html must start with PUBLIC_ (saw %sveltekit.env.SECRET_KEY%)"
    );

    fs::remove_dir_all(cwd).expect("remove temp dir");
}

#[test]
fn loads_explicit_error_template_when_present() {
    let cwd = temp_dir("error-template");
    let error_template = cwd.join("src").join("error.html");
    write_file(&error_template, "<h1>custom error</h1>");

    let config = validate_config(&json!({}), &cwd).expect("validate config");
    let page = load_error_page(&config).expect("load error page");

    assert_eq!(page, "<h1>custom error</h1>");

    fs::remove_dir_all(cwd).expect("remove temp dir");
}

#[test]
fn falls_back_to_default_error_template_when_missing() {
    let cwd = temp_dir("error-template-fallback");
    let config = validate_config(&json!({}), &cwd).expect("validate config");
    let page = load_error_page(&config).expect("load error page");

    assert!(page.contains("%sveltekit.error.message%"));
    assert!(page.contains("%sveltekit.status%"));

    fs::remove_dir_all(cwd).expect("remove temp dir");
}

#[test]
fn loads_project_from_validated_config_and_routes() {
    let cwd = temp_dir("project-load");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("error.html"),
        "<h1>project error</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );

    let project = load_project(&cwd).expect("load project");

    assert_eq!(project.cwd, cwd);
    assert_eq!(project.manifest.routes.len(), 1);
    assert_eq!(project.manifest.nodes.len(), 3);
    assert!(project.template.contains("%sveltekit.head%"));
    assert_eq!(project.error_page, "<h1>project error</h1>");

    fs::remove_dir_all(&project.cwd).expect("remove temp dir");
}

#[test]
fn load_project_uses_default_config_when_no_config_file_exists() {
    let cwd = temp_dir("project-default-config");
    write_file(
        &cwd.join("src").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );

    let project = load_project(&cwd).expect("load project");

    assert_eq!(
        project.config.kit.files.routes,
        cwd.join("src").join("routes")
    );
    assert_eq!(project.manifest.routes.len(), 1);
    assert!(project.error_page.contains("%sveltekit.error.message%"));

    fs::remove_dir_all(&project.cwd).expect("remove temp dir");
}

#[test]
fn load_project_respects_custom_src_from_js_config_file() {
    let cwd = temp_dir("project-custom-src-js");
    write_file(
        &cwd.join("svelte.config.js"),
        "export default { kit: { files: { src: 'source' } } };",
    );
    write_file(
        &cwd.join("source").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("source").join("routes").join("+page.svelte"),
        "<h1>custom home</h1>",
    );

    let project = load_project(&cwd).expect("load project");

    assert_eq!(project.config.kit.files.src, cwd.join("source"));
    assert_eq!(
        project.config.kit.files.routes,
        cwd.join("source").join("routes")
    );
    assert_eq!(project.manifest.routes.len(), 1);

    fs::remove_dir_all(&project.cwd).expect("remove temp dir");
}

#[test]
fn load_project_respects_ts_config_file() {
    let cwd = temp_dir("project-ts-config");
    write_file(
        &cwd.join("svelte.config.ts"),
        "export default { kit: { files: { src: 'source' } } };",
    );
    write_file(
        &cwd.join("source").join("app.html"),
        "<html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("source").join("routes").join("+page.svelte"),
        "<h1>ts home</h1>",
    );

    let project = load_project(&cwd).expect("load project");

    assert_eq!(project.config.kit.files.src, cwd.join("source"));
    assert_eq!(project.manifest.routes.len(), 1);

    fs::remove_dir_all(&project.cwd).expect("remove temp dir");
}

#[test]
fn load_config_preserves_inline_function_valued_options() {
    let cwd = temp_dir("config-inline-functions");
    write_file(
        &cwd.join("svelte.config.js"),
        r#"
export default {
	kit: {
		adapter: {
			adapt() {
				return null;
			}
		},
		serviceWorker: {
			files: (filename) => !filename.endsWith('.map')
		},
		prerender: {
			handleHttpError: ({ message }) => {
				throw new Error(message);
			},
			handleMissingId: (details) => details.path
		},
		typescript: {
			config: (config) => ({ ...config, extends: './tsconfig.base.json' })
		}
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(
        config
            .kit
            .adapter
            .as_ref()
            .and_then(|adapter| adapter.adapt_source()),
        Some("adapt() {\n\t\t\t\treturn null;\n\t\t\t}")
    );
    assert_eq!(
        config.kit.adapter.as_ref().map(|adapter| adapter
            .source
            .as_ref()
            .expect("adapter source")
            .kind()),
        Some(JsSourceKind::Method)
    );
    assert_eq!(
        config.kit.service_worker.custom_filter_source(),
        Some("(filename) => !filename.endsWith('.map')")
    );
    assert!(matches!(
        &config.kit.service_worker.files,
        ServiceWorkerFilesFilter::Source(source)
            if source.kind() == JsSourceKind::Function
    ));
    assert_eq!(
        config.kit.typescript.custom_config_source(),
        Some("(config) => ({ ...config, extends: './tsconfig.base.json' })")
    );
    assert!(matches!(
        &config.kit.typescript.config,
        TypeScriptConfigHook::Source(source)
            if source.kind() == JsSourceKind::Function
    ));
    assert_eq!(
        config.kit.prerender.handle_http_error.custom_source(),
        Some("({ message }) => {\n\t\t\t\tthrow new Error(message);\n\t\t\t}")
    );
    assert!(matches!(
        &config.kit.prerender.handle_http_error,
        PrerenderPolicy::Source(source)
            if source.kind() == JsSourceKind::Function
    ));
    assert_eq!(
        config.kit.prerender.handle_missing_id.custom_source(),
        Some("(details) => details.path")
    );
    assert!(matches!(
        &config.kit.prerender.handle_missing_id,
        PrerenderPolicy::Source(source)
            if source.kind() == JsSourceKind::Function
    ));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_top_level_identifier_references() {
    let cwd = temp_dir("config-top-level-identifiers");
    write_file(
        &cwd.join("svelte.config.js"),
        r#"
const src = 'source';
const swFiles = (filename) => !filename.endsWith('.map');
export default {
	extensions: ['.svelte'],
	kit: {
		files: { src },
		serviceWorker: { files: swFiles }
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));
    assert_eq!(
        config.kit.service_worker.custom_filter_source(),
        Some("(filename) => !filename.endsWith('.map')")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_preserves_imported_adapter_call_expression() {
    let cwd = temp_dir("config-imported-adapter");
    write_file(
        &cwd.join("svelte.config.js"),
        r#"
import adapter from '@sveltejs/adapter-auto';
export default {
	kit: {
		adapter: adapter()
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(
        config
            .kit
            .adapter
            .as_ref()
            .and_then(|adapter| adapter.adapt_source()),
        Some("adapter()")
    );
    assert_eq!(
        config.kit.adapter.as_ref().map(|adapter| adapter
            .source
            .as_ref()
            .expect("adapter source")
            .kind()),
        Some(JsSourceKind::CallExpression)
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_unwraps_common_typescript_expression_wrappers() {
    let cwd = temp_dir("config-ts-wrappers");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
const src = 'source';
export default ({
	kit: {
		files: { src }
	}
} satisfies Record<string, unknown>) as Record<string, unknown>;
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_local_named_imports() {
    let cwd = temp_dir("config-local-named-imports");
    write_file(
        &cwd.join("constants.ts"),
        "export const src = 'source';\nexport const extensions = ['.svelte'];\n",
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
import { src, extensions } from './constants';
export default {
	extensions,
	kit: {
		files: { src }
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.extensions, vec![".svelte"]);
    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_local_default_imports() {
    let cwd = temp_dir("config-local-default-imports");
    write_file(
        &cwd.join("shared.ts"),
        r#"
const src = 'source';
export default {
	kit: {
		files: { src }
	}
};
"#,
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        "import config from './shared';\nexport default config;\n",
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_local_re_exports() {
    let cwd = temp_dir("config-local-reexports");
    write_file(&cwd.join("constants.ts"), "export const src = 'source';\n");
    write_file(
        &cwd.join("shared.ts"),
        "export { src } from './constants';\n",
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
import { src } from './shared';
export default {
	kit: {
		files: { src }
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_local_default_re_exports() {
    let cwd = temp_dir("config-local-default-reexports");
    write_file(
        &cwd.join("base.ts"),
        r#"
export default {
	kit: {
		files: {
			src: 'source'
		}
	}
};
"#,
    );
    write_file(
        &cwd.join("shared.ts"),
        "export { default } from './base';\n",
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        "import config from './shared';\nexport default config;\n",
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_default_as_named_re_exports() {
    let cwd = temp_dir("config-default-as-named-reexports");
    write_file(
        &cwd.join("base.ts"),
        r#"
export default {
	kit: {
		files: {
			src: 'source'
		}
	}
};
"#,
    );
    write_file(
        &cwd.join("shared.ts"),
        "export { default as config } from './base';\n",
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        "import { config } from './shared';\nexport default config;\n",
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_export_star_re_exports() {
    let cwd = temp_dir("config-export-star-reexports");
    write_file(&cwd.join("constants.ts"), "export const src = 'source';\n");
    write_file(&cwd.join("shared.ts"), "export * from './constants';\n");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
import { src } from './shared';
export default {
	kit: {
		files: { src }
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_export_star_namespace_re_exports() {
    let cwd = temp_dir("config-export-star-namespace-reexports");
    write_file(
        &cwd.join("constants.ts"),
        r#"
export const kit = {
	files: {
		src: 'source'
	}
};
"#,
    );
    write_file(
        &cwd.join("shared.ts"),
        "export * as shared from './constants';\n",
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
import { shared } from './shared';
export default {
	kit: shared.kit
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_local_object_member_access() {
    let cwd = temp_dir("config-local-member-access");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
const shared = {
	kit: {
		files: {
			src: 'source'
		}
	}
};

export default {
	kit: shared.kit
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_namespace_import_member_access() {
    let cwd = temp_dir("config-namespace-import-member-access");
    write_file(
        &cwd.join("shared.ts"),
        r#"
export const kit = {
	files: {
		src: 'source'
	}
};
"#,
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
import * as shared from './shared';

export default {
	kit: shared.kit
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_local_object_spreads() {
    let cwd = temp_dir("config-local-object-spreads");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
const shared_files = {
	src: 'source'
};

const shared_kit = {
	files: shared_files
};

export default {
	...{
		extensions: ['.svelte']
	},
	kit: {
		...shared_kit
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.extensions, vec![".svelte"]);
    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_imported_object_spreads() {
    let cwd = temp_dir("config-imported-object-spreads");
    write_file(
        &cwd.join("shared.ts"),
        r#"
export const files = {
	src: 'source'
};

export const kit = {
	files
};
"#,
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
import { kit } from './shared';

export default {
	kit: {
		...kit
	}
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_local_object_destructuring() {
    let cwd = temp_dir("config-local-object-destructuring");
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
const shared = {
	kit: {
		files: {
			src: 'source'
		}
	}
};

const { kit } = shared;

export default { kit };
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn load_config_resolves_nested_object_destructuring() {
    let cwd = temp_dir("config-nested-object-destructuring");
    write_file(
        &cwd.join("shared.ts"),
        r#"
export const shared = {
	kit: {
		files: {
			src: 'source'
		}
	}
};
"#,
    );
    write_file(
        &cwd.join("svelte.config.ts"),
        r#"
import { shared } from './shared';

const {
	kit: { files }
} = shared;

export default {
	kit: { files }
};
"#,
    );

    let config = load_config(&cwd).expect("load config");

    assert_eq!(config.kit.files.src, cwd.join("source"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
