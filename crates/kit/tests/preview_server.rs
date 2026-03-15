use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{build_preview_server_plan, validate_config};

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
        .join(format!("svelte-kit-preview-server-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn builds_preview_server_plan() {
    let cwd = temp_dir("plan");
    let config = validate_config(
        &serde_json::json!({
            "kit": {
                "outDir": ".svelte-kit",
                "paths": {
                    "base": "/base",
                    "assets": "https://cdn.example.com"
                },
                "env": {
                    "dir": ".envdir"
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");

    let plan = build_preview_server_plan(&cwd, &config.kit, true, "preview-mode", "\"123\"");

    assert_eq!(plan.protocol, "https");
    assert_eq!(plan.base, "/base");
    assert_eq!(plan.assets, "/_svelte_kit_assets");
    assert_eq!(
        plan.output_server_dir,
        Utf8PathBuf::from(".svelte-kit/output/server")
    );
    assert_eq!(
        plan.output_client_dir,
        Utf8PathBuf::from(".svelte-kit/output/client")
    );
    assert_eq!(
        plan.prerendered_dependencies_dir,
        Utf8PathBuf::from(".svelte-kit/output/prerendered/dependencies")
    );
    assert_eq!(
        plan.prerendered_pages_dir,
        Utf8PathBuf::from(".svelte-kit/output/prerendered/pages")
    );
    assert_eq!(plan.env_dir, Utf8PathBuf::from(".envdir"));
    assert_eq!(plan.mode, "preview-mode");
    assert_eq!(plan.etag, "\"123\"");
    assert_eq!(
        plan.immutable_cache_control,
        "public,max-age=31536000,immutable"
    );
    assert!(plan.base_middleware_name.is_some());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
