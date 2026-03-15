use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use serde_json::{Value, json};
use svelte_kit::{
    Asset, BuildData, BuildManifestChunk, BuilderAdapterEntry, BuilderFacade,
    BuilderPrerenderOption, BuilderPrerendered, BuilderRouteApi, BuilderRouteFilter,
    BuilderRoutePage, BuilderServerMetadata, BuilderServerMetadataRoute, Hooks, KitManifest,
    ManifestEndpoint, ManifestNode, ManifestNodeKind, ManifestRoute, ManifestRoutePage,
    RemoteChunk, RouteParam, validate_config,
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
        .join(format!("svelte-kit-builder-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

fn chunk(file: &str, assets: &[&str]) -> BuildManifestChunk {
    BuildManifestChunk {
        file: file.to_string(),
        imports: Vec::new(),
        dynamic_imports: Vec::new(),
        css: Vec::new(),
        assets: assets.iter().map(|value| (*value).to_string()).collect(),
    }
}

fn sample_manifest() -> KitManifest {
    KitManifest {
        assets: vec![Asset {
            file: Utf8PathBuf::from("favicon.png"),
            size: 4,
            type_: Some("image/png".to_string()),
        }],
        hooks: Hooks {
            client: None,
            server: Some(Utf8PathBuf::from("src/hooks.server.ts")),
            universal: None,
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
                id: "/blog/[slug]".to_string(),
                pattern: regex::Regex::new("^/blog/([^/]+?)/?$").expect("regex"),
                params: vec![RouteParam {
                    name: "slug".to_string(),
                    matcher: None,
                    optional: false,
                    rest: false,
                    chained: false,
                }],
                page: Some(ManifestRoutePage {
                    layouts: vec![Some(0)],
                    errors: vec![Some(1)],
                    leaf: 3,
                }),
                endpoint: None,
            },
            ManifestRoute {
                id: "/blog/[slug].json".to_string(),
                pattern: regex::Regex::new("^/blog/([^/]+?)\\.json/?$").expect("regex"),
                params: vec![RouteParam {
                    name: "slug".to_string(),
                    matcher: None,
                    optional: false,
                    rest: false,
                    chained: false,
                }],
                page: None,
                endpoint: Some(ManifestEndpoint {
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
            ManifestNode {
                kind: ManifestNodeKind::Page,
                component: Some(Utf8PathBuf::from("src/routes/blog/[slug]/+page.svelte")),
                universal: Some(Utf8PathBuf::from("src/routes/blog/[slug]/+page.ts")),
                server: Some(Utf8PathBuf::from("src/routes/blog/[slug]/+page.server.ts")),
                parent_id: None,
                universal_page_options: None,
                server_page_options: None,
                page_options: None,
            },
        ],
        routes: Vec::new(),
    }
}

#[test]
fn builds_route_definitions_from_metadata() {
    let manifest = sample_manifest();
    let metadata = BuilderServerMetadata {
        routes: BTreeMap::from([
            (
                "/blog/[slug]".to_string(),
                BuilderServerMetadataRoute {
                    config: json!({ "runtime": "edge" }),
                    api: BuilderRouteApi::default(),
                    page: BuilderRoutePage {
                        methods: vec!["GET".to_string(), "POST".to_string()],
                    },
                    methods: vec!["GET".to_string(), "POST".to_string()],
                    prerender: Some(BuilderPrerenderOption::Auto),
                    entries: None,
                },
            ),
            (
                "/blog/[slug].json".to_string(),
                BuilderServerMetadataRoute {
                    config: Value::Null,
                    api: BuilderRouteApi {
                        methods: vec!["GET".to_string()],
                    },
                    page: BuilderRoutePage::default(),
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::False),
                    entries: None,
                },
            ),
        ]),
    };
    let prerendered = BuilderPrerendered {
        paths: vec!["/".to_string()],
        ..Default::default()
    };
    let config = validate_config(&json!({}), &repo_root()).expect("validated config");
    let out_dir = repo_root().join(".svelte-kit");
    let server_manifest = BTreeMap::new();
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir,
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };
    let facade = BuilderFacade::new(
        &config,
        &build_data,
        &metadata,
        &manifest.manifest_routes,
        &prerendered,
        &[],
    );

    let routes = facade.routes();
    let blog = routes
        .iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("blog route");
    let root = routes
        .iter()
        .find(|route| route.id == "/")
        .expect("root route");

    assert_eq!(blog.page.methods, vec!["GET", "POST"]);
    assert_eq!(blog.methods, vec!["GET", "POST"]);
    assert_eq!(blog.prerender, BuilderPrerenderOption::Auto);
    assert_eq!(blog.config, json!({ "runtime": "edge" }));
    assert_eq!(blog.segments.len(), 2);
    assert_eq!(blog.segments[0].content, "blog");
    assert_eq!(blog.segments[1].content, "[slug]");
    assert!(blog.segments[1].dynamic);
    assert_eq!(root.prerender, BuilderPrerenderOption::True);
}

#[test]
fn groups_routes_and_includes_json_endpoints() {
    let manifest = sample_manifest();
    let metadata = BuilderServerMetadata {
        routes: BTreeMap::from([(
            "/blog/[slug]".to_string(),
            BuilderServerMetadataRoute {
                config: Value::Null,
                api: BuilderRouteApi::default(),
                page: BuilderRoutePage {
                    methods: vec!["GET".to_string()],
                },
                methods: vec!["GET".to_string()],
                prerender: Some(BuilderPrerenderOption::False),
                entries: None,
            },
        )]),
    };
    let prerendered = BuilderPrerendered::default();
    let config = validate_config(&json!({}), &repo_root()).expect("validated config");
    let server_manifest = BTreeMap::new();
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: repo_root().join(".svelte-kit"),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };
    let facade = BuilderFacade::new(
        &config,
        &build_data,
        &metadata,
        &manifest.manifest_routes,
        &prerendered,
        &[],
    );

    let grouped = facade.group_routes("/blog/[slug]", |route| route.id.ends_with(".json"));
    let ids = grouped
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["/blog/[slug]", "/blog/[slug].json"]);
}

#[test]
fn builder_generates_manifest_and_finds_assets() {
    let cwd = temp_dir("facade");
    let out_dir = cwd.join(".svelte-kit");
    write_file(&out_dir.join("server").join("blog-page.json"), b"page");
    write_file(&out_dir.join("server").join("blog-api.bin"), b"api");
    write_file(&out_dir.join("server").join("hooks-server.dat"), b"hook");
    write_file(
        &out_dir.join("server").join("layout.png"),
        &[137, 80, 78, 71],
    );

    let manifest = sample_manifest();
    let server_manifest = BTreeMap::from([
        (
            "src/routes/+layout.ts".to_string(),
            chunk("chunks/layout.js", &["layout.png"]),
        ),
        (
            "src/routes/blog/[slug]/+page.ts".to_string(),
            chunk("chunks/blog-page.js", &["blog-page.json"]),
        ),
        (
            "src/routes/blog/[slug]/+page.server.ts".to_string(),
            chunk("chunks/blog-page-server.js", &[]),
        ),
        (
            "src/routes/blog/[slug]/+server.ts".to_string(),
            chunk("chunks/blog-api.js", &["blog-api.bin"]),
        ),
        (
            "src/hooks.server.ts".to_string(),
            chunk("chunks/hooks.js", &["hooks-server.dat"]),
        ),
    ]);
    let metadata = BuilderServerMetadata {
        routes: BTreeMap::from([
            (
                "/".to_string(),
                BuilderServerMetadataRoute {
                    config: Value::Null,
                    api: BuilderRouteApi::default(),
                    page: BuilderRoutePage {
                        methods: vec!["GET".to_string()],
                    },
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::True),
                    entries: None,
                },
            ),
            (
                "/blog/[slug]".to_string(),
                BuilderServerMetadataRoute {
                    config: json!({ "runtime": "node" }),
                    api: BuilderRouteApi::default(),
                    page: BuilderRoutePage {
                        methods: vec!["GET".to_string()],
                    },
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::False),
                    entries: None,
                },
            ),
            (
                "/blog/[slug].json".to_string(),
                BuilderServerMetadataRoute {
                    config: Value::Null,
                    api: BuilderRouteApi {
                        methods: vec!["GET".to_string()],
                    },
                    page: BuilderRoutePage::default(),
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::False),
                    entries: None,
                },
            ),
        ]),
    };
    let prerendered = BuilderPrerendered {
        paths: vec!["/".to_string()],
        ..Default::default()
    };
    let config = validate_config(&json!({}), &cwd).expect("validated config");
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
    let remotes = [RemoteChunk {
        hash: "remote".to_string(),
    }];
    let facade = BuilderFacade::new(
        &config,
        &build_data,
        &metadata,
        &manifest.manifest_routes,
        &prerendered,
        &remotes,
    );

    let selected = facade
        .routes()
        .iter()
        .filter(|route| route.id.starts_with("/blog/"))
        .cloned()
        .collect::<Vec<_>>();
    let assets = facade.find_server_assets(&selected);
    let manifest_contents = facade
        .generate_manifest("..", None)
        .expect("generate facade manifest");

    assert_eq!(
        assets,
        vec![
            "blog-api.bin",
            "blog-page.json",
            "hooks-server.dat",
            "layout.png"
        ]
    );
    assert!(manifest_contents.contains("\"/blog/[slug]\""));
    assert!(!manifest_contents.contains("new RegExp(\"^/$\")"));
    assert!(manifest_contents.contains("prerendered_routes: new Set([\"/\"])"));
    assert!(manifest_contents.contains("__memo(() => import(\"../chunks/blog-api.js\"))"));
    assert!(
        manifest_contents
            .contains("\"remote\": __memo(() => import(\"../chunks/remote-remote.js\"))")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn builder_writes_client_server_and_prerendered_outputs() {
    let cwd = temp_dir("write-outputs");
    let out_dir = cwd.join(".svelte-kit");
    write_file(
        &out_dir.join("output").join("client").join("app.js"),
        b"console.log('client');",
    );
    write_file(
        &out_dir
            .join("output")
            .join("client")
            .join(".vite")
            .join("manifest.json"),
        b"{}",
    );
    write_file(
        &out_dir.join("output").join("server").join("index.js"),
        b"export const answer = 42;",
    );
    write_file(
        &out_dir
            .join("output")
            .join("prerendered")
            .join("pages")
            .join("index.html"),
        b"<html></html>",
    );
    write_file(
        &out_dir
            .join("output")
            .join("prerendered")
            .join("dependencies")
            .join("_app")
            .join("env.js"),
        b"export const env = {};",
    );

    let manifest = sample_manifest();
    let server_manifest = BTreeMap::new();
    let metadata = BuilderServerMetadata::default();
    let prerendered = BuilderPrerendered::default();
    let config = validate_config(&json!({}), &cwd).expect("validated config");
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: out_dir.clone(),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };
    let facade = BuilderFacade::new(
        &config,
        &build_data,
        &metadata,
        &manifest.manifest_routes,
        &prerendered,
        &[],
    );

    let client_dest = cwd.join("client-out");
    let server_dest = cwd.join("server-out");
    let prerender_dest = cwd.join("prerender-out");

    let client_files = facade.write_client(&client_dest).expect("write client");
    let server_files = facade.write_server(&server_dest).expect("write server");
    let prerender_files = facade
        .write_prerendered(&prerender_dest)
        .expect("write prerendered");

    assert_eq!(client_files, vec!["app.js"]);
    assert!(client_dest.join("app.js").is_file());
    assert!(!client_dest.join(".vite").exists());
    assert_eq!(server_files, vec!["index.js"]);
    assert!(server_dest.join("index.js").is_file());
    assert_eq!(prerender_files, vec!["index.html", "_app/env.js"]);
    assert!(prerender_dest.join("index.html").is_file());
    assert!(prerender_dest.join("_app").join("env.js").is_file());

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn builder_generates_prerender_env_module() {
    let cwd = temp_dir("env-module");
    let out_dir = cwd.join(".svelte-kit");

    let manifest = sample_manifest();
    let server_manifest = BTreeMap::new();
    let metadata = BuilderServerMetadata::default();
    let prerendered = BuilderPrerendered::default();
    let config = validate_config(&json!({}), &cwd).expect("validated config");
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: out_dir.clone(),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };
    let facade = BuilderFacade::new(
        &config,
        &build_data,
        &metadata,
        &manifest.manifest_routes,
        &prerendered,
        &[],
    );

    let dest = facade
        .generate_env_module(
            json!({
                "PUBLIC_ORIGIN": "https://example.com",
                "PUBLIC_FLAG": "enabled"
            })
            .as_object()
            .expect("public env object"),
        )
        .expect("generate env module");

    let contents = fs::read_to_string(&dest).expect("read env module");
    assert!(contents.contains("\"PUBLIC_ORIGIN\": \"https://example.com\""));
    assert!(contents.contains("\"PUBLIC_FLAG\": \"enabled\""));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn builder_create_entries_dedupes_and_uses_empty_prerendered_set() {
    let cwd = temp_dir("create-entries");
    let out_dir = cwd.join(".svelte-kit");
    write_file(&out_dir.join("server").join("blog-page.json"), b"page");
    write_file(&out_dir.join("server").join("blog-api.bin"), b"api");
    write_file(&out_dir.join("server").join("hooks-server.dat"), b"hook");
    write_file(
        &out_dir.join("server").join("layout.png"),
        &[137, 80, 78, 71],
    );

    let manifest = sample_manifest();
    let server_manifest = BTreeMap::from([
        (
            "src/routes/+layout.ts".to_string(),
            chunk("chunks/layout.js", &["layout.png"]),
        ),
        (
            "src/routes/blog/[slug]/+page.ts".to_string(),
            chunk("chunks/blog-page.js", &["blog-page.json"]),
        ),
        (
            "src/routes/blog/[slug]/+server.ts".to_string(),
            chunk("chunks/blog-api.js", &["blog-api.bin"]),
        ),
        (
            "src/hooks.server.ts".to_string(),
            chunk("chunks/hooks.js", &["hooks-server.dat"]),
        ),
    ]);
    let metadata = BuilderServerMetadata {
        routes: BTreeMap::from([
            (
                "/".to_string(),
                BuilderServerMetadataRoute {
                    config: Value::Null,
                    api: BuilderRouteApi::default(),
                    page: BuilderRoutePage {
                        methods: vec!["GET".to_string()],
                    },
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::True),
                    entries: None,
                },
            ),
            (
                "/blog/[slug]".to_string(),
                BuilderServerMetadataRoute {
                    config: Value::Null,
                    api: BuilderRouteApi::default(),
                    page: BuilderRoutePage {
                        methods: vec!["GET".to_string()],
                    },
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::False),
                    entries: None,
                },
            ),
            (
                "/blog/[slug].json".to_string(),
                BuilderServerMetadataRoute {
                    config: Value::Null,
                    api: BuilderRouteApi {
                        methods: vec!["GET".to_string()],
                    },
                    page: BuilderRoutePage::default(),
                    methods: vec!["GET".to_string()],
                    prerender: Some(BuilderPrerenderOption::False),
                    entries: None,
                },
            ),
        ]),
    };
    let prerendered = BuilderPrerendered {
        paths: vec!["/".to_string()],
        ..Default::default()
    };
    let config = validate_config(&json!({}), &cwd).expect("validated config");
    let build_data = BuildData {
        app_dir: "_app".to_string(),
        app_path: "/_app".to_string(),
        manifest_data: &manifest,
        out_dir: out_dir.clone(),
        service_worker: None,
        client: None,
        server_manifest: &server_manifest,
    };
    let facade = BuilderFacade::new(
        &config,
        &build_data,
        &metadata,
        &manifest.manifest_routes,
        &prerendered,
        &[],
    );

    let mut seen = Vec::new();
    let mut rendered = Vec::new();
    facade
        .create_entries(
            |route| BuilderAdapterEntry {
                id: if route.id.starts_with("/blog/") {
                    "blog".to_string()
                } else {
                    route.id.clone()
                },
                filter: BuilderRouteFilter::new(|candidate| candidate.id.ends_with(".json")),
            },
            |entry| {
                seen.push((
                    entry.id.clone(),
                    entry
                        .routes
                        .iter()
                        .map(|route| route.id.clone())
                        .collect::<Vec<_>>(),
                ));
                rendered.push(entry.generate_manifest("..")?);
                Ok(())
            },
        )
        .expect("create entries");

    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "blog");
    assert_eq!(seen[0].1, vec!["/blog/[slug]", "/blog/[slug].json"]);
    assert!(rendered[0].contains("\"/blog/[slug]\""));
    assert!(rendered[0].contains("prerendered_routes: new Set([])"));
    assert!(!rendered[0].contains("\"/\""));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}
