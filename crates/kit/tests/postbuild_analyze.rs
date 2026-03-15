use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use svelte_kit::{
    BuildData, BuildManifestChunk, RemoteExport, RemoteFunctionInfo, RemoteFunctionKind,
    analyze_postbuild, load_project,
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
        .join(format!("svelte-kit-postbuild-analyze-{label}-{unique}"));
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
fn analyze_postbuild_combines_route_remote_and_server_node_data() {
    let cwd = temp_dir("combined");
    write_file(
        &cwd.join("src").join("app.html"),
        "<!doctype html><html><head>%sveltekit.head%</head><body>%sveltekit.body%</body></html>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.ts"),
        "export function load() { return { message: 'ok' }; }",
    );

    let project = load_project(&cwd).expect("project");
    let server_manifest = BTreeMap::from([
        (
            "src/routes/+page.ts".to_string(),
            BuildManifestChunk {
                file: "nodes/page.js".to_string(),
                ..Default::default()
            },
        ),
        (
            "src/routes/+page.svelte".to_string(),
            BuildManifestChunk {
                file: "nodes/page.ssr.js".to_string(),
                ..Default::default()
            },
        ),
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
    ]);
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &project.manifest,
        out_dir: project.cwd.join(".svelte-kit/output"),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };
    let remotes = BTreeMap::from([(
        "remote-hash".to_string(),
        BTreeMap::from([
            (
                "query_posts".to_string(),
                RemoteExport::new(Some(RemoteFunctionInfo::new(RemoteFunctionKind::Query))),
            ),
            (
                "prerender_feed".to_string(),
                RemoteExport::new(Some(
                    RemoteFunctionInfo::new(RemoteFunctionKind::Prerender).with_dynamic(true),
                )),
            ),
        ]),
    )]);

    let analyzed = analyze_postbuild(
        &project.cwd,
        &project.config,
        &build_data,
        None,
        None,
        &remotes,
        None,
    )
    .expect("postbuild analyze");

    assert!(analyzed.metadata.routes.routes.contains_key("/"));
    assert_eq!(analyzed.remotes["remote-hash"]["query_posts"].dynamic, true);
    assert_eq!(
        analyzed.remotes["remote-hash"]["prerender_feed"].kind,
        RemoteFunctionKind::Prerender
    );
    assert_eq!(
        analyzed.server_nodes_plan.node_modules.len(),
        project.manifest.nodes.len()
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
