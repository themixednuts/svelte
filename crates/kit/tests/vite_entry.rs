use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::{Utf8Path, Utf8PathBuf};
use svelte_kit::{
    BuildData, BuildManifestChunk, BuilderPrerenderOption, BuilderRouteApi, BuilderRoutePage,
    BuilderServerMetadata, BuilderServerMetadataRoute, ServiceWorkerBuildEntry,
    build_vite_orchestration_plan, load_project,
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
        .join(format!("svelte-kit-vite-entry-{label}-{unique}"));
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
fn builds_vite_orchestration_plan_from_existing_subplans() {
    let cwd = temp_dir("plan");
    write_file(
        &cwd.join("src").join("app.html"),
        "<!doctype html><html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    let project = load_project(&cwd).expect("project should load");
    let server_manifest = BTreeMap::from([
        (
            "layout.svelte".to_string(),
            BuildManifestChunk {
                file: "nodes/layout.ssr.js".to_string(),
                ..Default::default()
            },
        ),
        (
            "error.svelte".to_string(),
            BuildManifestChunk {
                file: "nodes/error.ssr.js".to_string(),
                ..Default::default()
            },
        ),
        (
            "src/routes/+page.svelte".to_string(),
            BuildManifestChunk {
                file: "nodes/0.js".to_string(),
                ..Default::default()
            },
        ),
    ]);
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &project.manifest,
        out_dir: project.cwd.join(".svelte-kit/output"),
        service_worker: Some("src/service-worker.js".to_string()),
        client: None,
        server_manifest: &server_manifest,
    };
    let metadata = BuilderServerMetadata {
        routes: BTreeMap::from([(
            "/".to_string(),
            BuilderServerMetadataRoute {
                page: BuilderRoutePage {
                    methods: vec!["GET".to_string()],
                },
                api: BuilderRouteApi::default(),
                methods: vec!["GET".to_string()],
                prerender: Some(BuilderPrerenderOption::True),
                entries: None,
                config: serde_json::Value::Null,
            },
        )]),
    };
    let service_worker_entries = BTreeMap::from([(
        "entry".to_string(),
        ServiceWorkerBuildEntry {
            file: "entry.js".to_string(),
            css: vec![],
            assets: vec!["logo.svg".to_string()],
        },
    )]);
    let public_env = serde_json::Map::from_iter([(
        "PUBLIC_VERSION".to_string(),
        serde_json::Value::String("v1".to_string()),
    )]);
    let env = BTreeMap::from([("PUBLIC_VERSION".to_string(), "v1".to_string())]);

    let plan = build_vite_orchestration_plan(
        &cwd,
        &project.config,
        &build_data,
        &metadata,
        ".svelte-kit/output/server/manifest.js",
        &service_worker_entries,
        &["favicon.png".to_string()],
        &["/".to_string()],
        &public_env,
        &env,
        true,
        false,
    )
    .expect("vite orchestration plan should build");

    assert_eq!(plan.out, ".svelte-kit/output");
    assert!(!plan.is_rolldown);
    assert!(plan.hash_routing);
    assert_eq!(
        plan.version_hash,
        svelte_kit::hash_values([svelte_kit::HashValue::Str(&project.config.kit.version.name)])
            .expect("version hash")
    );
    assert_eq!(
        plan.preview.server_dir,
        Utf8Path::new(".svelte-kit/output/server")
    );
    assert!(plan.service_worker.is_some());
    assert!(plan.prerender.fallback.is_some());
    assert_eq!(
        plan.postbuild.server_nodes_plan.node_modules.len(),
        project.manifest.nodes.len()
    );
    assert_eq!(plan.module_ids.service_worker, "\0virtual:service-worker");

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
