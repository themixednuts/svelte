use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use svelte_kit::{
    BuildData, BuildManifestChunk, RouterResolution, build_vite_dev_plan, load_project,
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
        .join(format!("svelte-kit-vite-dev-{label}-{unique}"));
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
fn builds_vite_dev_plan_with_generated_client_entries() {
    let cwd = temp_dir("plan");
    write_file(
        &cwd.join("src").join("app.html"),
        "<!doctype html><html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    let config = validate_config(
        &serde_json::json!({
            "kit": {
                "outDir": ".svelte-kit",
                "router": {
                    "resolution": "server"
                }
            }
        }),
        &cwd,
    )
    .expect("config should validate");
    let project = load_project(&cwd).expect("project should load");
    let server_manifest = BTreeMap::from([(
        "layout.svelte".to_string(),
        BuildManifestChunk {
            file: "nodes/layout.js".to_string(),
            ..Default::default()
        },
    )]);
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &project.manifest,
        out_dir: Utf8Path::new(".svelte-kit/output").to_path_buf(),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };

    let plan = build_vite_dev_plan(
        &config.kit,
        build_data.manifest_data,
        ".svelte-kit",
        "/@fs/src/runtime",
        &["remote-a".to_string(), "remote-b".to_string()],
    );

    assert_eq!(plan.app_dir, "_app");
    assert_eq!(plan.app_path, "_app");
    assert_eq!(plan.client_start, "/@fs/src/runtime/client/entry.js");
    assert_eq!(plan.client_app, ".svelte-kit/generated/client/app.js");
    assert_eq!(
        plan.remote_hashes,
        vec!["remote-a".to_string(), "remote-b".to_string()]
    );
    assert_eq!(plan.router_resolution, RouterResolution::Server);
    assert!(plan.client_nodes.is_some());
    assert!(plan.client_routes.is_some());
    assert!(!plan.server_nodes.is_empty());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
