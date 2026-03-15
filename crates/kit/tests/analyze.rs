use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use std::collections::BTreeMap;

use svelte_kit::{
    AdapterFeatures, AnalyzeError, BuildData, BuildManifestChunk, BuilderPrerenderOption, Error,
    RemoteExport, RemoteFunctionInfo, RemoteFunctionKind, analyze_remote_metadata,
    analyze_server_metadata, analyze_server_metadata_with_features, load_project,
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
        .join(format!("svelte-kit-analyze-{label}-{unique}"));
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
fn analyzes_server_metadata_from_route_modules() {
    let cwd = temp_dir("metadata");
    write_app_template(&cwd);
    write_file(
        &cwd.join("src").join("routes").join("+layout.ts"),
        "export const prerender = true;\nexport const config = { runtime: 'edge' };\n",
    );
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
            .join("+page.ts"),
        "export function load() {}\nexport const entries = [{ slug: 'a' }];\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug]")
            .join("+page.server.ts"),
        "export const actions = { default: async () => ({ ok: true }) };\n",
    );
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("blog")
            .join("[slug]")
            .join("+server.ts"),
        "export function GET() { return new Response('ok'); }\nexport const config = { runtime: 'edge' };\nexport const prerender = false;\n",
    );

    let project = load_project(&cwd).expect("load project");
    let metadata = analyze_server_metadata(&project.cwd, &project.config, &project.manifest)
        .expect("analyze metadata");

    let route = metadata
        .routes
        .routes
        .get("/blog/[slug]")
        .expect("route metadata");
    let blog_leaf = metadata.nodes.last().expect("leaf metadata");

    assert_eq!(route.page.methods, vec!["GET", "POST"]);
    assert_eq!(route.api.methods, vec!["GET"]);
    assert_eq!(route.entries, Some(vec!["/blog/a".to_string()]));
    assert_eq!(route.config["runtime"], "edge");
    assert_eq!(route.prerender, Some(BuilderPrerenderOption::True));
    assert!(blog_leaf.has_universal_load);
    assert!(!blog_leaf.has_server_load);

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rejects_invalid_page_server_exports_during_analysis() {
    let cwd = temp_dir("invalid-export");
    write_app_template(&cwd);
    write_file(
        &cwd.join("src").join("routes").join("+page.svelte"),
        "<h1>home</h1>\n",
    );
    write_file(
        &cwd.join("src").join("routes").join("+page.server.ts"),
        "export function GET() { return new Response('nope'); }\n",
    );

    let project = load_project(&cwd).expect("load project");
    let error = analyze_server_metadata(&project.cwd, &project.config, &project.manifest)
        .expect_err("invalid export should fail");

    assert!(error.to_string().contains("Invalid export 'GET'"));
    assert!(error.to_string().contains("+server.ts"));

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rejects_prerendered_mutative_endpoints() {
    let cwd = temp_dir("mutative-endpoint");
    write_app_template(&cwd);
    write_file(
        &cwd.join("src")
            .join("routes")
            .join("api")
            .join("+server.ts"),
        "export const prerender = true;\nexport function POST() { return new Response('nope'); }\n",
    );

    let project = load_project(&cwd).expect("load project");
    let error = analyze_server_metadata(&project.cwd, &project.config, &project.manifest)
        .expect_err("mutative prerender endpoint should fail");

    assert!(
        error
            .to_string()
            .contains("Cannot prerender a +server file with POST, PATCH, PUT, or DELETE (/api)")
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn rejects_routes_using_unsupported_read_features() {
    let cwd = temp_dir("unsupported-read");
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
        "export async function load() { return {}; }\n",
    );
    write_file(
        &cwd.join("src").join("hooks.server.ts"),
        "export const handle = async ({ event, resolve }) => resolve(event);\n",
    );

    let project = load_project(&cwd).expect("load project");
    let server_manifest = BTreeMap::from([
        (
            "src/routes/blog/[slug]/+page.server.ts".to_string(),
            BuildManifestChunk {
                file: "nodes/blog-page-server.js".to_string(),
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
    let tracked_features = BTreeMap::from([(
        "nodes/blog-page-server.js".to_string(),
        vec!["$app/server:read".to_string()],
    )]);
    let adapter = AdapterFeatures {
        name: "adapter-static".to_string(),
        supports_read: false,
    };

    let error = analyze_server_metadata_with_features(
        &project.cwd,
        &project.config,
        &project.manifest,
        Some(&build_data),
        Some(&tracked_features),
        Some(&adapter),
    )
    .expect_err("unsupported read should fail");

    assert_eq!(
        error.to_string(),
        "Cannot use `read` from `$app/server` in /blog/[slug] when using adapter-static. Please ensure that your adapter is up to date and supports this feature."
    );

    fs::remove_dir_all(&cwd).expect("remove temp dir");
}

#[test]
fn analyzes_remote_export_metadata() {
    let remotes = BTreeMap::from([
        (
            "hash-a".to_string(),
            BTreeMap::from([
                (
                    "query_users".to_string(),
                    RemoteExport::new(Some(RemoteFunctionInfo::new(RemoteFunctionKind::Query))),
                ),
                (
                    "prerender_posts".to_string(),
                    RemoteExport::new(Some(
                        RemoteFunctionInfo::new(RemoteFunctionKind::Prerender).with_dynamic(true),
                    )),
                ),
            ]),
        ),
        (
            "hash-b".to_string(),
            BTreeMap::from([(
                "static_prerender".to_string(),
                RemoteExport::new(Some(RemoteFunctionInfo::new(RemoteFunctionKind::Prerender))),
            )]),
        ),
    ]);

    let analyzed = analyze_remote_metadata(&remotes).expect("remote analysis");

    assert_eq!(
        analyzed["hash-a"]["query_users"],
        svelte_kit::AnalyzedRemoteExport {
            kind: RemoteFunctionKind::Query,
            dynamic: true,
        }
    );
    assert_eq!(
        analyzed["hash-a"]["prerender_posts"],
        svelte_kit::AnalyzedRemoteExport {
            kind: RemoteFunctionKind::Prerender,
            dynamic: true,
        }
    );
    assert_eq!(
        analyzed["hash-b"]["static_prerender"],
        svelte_kit::AnalyzedRemoteExport {
            kind: RemoteFunctionKind::Prerender,
            dynamic: false,
        }
    );
}

#[test]
fn rejects_invalid_remote_exports_during_analysis() {
    let remotes = BTreeMap::from([(
        "hash-a".to_string(),
        BTreeMap::from([("broken".to_string(), RemoteExport::new(None))]),
    )]);

    let error = analyze_remote_metadata(&remotes).expect_err("invalid remote export");
    assert!(matches!(
        error,
        Error::Analyze(AnalyzeError::InvalidRemoteExport { .. })
    ));
    assert!(
        error
            .to_string()
            .contains("all exports from this file must be remote functions")
    );
}
