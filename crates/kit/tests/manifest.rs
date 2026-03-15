use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8PathBuf;
use serde_json::{Map, json};
use svelte_kit::{
    Error, KitManifest, ManifestConfig, ManifestError, discover_assets, discover_hooks,
    discover_matchers, discover_routes,
};

fn repo_root() -> Utf8PathBuf {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    for candidate in manifest_dir.ancestors() {
        if candidate
            .join("kit/packages/kit/src/core/sync/create_manifest_data/test/samples")
            .is_dir()
        {
            return candidate.to_path_buf();
        }
    }

    panic!("failed to locate repository root with kit fixtures");
}

fn fixture_base() -> Utf8PathBuf {
    repo_root().join("kit/packages/kit/src/core/sync/create_manifest_data/test")
}

fn symlink_survived_git() -> bool {
    fs::symlink_metadata(
        fixture_base()
            .join("samples")
            .join("symlinks")
            .join("routes")
            .join("foo"),
    )
    .map(|metadata| metadata.file_type().is_symlink())
    .unwrap_or(false)
}

fn discover(sample: &str) -> Vec<svelte_kit::DiscoveredRoute> {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join(sample);
    let config = ManifestConfig::new(routes_dir, cwd);
    discover_routes(&config).expect("discover routes")
}

fn route<'a>(
    routes: &'a [svelte_kit::DiscoveredRoute],
    id: &str,
) -> &'a svelte_kit::DiscoveredRoute {
    routes
        .iter()
        .find(|route| route.id == id)
        .unwrap_or_else(|| panic!("missing route {id}"))
}

fn temp_dir(label: &str) -> Utf8PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let dir = repo_root()
        .join("tmp")
        .join(format!("svelte-kit-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

fn manifest_node<'a>(
    nodes: &'a [svelte_kit::ManifestNode],
    path: &str,
) -> &'a svelte_kit::ManifestNode {
    nodes
        .iter()
        .find(|node| {
            node.component.as_ref().map(|p| p.as_str()) == Some(path)
                || node.universal.as_ref().map(|p| p.as_str()) == Some(path)
                || node.server.as_ref().map(|p| p.as_str()) == Some(path)
        })
        .unwrap_or_else(|| panic!("missing node {path}"))
}

#[test]
fn discovers_param_matchers() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("basic");
    let config = ManifestConfig::new(routes_dir, cwd);
    let matchers = discover_matchers(&config).expect("discover matchers");

    assert_eq!(
        matchers.get("foo").map(|path| path.as_str()),
        Some("params/foo.js")
    );
    assert_eq!(
        matchers.get("bar").map(|path| path.as_str()),
        Some("params/bar.js")
    );
}

#[test]
fn discovers_static_assets() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("basic");
    let config = ManifestConfig::new(routes_dir, cwd);
    let assets = discover_assets(&config).expect("discover assets");

    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0].file.as_str(), "bar/baz.txt");
    assert_eq!(assets[0].size, 14);
    assert_eq!(assets[0].type_.as_deref(), Some("text/plain"));
    assert_eq!(assets[1].file.as_str(), "foo.txt");
    assert_eq!(assets[1].size, 9);
    assert_eq!(assets[1].type_.as_deref(), Some("text/plain"));
}

#[test]
fn builds_manifest_from_routes_assets_and_matchers() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("basic");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");

    assert_eq!(manifest.routes.len(), 6);
    assert_eq!(manifest.assets.len(), 2);
    assert_eq!(manifest.matchers.len(), 2);
    assert_eq!(manifest.nodes.len(), 6);
    assert!(manifest.matchers.contains_key("foo"));
    assert_eq!(manifest.assets[0].file.as_str(), "bar/baz.txt");
}

#[test]
fn builds_manifest_from_symlinked_routes() {
    if !symlink_survived_git() {
        return;
    }

    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("symlinks").join("routes");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");

    let nodes = manifest
        .nodes
        .iter()
        .map(|node| {
            (
                format!("{:?}", node.kind),
                node.component
                    .as_ref()
                    .map(|path| path.as_str().to_string()),
                node.universal
                    .as_ref()
                    .map(|path| path.as_str().to_string()),
                node.server.as_ref().map(|path| path.as_str().to_string()),
            )
        })
        .collect::<Vec<_>>();
    let routes = manifest
        .manifest_routes
        .iter()
        .map(|route| {
            let page = route
                .page
                .as_ref()
                .map(|page| (page.layouts.clone(), page.errors.clone(), page.leaf));
            (route.id.as_str(), page)
        })
        .collect::<Vec<_>>();

    assert_eq!(
        nodes,
        vec![
            (
                "Layout".to_string(),
                Some("layout.svelte".to_string()),
                None,
                None
            ),
            (
                "Error".to_string(),
                Some("error.svelte".to_string()),
                None,
                None
            ),
            (
                "Page".to_string(),
                Some("samples/symlinks/routes/+page.svelte".to_string()),
                None,
                None,
            ),
            (
                "Page".to_string(),
                Some("samples/symlinks/routes/foo/+page.svelte".to_string()),
                None,
                None,
            ),
        ]
    );
    assert_eq!(
        routes,
        vec![
            ("/", Some((vec![Some(0)], vec![Some(1)], 2))),
            ("/foo", Some((vec![Some(0)], vec![Some(1)], 3))),
        ]
    );
}

#[test]
fn builds_manifest_nodes_from_explicit_route_files() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("page-without-svelte-file");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");

    let nodes = manifest
        .nodes
        .iter()
        .map(|node| {
            (
                format!("{:?}", node.kind),
                node.component
                    .as_ref()
                    .map(|path| path.as_str().to_string()),
                node.universal
                    .as_ref()
                    .map(|path| path.as_str().to_string()),
                node.server.as_ref().map(|path| path.as_str().to_string()),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        nodes,
        vec![
            (
                "Layout".to_string(),
                Some("layout.svelte".to_string()),
                None,
                None,
            ),
            (
                "Error".to_string(),
                Some("error.svelte".to_string()),
                None,
                None,
            ),
            (
                "Error".to_string(),
                Some("samples/page-without-svelte-file/error/+error.svelte".to_string()),
                None,
                None,
            ),
            (
                "Layout".to_string(),
                Some("samples/page-without-svelte-file/layout/+layout.svelte".to_string()),
                None,
                None,
            ),
            (
                "Layout".to_string(),
                Some("layout.svelte".to_string()),
                Some("samples/page-without-svelte-file/layout/exists/+layout.js".to_string()),
                None,
            ),
            (
                "Page".to_string(),
                Some("samples/page-without-svelte-file/+page.svelte".to_string()),
                None,
                None,
            ),
            (
                "Page".to_string(),
                None,
                Some("samples/page-without-svelte-file/error/[...path]/+page.js".to_string()),
                None,
            ),
            (
                "Page".to_string(),
                Some("samples/page-without-svelte-file/layout/exists/+page.svelte".to_string()),
                None,
                None,
            ),
            (
                "Page".to_string(),
                None,
                None,
                Some(
                    "samples/page-without-svelte-file/layout/redirect/+page.server.js".to_string()
                ),
            ),
        ]
    );
}

#[test]
fn builds_manifest_with_missing_routes_dir_using_fallback_nodes() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("basic").join("routes");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");

    let nodes = manifest
        .nodes
        .iter()
        .map(|node| {
            (
                format!("{:?}", node.kind),
                node.component
                    .as_ref()
                    .map(|path| path.as_str().to_string()),
                node.universal
                    .as_ref()
                    .map(|path| path.as_str().to_string()),
                node.server.as_ref().map(|path| path.as_str().to_string()),
            )
        })
        .collect::<Vec<_>>();
    let routes = manifest
        .manifest_routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        nodes,
        vec![
            (
                "Layout".to_string(),
                Some("layout.svelte".to_string()),
                None,
                None
            ),
            (
                "Error".to_string(),
                Some("error.svelte".to_string()),
                None,
                None
            ),
        ]
    );
    assert_eq!(routes, vec!["/"]);
    assert!(manifest.manifest_routes[0].page.is_none());
}

#[test]
fn includes_parent_ids_on_named_layout_page_nodes() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("named-layouts");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");

    let reset_page = manifest_node(
        &manifest.nodes,
        "samples/named-layouts/b/c/c2/+page@.svelte",
    );
    let special_page = manifest_node(
        &manifest.nodes,
        "samples/named-layouts/b/d/(special)/(extraspecial)/d3/+page@(special).svelte",
    );

    assert_eq!(reset_page.parent_id.as_deref(), Some(""));
    assert_eq!(special_page.parent_id.as_deref(), Some("(special)"));
}

#[test]
fn includes_parent_ids_on_named_layout_nodes() {
    let cwd = repo_root();
    let routes_dir = temp_dir("layout-parent-ids-routes");
    let params_dir = temp_dir("layout-parent-ids-params");

    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("a").join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("a").join("b").join("+layout@.svelte"),
        "<slot />",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let layout_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("a")
        .join("b")
        .join("+layout@.svelte")
        .as_str()
        .replace('\\', "/");
    let layout = manifest_node(&manifest.nodes, &layout_path);

    assert_eq!(layout.parent_id.as_deref(), Some(""));

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn inherits_page_options_through_layout_nodes() {
    let cwd = repo_root();
    let routes_dir = temp_dir("node-page-options-routes");
    let params_dir = temp_dir("node-page-options-params");

    write_file(&routes_dir.join("+layout.js"), "export const ssr = false;");
    write_file(
        &routes_dir.join("dashboard").join("+layout.server.js"),
        "export const prerender = true;",
    );
    write_file(
        &routes_dir
            .join("dashboard")
            .join("reports")
            .join("+page.js"),
        "export const csr = true;",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let root_layout_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("+layout.js")
        .as_str()
        .replace('\\', "/");
    let dashboard_layout_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("dashboard")
        .join("+layout.server.js")
        .as_str()
        .replace('\\', "/");
    let reports_page_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("dashboard")
        .join("reports")
        .join("+page.js")
        .as_str()
        .replace('\\', "/");

    let root_layout = manifest_node(&manifest.nodes, &root_layout_path);
    let dashboard_layout = manifest_node(&manifest.nodes, &dashboard_layout_path);
    let reports_page = manifest_node(&manifest.nodes, &reports_page_path);

    assert_eq!(
        root_layout
            .page_options
            .as_ref()
            .and_then(|options| options.get("ssr")),
        Some(&serde_json::json!(false))
    );
    assert_eq!(
        dashboard_layout
            .page_options
            .as_ref()
            .and_then(|options| options.get("ssr")),
        Some(&serde_json::json!(false))
    );
    assert_eq!(
        dashboard_layout
            .page_options
            .as_ref()
            .and_then(|options| options.get("prerender")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        reports_page
            .page_options
            .as_ref()
            .and_then(|options| options.get("ssr")),
        Some(&serde_json::json!(false))
    );
    assert_eq!(
        reports_page
            .page_options
            .as_ref()
            .and_then(|options| options.get("prerender")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        reports_page
            .page_options
            .as_ref()
            .and_then(|options| options.get("csr")),
        Some(&serde_json::json!(true))
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn merges_config_page_options_across_layout_chain() {
    let cwd = repo_root();
    let routes_dir = temp_dir("node-config-page-options-routes");
    let params_dir = temp_dir("node-config-page-options-params");

    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("+layout.server.js"),
        "export const config = { root: true, shared: 'root' };",
    );
    write_file(
        &routes_dir.join("dashboard").join("+layout.svelte"),
        "<slot />",
    );
    write_file(
        &routes_dir.join("dashboard").join("+layout.js"),
        "export const config = { shared: 'dashboard', dashboard: true };",
    );
    write_file(
        &routes_dir
            .join("dashboard")
            .join("reports")
            .join("+page.svelte"),
        "<h1>reports</h1>",
    );
    write_file(
        &routes_dir
            .join("dashboard")
            .join("reports")
            .join("+page.js"),
        "export const config = { page: 'universal' };",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let page_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("dashboard")
        .join("reports")
        .join("+page.js")
        .as_str()
        .replace('\\', "/");
    let page = manifest_node(&manifest.nodes, &page_path);

    assert_eq!(
        page.page_options
            .as_ref()
            .and_then(|options| options.get("config")),
        Some(&serde_json::Value::Object(Map::from_iter([
            ("dashboard".to_string(), json!(true)),
            ("page".to_string(), json!("universal")),
            ("root".to_string(), json!(true)),
            ("shared".to_string(), json!("dashboard")),
        ])))
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn propagates_dynamic_layout_page_options_to_descendants() {
    let cwd = repo_root();
    let routes_dir = temp_dir("dynamic-node-page-options-routes");
    let params_dir = temp_dir("dynamic-node-page-options-params");

    write_file(
        &routes_dir.join("+layout.js"),
        "export const ssr = process.env.SSR;",
    );
    write_file(
        &routes_dir.join("dashboard").join("+page.js"),
        "export const csr = true;",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let root_layout_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("+layout.js")
        .as_str()
        .replace('\\', "/");
    let dashboard_page_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("dashboard")
        .join("+page.js")
        .as_str()
        .replace('\\', "/");

    let root_layout = manifest_node(&manifest.nodes, &root_layout_path);
    let dashboard_page = manifest_node(&manifest.nodes, &dashboard_page_path);

    assert!(root_layout.page_options.is_none());
    assert!(dashboard_page.page_options.is_none());

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn respects_named_layout_parents_for_layout_node_page_options() {
    let cwd = repo_root();
    let routes_dir = temp_dir("named-layout-node-page-options-routes");
    let params_dir = temp_dir("named-layout-node-page-options-params");

    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("+layout.js"), "export const ssr = false;");
    write_file(&routes_dir.join("a").join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("a").join("+layout.js"),
        "export const prerender = true;",
    );
    write_file(
        &routes_dir.join("a").join("b").join("+layout@.svelte"),
        "<slot />",
    );
    write_file(
        &routes_dir.join("a").join("b").join("+layout.js"),
        "export const csr = true;",
    );
    write_file(
        &routes_dir
            .join("a")
            .join("b")
            .join("c")
            .join("+page.svelte"),
        "<h1>page</h1>",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let layout_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("a")
        .join("b")
        .join("+layout.js")
        .as_str()
        .replace('\\', "/");

    let layout = manifest_node(&manifest.nodes, &layout_path);

    assert_eq!(
        layout
            .page_options
            .as_ref()
            .and_then(|options| options.get("ssr")),
        Some(&serde_json::json!(false))
    );
    assert_eq!(
        layout
            .page_options
            .as_ref()
            .and_then(|options| options.get("csr")),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        layout
            .page_options
            .as_ref()
            .and_then(|options| options.get("prerender")),
        None
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_missing_named_layout_references_for_layout_nodes() {
    let cwd = repo_root();
    let routes_dir = temp_dir("missing-layout-parent-routes");
    let params_dir = temp_dir("missing-layout-parent-params");

    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("a").join("+layout@missing.svelte"),
        "<slot />",
    );
    write_file(
        &routes_dir.join("a").join("b").join("+page.svelte"),
        "<h1>page</h1>",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error = KitManifest::discover(&config).expect_err("missing named layout should error");
    let expected_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("a")
        .join("+layout@missing.svelte")
        .as_str()
        .replace('\\', "/");

    assert_eq!(
        error.to_string(),
        format!(r#"{expected_path} references missing segment "missing""#)
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn builds_manifest_routes_from_explicit_node_indexes() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("page-without-svelte-file");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");

    let route_ids = manifest
        .manifest_routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();
    let discovered_ids = manifest
        .routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(route_ids, discovered_ids);

    let routes = manifest
        .manifest_routes
        .iter()
        .map(|route| {
            let page = route
                .page
                .as_ref()
                .map(|page| (page.layouts.clone(), page.errors.clone(), page.leaf));
            (route.id.as_str(), page)
        })
        .collect::<Vec<_>>();

    assert_eq!(
        routes,
        vec![
            ("/", Some((vec![Some(0)], vec![Some(1)], 5))),
            ("/error", None),
            (
                "/error/[...path]",
                Some((vec![Some(0), None], vec![Some(1), Some(2)], 6)),
            ),
            ("/layout", None),
            (
                "/layout/exists",
                Some((
                    vec![Some(0), Some(3), Some(4)],
                    vec![Some(1), None, None],
                    7
                )),
            ),
            (
                "/layout/redirect",
                Some((vec![Some(0), Some(3)], vec![Some(1), None], 8)),
            ),
        ]
    );
}

#[test]
fn discovers_hooks_entries() {
    let cwd = repo_root();
    let routes_dir =
        cwd.join("kit/packages/kit/src/core/sync/create_manifest_data/test/samples/basic");
    let hooks_dir = temp_dir("hooks");

    write_file(&hooks_dir.join("hooks.client.ts"), "export {};");
    write_file(
        &hooks_dir.join("hooks.server").join("index.js"),
        "export {};",
    );
    write_file(&hooks_dir.join("hooks.js"), "export {};");

    let mut config = ManifestConfig::new(routes_dir, cwd.clone());
    config.hooks_client = hooks_dir.join("hooks.client");
    config.hooks_server = hooks_dir.join("hooks.server");
    config.hooks_universal = hooks_dir.join("hooks");

    let hooks = discover_hooks(&config).expect("discover hooks");
    let expected_client = hooks_dir
        .join("hooks.client.ts")
        .strip_prefix(&cwd)
        .expect("hooks under repo root")
        .as_str()
        .replace('\\', "/");
    let expected_server = hooks_dir
        .join("hooks.server")
        .join("index.js")
        .strip_prefix(&cwd)
        .expect("hooks under repo root")
        .as_str()
        .replace('\\', "/");
    let expected_universal = hooks_dir
        .join("hooks.js")
        .strip_prefix(&cwd)
        .expect("hooks under repo root")
        .as_str()
        .replace('\\', "/");

    assert_eq!(
        hooks.client.as_ref().map(|path| path.as_str()),
        Some(expected_client.as_str())
    );
    assert_eq!(
        hooks.server.as_ref().map(|path| path.as_str()),
        Some(expected_server.as_str())
    );
    assert_eq!(
        hooks.universal.as_ref().map(|path| path.as_str()),
        Some(expected_universal.as_str())
    );

    fs::remove_dir_all(hooks_dir).expect("remove temp dir");
}

#[test]
fn includes_endpoint_page_options_in_manifest_routes() {
    let cwd = repo_root();
    let routes_dir = temp_dir("endpoint-page-options-routes");
    let params_dir = temp_dir("endpoint-page-options-params");

    write_file(
        &routes_dir.join("api").join("+server.js"),
        "export const prerender = true;",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let endpoint = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/api")
        .and_then(|route| route.endpoint.as_ref())
        .expect("endpoint route");

    let expected_endpoint = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("api")
        .join("+server.js")
        .as_str()
        .replace('\\', "/");

    assert_eq!(endpoint.file.as_str(), expected_endpoint);
    assert_eq!(
        endpoint
            .page_options
            .as_ref()
            .and_then(|options| options.get("prerender")),
        Some(&serde_json::json!(true))
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn finds_matching_manifest_route_and_decodes_params() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("basic");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");

    let matched = manifest
        .find_matching_route("/blog/hello%20world", |_, _| true)
        .expect("matching route");

    assert_eq!(matched.route.id, "/blog/[slug]");
    assert_eq!(
        matched.params,
        std::collections::BTreeMap::from([("slug".to_string(), "hello world".to_string())])
    );
}

#[test]
fn finds_matching_manifest_route_with_matcher_fallback() {
    let cwd = repo_root();
    let routes_dir = temp_dir("manifest-route-match-routes");
    let params_dir = temp_dir("manifest-route-match-params");

    write_file(
        &routes_dir
            .join("blog")
            .join("[slug=word]")
            .join("+page.svelte"),
        "<h1>matched</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>fallback</h1>",
    );
    write_file(
        &params_dir.join("word.js"),
        "export function match(param) { return /^\\\\w+$/.test(param); }",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let matcher = |matcher: &str, value: &str| {
        matcher == "word" && value.chars().all(|ch| ch == '_' || ch.is_alphanumeric())
    };

    let matched = manifest
        .find_matching_route("/blog/hello", matcher)
        .expect("matched route");
    assert_eq!(matched.route.id, "/blog/[slug=word]");

    let fallback = manifest
        .find_matching_route("/blog/hello-world", matcher)
        .expect("fallback route");
    assert_eq!(fallback.route.id, "/blog/[slug]");

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn builds_client_routes_with_server_data_flags() {
    let cwd = repo_root();
    let routes_dir = temp_dir("client-routes-routes");
    let params_dir = temp_dir("client-routes-params");

    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("+layout.server.js"),
        "export const load = () => ({ root: true });",
    );
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir
            .join("blog")
            .join("[slug]")
            .join("+page.server.js"),
        "export const load = () => ({ post: true });",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let manifest = KitManifest::discover(&config).expect("build manifest");
    let routes = manifest.build_client_routes();
    let route = routes
        .iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("client route");

    assert_eq!(route.errors.len(), route.layouts.len());
    assert_eq!(
        route.layouts,
        vec![Some(svelte_kit::ClientLayoutRef {
            uses_server_data: true,
            node: 0,
        })]
    );
    assert_eq!(
        route.leaf,
        svelte_kit::ClientLeafRef {
            uses_server_data: true,
            node: 2,
        }
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn client_routes_execute_matchers_without_decoding() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("basic");
    let config = ManifestConfig::new(routes_dir, cwd);
    let manifest = KitManifest::discover(&config).expect("build manifest");
    let routes = manifest.build_client_routes();
    let route = routes
        .iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("client route");

    assert_eq!(
        route.exec("/blog/hello%20world", |_, _| true),
        Some(std::collections::BTreeMap::from([(
            "slug".to_string(),
            "hello%20world".to_string(),
        )]))
    );
}

#[test]
fn rejects_param_matchers_with_bad_names() {
    let cwd = repo_root();
    let routes_dir =
        cwd.join("kit/packages/kit/src/core/sync/create_manifest_data/test/samples/basic");
    let params_dir = temp_dir("bad-matchers");
    fs::write(params_dir.join("foo.js"), "").expect("write matcher");
    fs::write(params_dir.join("boo-galoo.js"), "").expect("write matcher");

    let mut config = ManifestConfig::new(routes_dir, cwd);
    config.params_dir = params_dir.clone();

    let error = discover_matchers(&config).expect_err("bad matcher name should error");
    assert!(error
        .to_string()
        .contains(r#"Matcher names can only have underscores and alphanumeric characters — "boo-galoo.js" is invalid"#));

    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_duplicate_param_matchers() {
    let cwd = repo_root();
    let routes_dir =
        cwd.join("kit/packages/kit/src/core/sync/create_manifest_data/test/samples/basic");
    let params_dir = temp_dir("duplicate-matchers");
    fs::write(params_dir.join("foo.js"), "").expect("write matcher");
    fs::write(params_dir.join("foo.ts"), "").expect("write matcher");

    let mut config = ManifestConfig::new(routes_dir, cwd);
    config.params_dir = params_dir.clone();
    config.module_extensions = vec![".js".to_string(), ".ts".to_string()];

    let error = discover_matchers(&config).expect_err("duplicate matcher name should error");
    assert!(
        error.to_string().contains("Duplicate matchers:"),
        "{}",
        error
    );

    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_routes_that_reference_missing_matchers() {
    let cwd = repo_root();
    let routes_dir = temp_dir("missing-matcher-routes");
    let params_dir = temp_dir("missing-matcher-params");
    write_file(
        &routes_dir.join("[id=foo]").join("+page.svelte"),
        "<h1>hello</h1>",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error = discover_routes(&config).expect_err("missing matcher should error");
    assert_eq!(
        error.to_string(),
        "No matcher found for parameter 'foo' in route /[id=foo]"
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn ignores_hidden_route_directories_except_well_known() {
    let cwd = repo_root();
    let routes_dir = temp_dir("hidden-route-dirs");
    let params_dir = temp_dir("hidden-route-params");

    write_file(
        &routes_dir.join("_private").join("+server.js"),
        "export const GET = () => {};",
    );
    write_file(
        &routes_dir.join(".secret").join("+server.js"),
        "export const GET = () => {};",
    );
    write_file(
        &routes_dir
            .join(".well-known")
            .join("dnt-policy.txt")
            .join("+server.js"),
        "export const GET = () => {};",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let routes = discover_routes(&config).expect("discover routes");
    let endpoints = routes
        .iter()
        .filter_map(|route| route.endpoint.as_ref().map(|path| path.as_str()))
        .collect::<Vec<_>>();
    let expected_endpoint = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join(".well-known")
        .join("dnt-policy.txt")
        .join("+server.js")
        .as_str()
        .replace('\\', "/");

    assert_eq!(endpoints, vec![expected_endpoint.as_str()]);

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_plus_prefixed_route_directories() {
    let cwd = repo_root();
    let routes_dir = temp_dir("plus-prefixed-dir");
    let params_dir = temp_dir("plus-prefixed-dir-params");

    write_file(
        &routes_dir.join("+private").join("+page.svelte"),
        "<h1>hello</h1>",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error = discover_routes(&config).expect_err("plus-prefixed dir should error");
    let expected_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("+private")
        .as_str()
        .replace('\\', "/");

    assert_eq!(
        error.to_string(),
        format!("Files and directories prefixed with + are reserved (saw {expected_path})")
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_empty_routes_directories() {
    let cwd = repo_root();
    let routes_dir = temp_dir("empty-routes");
    let params_dir = temp_dir("empty-routes-params");

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error = discover_routes(&config).expect_err("empty routes dir should error");
    assert!(matches!(
        error,
        Error::Manifest(ManifestError::NoRoutesFound)
    ));
    assert_eq!(
        error.to_string(),
        "No routes found. If you are using a custom src/routes directory, make sure it is specified in your Svelte config file"
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_missing_routes_directories() {
    let cwd = repo_root();
    let routes_dir = temp_dir("missing-routes");
    let params_dir = temp_dir("missing-routes-params");
    fs::remove_dir_all(&routes_dir).expect("remove temp dir");

    let mut config = ManifestConfig::new(routes_dir, cwd);
    config.params_dir = params_dir.clone();

    let error = discover_routes(&config).expect_err("missing routes dir should error");
    assert!(matches!(
        error,
        Error::Manifest(ManifestError::NoRoutesFound)
    ));
    assert_eq!(
        error.to_string(),
        "No routes found. If you are using a custom src/routes directory, make sure it is specified in your Svelte config file"
    );

    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_uppercase_route_escape_sequences() {
    let cwd = repo_root();
    let routes_dir = temp_dir("uppercase-route-escape");
    let params_dir = temp_dir("uppercase-route-escape-params");

    write_file(
        &routes_dir.join("[x+3F]").join("+page.svelte"),
        "<h1>hello</h1>",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error = discover_routes(&config).expect_err("uppercase escape should error");

    assert_eq!(
        error.to_string(),
        "Character escape sequence in /[x+3F] must be lowercase"
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_invalid_route_escape_sequences() {
    let cwd = repo_root();
    let routes_dir = temp_dir("invalid-route-escape");
    let params_dir = temp_dir("invalid-route-escape-params");

    write_file(
        &routes_dir.join("[x+zz]").join("+page.svelte"),
        "<h1>hello</h1>",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error = discover_routes(&config).expect_err("invalid escape should error");

    assert_eq!(
        error.to_string(),
        "Invalid character escape sequence in /[x+zz]"
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_invalid_route_escape_lengths() {
    let cwd = repo_root();
    let hex_routes_dir = temp_dir("invalid-hex-route-escape");
    let hex_params_dir = temp_dir("invalid-hex-route-escape-params");
    let unicode_routes_dir = temp_dir("invalid-unicode-route-escape");
    let unicode_params_dir = temp_dir("invalid-unicode-route-escape-params");

    write_file(
        &hex_routes_dir.join("[x+123]").join("+page.svelte"),
        "<h1>hello</h1>",
    );
    write_file(
        &unicode_routes_dir.join("[u+123]").join("+page.svelte"),
        "<h1>hello</h1>",
    );

    let mut hex_config = ManifestConfig::new(hex_routes_dir.clone(), cwd.clone());
    hex_config.params_dir = hex_params_dir.clone();

    let hex_error = discover_routes(&hex_config).expect_err("invalid hex escape should error");

    assert_eq!(
        hex_error.to_string(),
        "Hexadecimal escape sequence in /[x+123] must be two characters"
    );

    let mut unicode_config = ManifestConfig::new(unicode_routes_dir.clone(), cwd);
    unicode_config.params_dir = unicode_params_dir.clone();

    let unicode_error =
        discover_routes(&unicode_config).expect_err("invalid unicode escape should error");

    assert_eq!(
        unicode_error.to_string(),
        "Unicode escape sequence in /[u+123] must be between four and six characters"
    );

    fs::remove_dir_all(hex_routes_dir).expect("remove temp dir");
    fs::remove_dir_all(hex_params_dir).expect("remove temp dir");
    fs::remove_dir_all(unicode_routes_dir).expect("remove temp dir");
    fs::remove_dir_all(unicode_params_dir).expect("remove temp dir");
}

#[test]
fn rejects_hash_characters_in_route_segments() {
    let cwd = repo_root();
    let routes_dir = temp_dir("hash-route-segment");
    let params_dir = temp_dir("hash-route-segment-params");

    write_file(
        &routes_dir.join("foo#bar").join("+page.svelte"),
        "<h1>hello</h1>",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error = discover_routes(&config).expect_err("hash route segment should error");

    assert_eq!(
        error.to_string(),
        "Route /foo#bar should be renamed to /foo[x+23]bar"
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn discovers_basic_routes_and_files() {
    let routes = discover("basic");
    let ids = routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec![
            "/",
            "/about",
            "/blog.json",
            "/blog",
            "/blog/[slug].json",
            "/blog/[slug]",
        ]
    );

    assert_eq!(
        route(&routes, "/")
            .page
            .as_ref()
            .and_then(|page| page.component.as_ref())
            .map(|path| path.as_str()),
        Some("samples/basic/+page.svelte")
    );
    assert_eq!(
        route(&routes, "/blog/[slug].json")
            .endpoint
            .as_ref()
            .map(|path| path.as_str()),
        Some("samples/basic/blog/[slug].json/+server.ts")
    );
    assert!(
        route(&routes, "/blog/[slug]")
            .pattern
            .is_match("/blog/hello-world")
    );
}

#[test]
fn parses_optional_and_rest_routes() {
    let optional = discover("optional");
    let prefix = route(&optional, "/prefix[[suffix]]");
    assert!(prefix.pattern.is_match("/prefix"));
    assert!(prefix.pattern.is_match("/prefixvalue"));

    let nested = route(&optional, "/nested/[[optional]]/sub");
    assert!(nested.pattern.is_match("/nested/sub"));
    assert!(nested.pattern.is_match("/nested/value/sub"));

    let rest = discover("rest-prefix-suffix");
    let rest_route = route(&rest, "/prefix-[...rest]");
    assert!(rest_route.pattern.is_match("/prefix-anything/goes"));

    let rest_endpoint = route(&rest, "/[...rest].json");
    assert!(rest_endpoint.pattern.is_match("/foo/bar.json"));
}

#[test]
fn parses_multi_slug_segments() {
    let routes = discover("multiple-slugs");
    let route = route(&routes, "/[file].[ext]");

    assert_eq!(route.params.len(), 2);
    assert_eq!(route.params[0].name, "file");
    assert_eq!(route.params[1].name, "ext");
    assert!(route.pattern.is_match("/hello.txt"));
}

#[test]
fn supports_multi_slug_endpoints() {
    let routes = discover("multiple-slugs");

    assert_eq!(
        route(&routes, "/[file].[ext]")
            .endpoint
            .as_ref()
            .map(|path| path.as_str()),
        Some("samples/multiple-slugs/[file].[ext]/+server.js")
    );
}

#[test]
fn rejects_unseparated_dynamic_params() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("invalid-params");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("invalid params should error");

    assert_eq!(
        error.to_string(),
        "Invalid route /[foo][bar] — parameters must be separated"
    );
}

#[test]
fn ignores_lockfile_like_names() {
    let routes = discover("lockfiles");
    let ids = routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["/", "/foo"]);
    assert_eq!(
        route(&routes, "/foo")
            .endpoint
            .as_ref()
            .map(|path| path.as_str()),
        Some("samples/lockfiles/foo/+server.js")
    );
}

#[test]
fn rejects_conflicting_group_routes() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("conflicting-groups");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("conflicting grouped routes should error");
    let message = error.to_string();

    assert!(message.contains(r#"The "/(x)/a" and "/(y)/a" routes conflict with each other"#));
}

#[test]
fn includes_inherited_layouts_for_page_routes() {
    let routes = discover("basic-layout");
    let page = route(&routes, "/foo").page.as_ref().expect("page route");

    let layouts = page
        .layouts
        .iter()
        .map(|layout| {
            layout
                .as_ref()
                .and_then(|layout| layout.component.as_ref().map(|path| path.as_str()))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        layouts,
        vec![
            Some("samples/basic-layout/+layout.svelte"),
            Some("samples/basic-layout/foo/+layout.svelte"),
        ]
    );
}

#[test]
fn supports_named_layout_resolution_for_pages() {
    let routes = discover("named-layouts");
    let page = route(&routes, "/b/d/(special)/(extraspecial)/d3")
        .page
        .as_ref()
        .expect("page route");

    let layouts = page
        .layouts
        .iter()
        .map(|layout| {
            layout
                .as_ref()
                .and_then(|layout| layout.component.as_ref().map(|path| path.as_str()))
        })
        .collect::<Vec<_>>();

    assert_eq!(
        layouts,
        vec![
            Some("samples/named-layouts/+layout.svelte"),
            Some("samples/named-layouts/b/d/(special)/+layout.svelte"),
        ]
    );
}

#[test]
fn rejects_missing_named_layout_references() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("named-layout-missing");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("missing named layout should error");
    let message = error.to_string();
    assert!(matches!(
        error,
        Error::Manifest(ManifestError::MissingNamedLayoutSegment { .. })
    ));

    assert_eq!(
        message,
        r#"samples/named-layout-missing/+page@missing.svelte references missing segment "missing""#
    );
}

#[test]
fn rejects_named_layout_references_in_non_svelte_files() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("invalid-named-layout-reference");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error =
        discover_routes(&config).expect_err("non-svelte named layout reference should error");
    let message = error.to_string();

    assert_eq!(
        message,
        "Only Svelte files can reference named layouts. Remove '@' from +page@.js (at samples/invalid-named-layout-reference/x/+page@.js)"
    );
}

#[test]
fn rejects_named_layout_references_in_non_svelte_layout_files() {
    let cwd = repo_root();
    let routes_dir = temp_dir("invalid-layout-named-layout-reference");
    let params_dir = temp_dir("invalid-layout-named-layout-reference-params");

    write_file(
        &routes_dir.join("x").join("+layout@.js"),
        "export const csr = true;",
    );

    let mut config = ManifestConfig::new(routes_dir.clone(), cwd);
    config.params_dir = params_dir.clone();

    let error =
        discover_routes(&config).expect_err("non-svelte named layout reference should error");

    let expected_path = routes_dir
        .strip_prefix(repo_root())
        .expect("temp routes under repo root")
        .join("x")
        .join("+layout@.js")
        .as_str()
        .replace('\\', "/");

    assert_eq!(
        error.to_string(),
        format!(
            "Only Svelte files can reference named layouts. Remove '@' from +layout@.js (at {expected_path})"
        )
    );

    fs::remove_dir_all(routes_dir).expect("remove temp dir");
    fs::remove_dir_all(params_dir).expect("remove temp dir");
}

#[test]
fn rejects_conflicting_page_modules() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("conflicting-ts-js-handlers-page");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("duplicate page modules should error");
    let message = error.to_string();

    assert_eq!(
        message,
        "Multiple universal page module files found in samples/conflicting-ts-js-handlers-page/ : +page.js and +page.ts"
    );
}

#[test]
fn rejects_conflicting_layout_components() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("multiple-layouts");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("duplicate layout components should error");
    let message = error.to_string();

    assert_eq!(
        message,
        "Multiple layout component files found in samples/multiple-layouts/ : +layout.svelte and +layout@.svelte"
    );
}

#[test]
fn rejects_conflicting_page_components() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("multiple-pages");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("duplicate page components should error");
    let message = error.to_string();

    assert_eq!(
        message,
        "Multiple page component files found in samples/multiple-pages/ : +page.svelte and +page@.svelte"
    );
}

#[test]
fn rejects_conflicting_layout_modules() {
    let cwd = fixture_base();
    let routes_dir = cwd
        .join("samples")
        .join("conflicting-ts-js-handlers-layout");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("duplicate layout modules should error");
    let message = error.to_string();

    assert_eq!(
        message,
        "Multiple server layout module files found in samples/conflicting-ts-js-handlers-layout/ : +layout.server.js and +layout.server.ts"
    );
}

#[test]
fn rejects_conflicting_endpoints() {
    let cwd = fixture_base();
    let routes_dir = cwd
        .join("samples")
        .join("conflicting-ts-js-handlers-server");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("duplicate endpoints should error");
    let message = error.to_string();

    assert_eq!(
        message,
        "Multiple endpoint files found in samples/conflicting-ts-js-handlers-server/ : +server.js and +server.ts"
    );
}

#[test]
fn rejects_conflicting_param_routes() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("conflicting-params");
    let config = ManifestConfig::new(routes_dir, cwd);

    let error = discover_routes(&config).expect_err("conflicting param routes should error");
    let message = error.to_string();

    assert_eq!(
        message,
        r#"The "/[slug1]" and "/[slug2]" routes conflict with each other"#
    );
}

#[test]
fn supports_custom_component_extensions() {
    let cwd = fixture_base();
    let routes_dir = cwd.join("samples").join("custom-extension");
    let mut config = ManifestConfig::new(routes_dir, cwd);
    config.component_extensions = vec![
        ".jazz".to_string(),
        ".beebop".to_string(),
        ".funk".to_string(),
        ".svelte".to_string(),
    ];

    let routes = discover_routes(&config).expect("discover routes");
    let ids = routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec![
            "/",
            "/about",
            "/blog.json",
            "/blog",
            "/blog/[slug].json",
            "/blog/[slug]",
        ]
    );

    assert_eq!(
        route(&routes, "/")
            .page
            .as_ref()
            .and_then(|page| page.component.as_ref())
            .map(|path| path.as_str()),
        Some("samples/custom-extension/+page.funk")
    );
    assert_eq!(
        route(&routes, "/about")
            .page
            .as_ref()
            .and_then(|page| page.component.as_ref())
            .map(|path| path.as_str()),
        Some("samples/custom-extension/about/+page.jazz")
    );
    assert_eq!(
        route(&routes, "/blog/[slug]")
            .page
            .as_ref()
            .and_then(|page| page.component.as_ref())
            .map(|path| path.as_str()),
        Some("samples/custom-extension/blog/[slug]/+page.beebop")
    );
}

#[test]
fn supports_pages_without_svelte_components() {
    let routes = discover("page-without-svelte-file");

    let module_only = route(&routes, "/error/[...path]")
        .page
        .as_ref()
        .expect("module-only page route");
    assert_eq!(module_only.component, None);
    assert_eq!(
        module_only.universal.as_ref().map(|path| path.as_str()),
        Some("samples/page-without-svelte-file/error/[...path]/+page.js")
    );
    assert_eq!(
        module_only
            .errors
            .iter()
            .map(|path| path.as_ref().map(|path| path.as_str()))
            .collect::<Vec<_>>(),
        vec![Some("samples/page-without-svelte-file/error/+error.svelte")]
    );

    let server_only = route(&routes, "/layout/redirect")
        .page
        .as_ref()
        .expect("server-only page route");
    assert_eq!(server_only.component, None);
    assert_eq!(server_only.universal, None);
    assert_eq!(
        server_only.server.as_ref().map(|path| path.as_str()),
        Some("samples/page-without-svelte-file/layout/redirect/+page.server.js")
    );
    let layouts = server_only
        .layouts
        .iter()
        .map(|layout| {
            layout
                .as_ref()
                .and_then(|layout| layout.component.as_ref().map(|path| path.as_str()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        layouts,
        vec![Some(
            "samples/page-without-svelte-file/layout/+layout.svelte"
        )]
    );
}

#[test]
fn supports_encoded_static_route_segments() {
    let routes = discover("encoding");

    let quote = route(&routes, "/[x+22]");
    assert!(quote.pattern.is_match("/\""));

    let hash = route(&routes, "/[x+23]");
    assert!(hash.pattern.is_match("/%23"));

    let question_mark = route(&routes, "/[x+3f]");
    assert!(question_mark.pattern.is_match("/%3F"));
    assert!(question_mark.pattern.is_match("/%3f"));
}

#[test]
fn ignores_files_and_directories_with_leading_underscores() {
    let routes = discover("hidden-underscore");
    let endpoints = routes
        .iter()
        .filter_map(|route| route.endpoint.as_ref().map(|path| path.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(
        endpoints,
        vec!["samples/hidden-underscore/e/f/g/h/+server.js"]
    );
}

#[test]
fn ignores_files_and_directories_with_leading_dots_except_well_known() {
    let routes = discover("hidden-dot");
    let endpoints = routes
        .iter()
        .filter_map(|route| route.endpoint.as_ref().map(|path| path.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(
        endpoints,
        vec!["samples/hidden-dot/.well-known/dnt-policy.txt/+server.js"]
    );
}

#[test]
fn preserves_nested_layout_and_error_slots() {
    let routes = discover("nested-errors");
    let page = route(&routes, "/foo/bar/baz")
        .page
        .as_ref()
        .expect("page route");

    let layouts = page
        .layouts
        .iter()
        .map(|layout| {
            layout
                .as_ref()
                .and_then(|layout| layout.component.as_ref().map(|path| path.as_str()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        layouts,
        vec![
            Some("samples/nested-errors/foo/+layout.svelte"),
            None,
            Some("samples/nested-errors/foo/bar/baz/+layout.svelte"),
        ]
    );

    let errors = page
        .errors
        .iter()
        .map(|path| path.as_ref().map(|path| path.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        errors,
        vec![
            None,
            Some("samples/nested-errors/foo/bar/+error.svelte"),
            Some("samples/nested-errors/foo/bar/baz/+error.svelte"),
        ]
    );
}

#[test]
fn supports_nested_optional_routes() {
    let routes = discover("nested-optionals");
    let ids = routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["/", "/[[a]]/[[b]]", "/[[a]]"]);

    let nested = route(&routes, "/[[a]]/[[b]]");
    assert!(nested.pattern.is_match("/"));
    assert!(nested.pattern.is_match("/one"));
    assert!(nested.pattern.is_match("/one/two"));
}

#[test]
fn supports_group_preceding_optional_parameters() {
    let routes = discover("optional-group");
    let ids = routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["/", "/[[optional]]/(group)", "/[[optional]]"]);

    let grouped = route(&routes, "/[[optional]]/(group)");
    assert!(grouped.pattern.is_match("/"));
    assert!(grouped.pattern.is_match("/value"));
}

#[test]
fn sorts_rest_routes_correctly() {
    let routes = discover("rest");
    let ids = routes
        .iter()
        .map(|route| route.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["/", "/a", "/a/[...rest]", "/b", "/b/[...rest]"]);

    let a_rest = route(&routes, "/a/[...rest]");
    assert!(a_rest.pattern.is_match("/a"));
    assert!(a_rest.pattern.is_match("/a/one/two"));
    assert_eq!(
        a_rest
            .page
            .as_ref()
            .and_then(|page| page.server.as_ref())
            .map(|path| path.as_str()),
        Some("samples/rest/a/[...rest]/+page.server.js")
    );
}
