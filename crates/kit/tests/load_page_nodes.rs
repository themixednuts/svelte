use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::Utf8PathBuf;
use svelte_kit::{KitManifest, ManifestConfig, load_page_nodes};

fn temp_dir(label: &str) -> Utf8PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("svelte-kit-{label}-{nanos}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    Utf8PathBuf::from_path_buf(dir).expect("utf8 temp dir")
}

fn write_file(path: &camino::Utf8Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, contents).expect("write file");
}

#[test]
fn loads_page_nodes_in_layout_then_leaf_order() {
    let cwd = temp_dir("load-page-nodes");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("blog").join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .and_then(|route| route.page.as_ref())
        .expect("blog page route");

    let nodes = load_page_nodes(route, &manifest);
    assert_eq!(nodes.len(), 3);
    assert_eq!(
        nodes[0]
            .and_then(|node| node.component.as_deref())
            .map(|path| path.as_str()),
        Some("src/routes/+layout.svelte")
    );
    assert_eq!(
        nodes[1]
            .and_then(|node| node.component.as_deref())
            .map(|path| path.as_str()),
        Some("src/routes/blog/+layout.svelte")
    );
    assert_eq!(
        nodes[2]
            .and_then(|node| node.component.as_deref())
            .map(|path| path.as_str()),
        Some("src/routes/blog/+page.svelte")
    );
}
