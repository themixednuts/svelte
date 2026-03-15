use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{DevMiddlewareStep, build_vite_dev_server_plan, validate_config};

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
        .join(format!("svelte-kit-vite-dev-server-{label}-{unique}"));
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
fn builds_vite_dev_server_watch_and_middleware_plan() {
    let cwd = temp_dir("plan");
    write_file(&cwd.join("src").join("hooks.client.ts"), "export {};");
    write_file(&cwd.join("src").join("service-worker.ts"), "export {};");

    let config = validate_config(
        &serde_json::json!({
            "kit": {
                "outDir": ".svelte-kit",
                "paths": {
                    "base": "/base"
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");

    let plan = build_vite_dev_server_plan(&config.kit, false, true);

    assert_eq!(plan.debounce_ms, 100);
    assert_eq!(
        plan.static_middleware_names,
        vec![
            "viteServeStaticMiddleware".to_string(),
            "viteServePublicMiddleware".to_string()
        ]
    );
    assert!(
        plan.manifest_watch_roots
            .iter()
            .any(|path| path.ends_with("src/routes"))
    );
    assert!(
        plan.manifest_watch_roots
            .iter()
            .any(|path| path.ends_with("src/params"))
    );
    assert!(
        plan.manifest_watch_roots
            .iter()
            .any(|path| path.ends_with("src/hooks.client"))
    );
    assert_eq!(plan.service_worker_route, "/base/service-worker.js");
    assert_eq!(
        plan.service_worker_entry.as_deref(),
        Some("src/service-worker.ts")
    );
    assert_eq!(plan.assets_mount_path, "/base");
    assert_eq!(
        plan.steps,
        vec![
            DevMiddlewareStep::ServeAssets {
                scope: "/base".to_string(),
                allow_origin: "*".to_string(),
            },
            DevMiddlewareStep::RemoveViteStaticMiddlewares {
                names: vec![
                    "viteServeStaticMiddleware".to_string(),
                    "viteServePublicMiddleware".to_string()
                ],
            },
            DevMiddlewareStep::ServeServiceWorker {
                route: "/base/service-worker.js".to_string(),
                entry: Some("src/service-worker.ts".to_string()),
            },
            DevMiddlewareStep::SsrRequestHandler,
        ]
    );
    assert!(plan.restart_on_config_change);
    assert!(plan.full_reload_on_non_index_app_template_change);

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
