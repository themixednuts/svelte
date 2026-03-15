use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use svelte_kit::{Error, PeerImportError, resolve_peer_dependency};

fn temp_root(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("svelte-kit-{name}-{nanos}"))
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent directories should be created");
    }
    fs::write(path, contents).expect("file should be written");
}

#[test]
fn resolves_peer_dependency_relative_to_cwd() {
    let root = temp_root("peer-import");
    let cwd = root.join("apps/demo");
    let package_dir = root.join("node_modules/@scope/pkg");
    write(
        &package_dir.join("package.json"),
        r#"{
            "exports": {
                ".": {
                    "import": "./dist/index.js"
                },
                "./feature": {
                    "default": "./dist/feature.js"
                }
            }
        }"#,
    );

    let resolved = resolve_peer_dependency(&cwd, "@scope/pkg/feature")
        .expect("peer dependency should resolve");
    assert_eq!(resolved, package_dir.join("dist/feature.js"));

    fs::remove_dir_all(root).expect("temp tree should be removed");
}

#[test]
fn errors_when_peer_dependency_is_missing() {
    let root = temp_root("peer-import-missing");
    let cwd = root.join("apps/demo");
    fs::create_dir_all(&cwd).expect("cwd should be created");

    let error =
        resolve_peer_dependency(&cwd, "missing").expect_err("missing dependency should error");
    assert!(matches!(
        error,
        Error::PeerImport(PeerImportError::UnresolvedDependency { ref package_name })
            if package_name == "missing"
    ));
    assert!(
        error
            .to_string()
            .contains("Could not resolve peer dependency")
    );

    fs::remove_dir_all(root).expect("temp tree should be removed");
}
