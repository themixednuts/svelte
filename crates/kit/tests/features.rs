use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use serde_json::json;
use svelte_kit::{
    AdapterFeatures, BuildData, BuildManifestChunk, Error, FeatureError, check_feature,
    list_route_features, load_project,
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
        .join(format!("svelte-kit-features-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

fn write_app_template(cwd: &Utf8PathBuf) {
    write_file(
        &cwd.join("src").join("app.html"),
        "<!doctype html><html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
}

#[test]
fn checks_read_feature_support_against_adapter_capabilities() {
    let adapter = AdapterFeatures {
        name: "adapter-static".to_string(),
        supports_read: false,
    };

    let error = check_feature("/", &json!({}), "$app/server:read", Some(&adapter))
        .expect_err("unsupported read should fail");

    assert_eq!(
        error.to_string(),
        "Cannot use `read` from `$app/server` in / when using adapter-static. Please ensure that your adapter is up to date and supports this feature."
    );
    assert!(matches!(
        error,
        Error::Feature(FeatureError::UnsupportedRead { route_id, adapter_name })
        if route_id == "/" && adapter_name == "adapter-static"
    ));
}

#[test]
fn lists_route_features_from_server_chunk_graph() {
    let cwd = temp_dir("list");
    write_app_template(&cwd);
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug]")
            .join("+page.svelte"),
        "<h1>blog</h1>\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug]")
            .join("+page.server.ts"),
        "export function load() { return {}; }\n",
    );
    write_file(
        &cwd.join("src").join("hooks.server.ts"),
        "export const handle = async ({ event, resolve }) => resolve(event);\n",
    );

    let project = load_project(&cwd).expect("load project");
    let route = project
        .manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("blog route");

    let server_manifest = BTreeMap::from([
        (
            "src/routes/blog/[slug]/+page.server.ts".to_string(),
            BuildManifestChunk {
                file: "nodes/blog-page-server.js".to_string(),
                imports: vec!["src/hooks.server.ts".to_string()],
                ..Default::default()
            },
        ),
        (
            "src/hooks.server.ts".to_string(),
            BuildManifestChunk {
                file: "server/hooks.js".to_string(),
                ..Default::default()
            },
        ),
    ]);
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &project.manifest,
        out_dir: project.cwd.join(".svelte-kit"),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };
    let tracked_features = BTreeMap::from([
        (
            "nodes/blog-page-server.js".to_string(),
            vec!["$app/server:read".to_string()],
        ),
        ("server/hooks.js".to_string(), vec!["hooks".to_string()]),
    ]);

    let features = list_route_features(route, &build_data, &tracked_features);

    assert_eq!(
        features,
        vec!["$app/server:read".to_string(), "hooks".to_string()]
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
