use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{
    PreviewMiddlewareStep, build_preview_middleware_plan, build_preview_server_plan,
    validate_config,
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
        .join(format!("svelte-kit-preview-middleware-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn builds_preview_middleware_stack_plan() {
    let cwd = temp_dir("stack");
    let config = validate_config(
        &serde_json::json!({
            "kit": {
                "outDir": ".svelte-kit",
                "appDir": "_app",
                "paths": {
                    "base": "/base",
                    "assets": "https://cdn.example.com"
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");
    let server_plan = build_preview_server_plan(&cwd, &config.kit, false, "preview", "\"abc\"");
    let plan = build_preview_middleware_plan(&config.kit, &server_plan);

    assert_eq!(plan.steps.len(), 6);
    assert_eq!(
        plan.steps[0],
        PreviewMiddlewareStep::RemoveBaseMiddleware {
            name: "viteBaseMiddleware".to_string()
        }
    );
    assert_eq!(
        plan.steps[1],
        PreviewMiddlewareStep::ServeClientAssets {
            scope: "/_svelte_kit_assets".to_string(),
            dir: Utf8PathBuf::from(".svelte-kit/output/client"),
            immutable_prefix: "/_app/immutable".to_string(),
            immutable_cache_control: "public,max-age=31536000,immutable".to_string(),
        }
    );
    assert_eq!(
        plan.steps[2],
        PreviewMiddlewareStep::GuardBasePath {
            base: "/base".to_string()
        }
    );
    assert_eq!(
        plan.steps[3],
        PreviewMiddlewareStep::ServePrerenderedDependencies {
            scope: "/base".to_string(),
            dir: Utf8PathBuf::from(".svelte-kit/output/prerendered/dependencies"),
            etag: "\"abc\"".to_string(),
        }
    );
    assert_eq!(
        plan.steps[4],
        PreviewMiddlewareStep::ServePrerenderedPages {
            scope: "/base".to_string(),
            dir: Utf8PathBuf::from(".svelte-kit/output/prerendered/pages"),
            app_dir: "_app".to_string(),
            etag: "\"abc\"".to_string(),
        }
    );
    assert_eq!(plan.steps[5], PreviewMiddlewareStep::SsrRequestHandler);

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
