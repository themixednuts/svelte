use std::collections::BTreeSet;

use camino::Utf8Path;
use svelte_kit::{Error, ExportValidationError, RouteModuleKind, validate_module_exports};

fn export_set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

#[test]
fn validates_page_server_exports() {
    validate_module_exports(
        &export_set(&["load", "actions", "entries", "_private"]),
        RouteModuleKind::PageServer,
        Utf8Path::new("src/routes/+page.server.ts"),
    )
    .expect("valid page server exports");
}

#[test]
fn rejects_invalid_exports_with_supported_file_hint() {
    let error = validate_module_exports(
        &export_set(&["GET"]),
        RouteModuleKind::PageServer,
        Utf8Path::new("src/routes/+page.server.ts"),
    )
    .expect_err("GET is invalid in +page.server");

    assert_eq!(
        error.to_string(),
        "Invalid export 'GET' in src/routes/+page.server.ts ('GET' is a valid export in +server.ts)"
    );
    assert!(matches!(
        error,
        Error::ExportValidation(ExportValidationError::InvalidExportInPath { key, path, .. })
        if key == "GET" && path == "src/routes/+page.server.ts"
    ));
}

#[test]
fn rejects_invalid_exports_with_allowed_list_hint() {
    let error = validate_module_exports(
        &export_set(&["foo"]),
        RouteModuleKind::Endpoint,
        Utf8Path::new("src/routes/+server.js"),
    )
    .expect_err("unexpected endpoint export");

    assert_eq!(
        error.to_string(),
        "Invalid export 'foo' in src/routes/+server.js (valid exports are GET, POST, PATCH, PUT, DELETE, OPTIONS, HEAD, fallback, prerender, trailingSlash, config, entries, or anything with a '_' prefix)"
    );
    assert!(matches!(
        error,
        Error::ExportValidation(ExportValidationError::InvalidExportInPath { key, path, .. })
        if key == "foo" && path == "src/routes/+server.js"
    ));
}
