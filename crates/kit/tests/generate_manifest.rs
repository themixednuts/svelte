use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use serde_json::json;
use svelte_kit::{
    BuildData, BuildManifestChunk, Error, GenerateManifestError, Hooks, KitManifest, ManifestNode,
    ManifestNodeKind, ManifestRoute, ManifestRoutePage, RemoteChunk, find_deps, find_server_assets,
    generate_manifest,
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
        .join(format!("svelte-kit-generate-manifest-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

fn chunk(
    file: &str,
    imports: &[&str],
    dynamic_imports: &[&str],
    css: &[&str],
    assets: &[&str],
) -> BuildManifestChunk {
    BuildManifestChunk {
        file: file.to_string(),
        imports: imports.iter().map(|value| (*value).to_string()).collect(),
        dynamic_imports: dynamic_imports
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        css: css.iter().map(|value| (*value).to_string()).collect(),
        assets: assets.iter().map(|value| (*value).to_string()).collect(),
    }
}

#[test]
fn finds_transitive_manifest_dependencies() {
    let manifest = BTreeMap::from([
        (
            "entry.js".to_string(),
            chunk(
                "chunks/entry.js",
                &["shared.js"],
                &["dynamic.js"],
                &["entry.css"],
                &["entry.png"],
            ),
        ),
        (
            "shared.js".to_string(),
            chunk(
                "chunks/shared.js",
                &[],
                &[],
                &["shared.css"],
                &["shared.woff2"],
            ),
        ),
        (
            "dynamic.js".to_string(),
            chunk(
                "chunks/dynamic.js",
                &[],
                &[],
                &["dynamic.css"],
                &["dynamic.svg"],
            ),
        ),
    ]);

    let deps = find_deps(&manifest, "entry.js", false).expect("deps");
    assert_eq!(deps.file, "chunks/entry.js");
    assert_eq!(deps.imports, vec!["chunks/entry.js", "chunks/shared.js"]);
    assert_eq!(deps.stylesheets, vec!["entry.css", "shared.css"]);
    assert_eq!(deps.assets, vec!["entry.png", "shared.woff2"]);
    assert_eq!(deps.fonts, vec!["shared.woff2"]);
    assert!(deps.stylesheet_map.is_empty());
}

#[test]
fn generate_manifest_reports_missing_build_manifest_chunk() {
    let manifest_data = KitManifest {
        assets: Vec::new(),
        hooks: Hooks::default(),
        matchers: BTreeMap::new(),
        manifest_routes: vec![ManifestRoute {
            id: "/api".to_string(),
            pattern: regex::Regex::new("^/api$").expect("regex"),
            params: Vec::new(),
            page: None,
            endpoint: Some(svelte_kit::ManifestEndpoint {
                file: Utf8PathBuf::from("src/routes/api/+server.ts"),
                page_options: None,
            }),
        }],
        nodes: vec![
            ManifestNode {
                kind: ManifestNodeKind::Layout,
                component: None,
                universal: None,
                server: None,
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
            ManifestNode {
                kind: ManifestNodeKind::Error,
                component: None,
                universal: None,
                server: None,
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
        ],
        routes: Vec::new(),
    };
    let empty_server_manifest = BTreeMap::new();
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "_app".to_string(),
        manifest_data: &manifest_data,
        out_dir: Utf8PathBuf::from("build"),
        service_worker: None,
        client: None,
        server_manifest: &empty_server_manifest,
    };

    let error = generate_manifest(&build_data, &[], ".", &manifest_data.manifest_routes, &[])
        .expect_err("missing build chunk should fail");

    assert!(matches!(
        error,
        Error::GenerateManifest(GenerateManifestError::MissingBuildManifestFile { ref file })
            if file == "src/routes/api/+server.ts"
    ));
    assert_eq!(
        error.to_string(),
        "Could not find file \"src/routes/api/+server.ts\" in build manifest"
    );
}

#[test]
fn groups_dynamic_stylesheets_by_initial_importer() {
    let manifest = BTreeMap::from([
        (
            "entry.js".to_string(),
            chunk(
                "chunks/entry.js",
                &[],
                &["dynamic-a.js", "dynamic-b.js"],
                &[],
                &[],
            ),
        ),
        (
            "dynamic-a.js".to_string(),
            chunk(
                "chunks/dynamic-a.js",
                &[],
                &["dynamic-a-child.js"],
                &["dynamic-a.css"],
                &["dynamic-a.png"],
            ),
        ),
        (
            "dynamic-a-child.js".to_string(),
            chunk(
                "chunks/dynamic-a-child.js",
                &[],
                &[],
                &["dynamic-a-child.css"],
                &["dynamic-a-child.woff2"],
            ),
        ),
        (
            "dynamic-b.js".to_string(),
            chunk(
                "chunks/dynamic-b.js",
                &[],
                &[],
                &["dynamic-b.css"],
                &["dynamic-b.svg"],
            ),
        ),
    ]);

    let deps = find_deps(&manifest, "entry.js", true).expect("deps");
    assert_eq!(
        deps.stylesheet_map.get("dynamic-a.js").expect("dynamic a"),
        &svelte_kit::StylesheetMapEntry {
            css: vec!["dynamic-a.css".to_string()],
            assets: vec!["dynamic-a.png".to_string()],
        }
    );
    assert_eq!(
        deps.stylesheet_map.get("dynamic-b.js").expect("dynamic b"),
        &svelte_kit::StylesheetMapEntry {
            css: vec!["dynamic-b.css".to_string()],
            assets: vec!["dynamic-b.svg".to_string()],
        }
    );
}

#[test]
fn finds_server_assets_for_used_routes_nodes_and_hooks() {
    let manifest = KitManifest {
        assets: Vec::new(),
        hooks: Hooks {
            client: None,
            server: Some(Utf8PathBuf::from("src/hooks.server.ts")),
            universal: Some(Utf8PathBuf::from("src/hooks.ts")),
        },
        matchers: BTreeMap::new(),
        manifest_routes: vec![
            ManifestRoute {
                id: "/".to_string(),
                pattern: regex::Regex::new("^/$").expect("regex"),
                params: Vec::new(),
                page: Some(ManifestRoutePage {
                    layouts: vec![Some(0)],
                    errors: vec![Some(1)],
                    leaf: 2,
                }),
                endpoint: None,
            },
            ManifestRoute {
                id: "/api".to_string(),
                pattern: regex::Regex::new("^/api$").expect("regex"),
                params: Vec::new(),
                page: None,
                endpoint: Some(svelte_kit::ManifestEndpoint {
                    file: Utf8PathBuf::from("src/routes/api/+server.ts"),
                    page_options: None,
                }),
            },
        ],
        nodes: vec![
            ManifestNode {
                kind: ManifestNodeKind::Layout,
                component: Some(Utf8PathBuf::from("src/routes/+layout.svelte")),
                universal: Some(Utf8PathBuf::from("src/routes/+layout.ts")),
                server: Some(Utf8PathBuf::from("src/routes/+layout.server.ts")),
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
            ManifestNode {
                kind: ManifestNodeKind::Error,
                component: Some(Utf8PathBuf::from("src/routes/+error.svelte")),
                universal: None,
                server: None,
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
            ManifestNode {
                kind: ManifestNodeKind::Page,
                component: Some(Utf8PathBuf::from("src/routes/+page.svelte")),
                universal: Some(Utf8PathBuf::from("src/routes/+page.ts")),
                server: Some(Utf8PathBuf::from("src/routes/+page.server.ts")),
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
        ],
        routes: Vec::new(),
    };

    let server_manifest = BTreeMap::from([
        (
            "src/routes/+layout.ts".to_string(),
            chunk(
                "chunks/layout-universal.js",
                &[],
                &[],
                &[],
                &["layout-universal.png"],
            ),
        ),
        (
            "src/routes/+layout.server.ts".to_string(),
            chunk(
                "chunks/layout-server.js",
                &[],
                &[],
                &[],
                &["layout-server.txt"],
            ),
        ),
        (
            "src/routes/+page.ts".to_string(),
            chunk(
                "chunks/page-universal.js",
                &[],
                &[],
                &[],
                &["page-universal.css"],
            ),
        ),
        (
            "src/routes/+page.server.ts".to_string(),
            chunk(
                "chunks/page-server.js",
                &[],
                &[],
                &[],
                &["page-server.json"],
            ),
        ),
        (
            "src/routes/api/+server.ts".to_string(),
            chunk("chunks/api.js", &[], &[], &[], &["api.bin"]),
        ),
        (
            "src/hooks.server.ts".to_string(),
            chunk(
                "chunks/hooks-server.js",
                &[],
                &[],
                &[],
                &["hooks-server.dat"],
            ),
        ),
        (
            "src/hooks.ts".to_string(),
            chunk(
                "chunks/hooks-universal.js",
                &[],
                &[],
                &[],
                &["hooks-universal.dat"],
            ),
        ),
    ]);

    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: Utf8PathBuf::from(".svelte-kit"),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };

    let assets = find_server_assets(&build_data, &manifest.manifest_routes);
    assert_eq!(
        assets,
        vec![
            "api.bin",
            "hooks-server.dat",
            "hooks-universal.dat",
            "layout-server.txt",
            "layout-universal.png",
            "page-server.json",
            "page-universal.css",
        ]
    );
}

#[test]
fn generates_server_manifest_module() {
    let out_dir = temp_dir("server-manifest").join(".svelte-kit");
    write_file(&out_dir.join("server").join("api.bin"), b"api");
    write_file(&out_dir.join("server").join("hooks-server.dat"), b"hook");
    write_file(
        &out_dir.join("server").join("layout-universal.png"),
        &[137, 80, 78, 71],
    );

    let manifest = KitManifest {
        assets: vec![svelte_kit::Asset {
            file: Utf8PathBuf::from("favicon.png"),
            size: 4,
            type_: Some("image/png".to_string()),
        }],
        hooks: Hooks {
            client: None,
            server: Some(Utf8PathBuf::from("src/hooks.server.ts")),
            universal: None,
        },
        matchers: BTreeMap::from([("word".to_string(), Utf8PathBuf::from("src/params/word.ts"))]),
        manifest_routes: vec![
            ManifestRoute {
                id: "/".to_string(),
                pattern: regex::Regex::new("^/$").expect("regex"),
                params: Vec::new(),
                page: Some(ManifestRoutePage {
                    layouts: vec![Some(0)],
                    errors: vec![Some(1)],
                    leaf: 2,
                }),
                endpoint: None,
            },
            ManifestRoute {
                id: "/blog/[slug=word]".to_string(),
                pattern: regex::Regex::new("^/blog/([^/]+?)/?$").expect("regex"),
                params: vec![svelte_kit::RouteParam {
                    name: "slug".to_string(),
                    matcher: Some("word".to_string()),
                    optional: false,
                    rest: false,
                    chained: false,
                }],
                page: None,
                endpoint: Some(svelte_kit::ManifestEndpoint {
                    file: Utf8PathBuf::from("src/routes/blog/[slug]/+server.ts"),
                    page_options: None,
                }),
            },
        ],
        nodes: vec![
            ManifestNode {
                kind: ManifestNodeKind::Layout,
                component: Some(Utf8PathBuf::from("src/routes/+layout.svelte")),
                universal: Some(Utf8PathBuf::from("src/routes/+layout.ts")),
                server: None,
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
            ManifestNode {
                kind: ManifestNodeKind::Error,
                component: Some(Utf8PathBuf::from("src/routes/+error.svelte")),
                universal: None,
                server: None,
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
            ManifestNode {
                kind: ManifestNodeKind::Page,
                component: Some(Utf8PathBuf::from("src/routes/+page.svelte")),
                universal: None,
                server: None,
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
        ],
        routes: Vec::new(),
    };

    let server_manifest = BTreeMap::from([
        (
            "src/routes/+layout.ts".to_string(),
            chunk("chunks/layout.js", &[], &[], &[], &["layout-universal.png"]),
        ),
        (
            "src/routes/blog/[slug]/+server.ts".to_string(),
            chunk("chunks/api.js", &[], &[], &[], &["api.bin"]),
        ),
        (
            "src/hooks.server.ts".to_string(),
            chunk("chunks/hooks.js", &[], &[], &[], &["hooks-server.dat"]),
        ),
    ]);

    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: out_dir.clone(),
        service_worker: Some("service-worker.js".to_string()),
        client: Some(json!({
            "start": "entry.js",
            "imports": ["entry.js"],
            "stylesheets": [],
            "fonts": [],
            "uses_env_dynamic_public": false
        })),
        server_manifest: &server_manifest,
    };

    let generated = generate_manifest(
        &build_data,
        &["/".to_string()],
        "..",
        &manifest.manifest_routes,
        &[RemoteChunk {
            hash: "abc123".to_string(),
        }],
    )
    .expect("generate server manifest");

    assert!(generated.contents.contains("appDir: \"_app\""));
    assert!(generated.contents.contains("appPath: \"/_app\""));
    assert!(
        generated
            .contents
            .contains("assets: new Set([\"favicon.png\",\"service-worker.js\"])")
    );
    assert!(generated.contents.contains("client: {\"fonts\":[],\"imports\":[\"entry.js\"],\"start\":\"entry.js\",\"stylesheets\":[],\"uses_env_dynamic_public\":false}"));
    assert!(
        generated
            .contents
            .contains("__memo(() => import(\"../nodes/0.js\"))")
    );
    assert!(
        generated
            .contents
            .contains("__memo(() => import(\"../chunks/api.js\"))")
    );
    assert!(
        generated
            .contents
            .contains("\"abc123\": __memo(() => import(\"../chunks/remote-abc123.js\"))")
    );
    assert!(
        generated
            .contents
            .contains("new RegExp(\"^/blog/([^/]+?)/?$\")")
    );
    assert!(
        generated
            .contents
            .contains("const { match: word } = await import(\"../entries/matchers/word.js\")")
    );
    assert!(
        generated
            .contents
            .contains("prerendered_routes: new Set([\"/\"])")
    );
    assert!(generated.contents.contains("\".png\":\"image/png\""));
    assert!(generated.contents.contains("\"api.bin\":3"));
    assert!(generated.contents.contains("\"hooks-server.dat\":4"));
    assert!(generated.contents.contains("\"layout-universal.png\":4"));

    fs::remove_dir_all(out_dir.parent().expect("temp root")).expect("remove temp dir");
}
