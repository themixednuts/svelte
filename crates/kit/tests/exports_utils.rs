use std::collections::BTreeSet;

use camino::Utf8Path;
use svelte_kit::{
    RouteModuleKind, validate_layout_exports, validate_layout_server_exports,
    validate_page_exports, validate_page_server_exports, validate_server_exports,
};

fn export_set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn check_error(result: svelte_kit::Result<()>, expected: &str) {
    let error = result.expect_err("expected export validation error");
    assert_eq!(error.to_string(), expected);
}

#[test]
fn validates_layout_exports_like_upstream() {
    validate_layout_exports(&export_set(&[
        "load",
        "prerender",
        "csr",
        "ssr",
        "trailingSlash",
        "config",
    ]))
    .expect("valid +layout.js exports");
    validate_layout_exports(&export_set(&["_unknown"])).expect("private layout export");

    check_error(
        validate_layout_exports(&export_set(&["answer"])),
        "Invalid export 'answer' (valid exports are load, prerender, csr, ssr, trailingSlash, config, or anything with a '_' prefix)",
    );
    check_error(
        validate_layout_exports_for_path(
            &export_set(&["actions"]),
            Utf8Path::new("src/routes/foo/+page.ts"),
        ),
        "Invalid export 'actions' in src/routes/foo/+page.ts ('actions' is a valid export in +page.server.ts)",
    );
    check_error(
        validate_layout_exports(&export_set(&["GET"])),
        "Invalid export 'GET' ('GET' is a valid export in +server.js)",
    );
}

#[test]
fn validates_page_exports_like_upstream() {
    validate_page_exports(&export_set(&[
        "load",
        "prerender",
        "csr",
        "ssr",
        "trailingSlash",
        "config",
        "entries",
    ]))
    .expect("valid +page.js exports");
    validate_page_exports(&export_set(&["_unknown"])).expect("private page export");

    check_error(
        validate_page_exports(&export_set(&["answer"])),
        "Invalid export 'answer' (valid exports are load, prerender, csr, ssr, trailingSlash, config, entries, or anything with a '_' prefix)",
    );
    check_error(
        validate_page_exports_for_path(
            &export_set(&["actions"]),
            Utf8Path::new("src/routes/foo/+page.ts"),
        ),
        "Invalid export 'actions' in src/routes/foo/+page.ts ('actions' is a valid export in +page.server.ts)",
    );
    check_error(
        validate_page_exports(&export_set(&["GET"])),
        "Invalid export 'GET' ('GET' is a valid export in +server.js)",
    );
}

#[test]
fn validates_layout_server_exports_like_upstream() {
    validate_layout_server_exports(&export_set(&[
        "load",
        "prerender",
        "csr",
        "ssr",
        "trailingSlash",
        "config",
    ]))
    .expect("valid +layout.server.js exports");
    validate_layout_server_exports(&export_set(&["_unknown"]))
        .expect("private layout.server export");

    check_error(
        validate_layout_server_exports(&export_set(&["answer"])),
        "Invalid export 'answer' (valid exports are load, prerender, csr, ssr, trailingSlash, config, or anything with a '_' prefix)",
    );
    check_error(
        validate_layout_exports_for_path(
            &export_set(&["actions"]),
            Utf8Path::new("src/routes/foo/+page.ts"),
        ),
        "Invalid export 'actions' in src/routes/foo/+page.ts ('actions' is a valid export in +page.server.ts)",
    );
    check_error(
        validate_layout_server_exports(&export_set(&["POST"])),
        "Invalid export 'POST' ('POST' is a valid export in +server.js)",
    );
}

#[test]
fn validates_page_server_exports_like_upstream() {
    validate_page_server_exports(&export_set(&[
        "load",
        "prerender",
        "csr",
        "ssr",
        "trailingSlash",
        "config",
        "actions",
        "entries",
    ]))
    .expect("valid +page.server.js exports");
    validate_page_server_exports(&export_set(&["_unknown"])).expect("private page.server export");

    check_error(
        validate_page_server_exports(&export_set(&["answer"])),
        "Invalid export 'answer' (valid exports are load, prerender, csr, ssr, trailingSlash, config, actions, entries, or anything with a '_' prefix)",
    );
    check_error(
        validate_page_server_exports(&export_set(&["POST"])),
        "Invalid export 'POST' ('POST' is a valid export in +server.js)",
    );
}

#[test]
fn validates_server_exports_like_upstream() {
    validate_server_exports(&export_set(&["GET"])).expect("valid +server.js exports");
    validate_server_exports(&export_set(&["_unknown"])).expect("private endpoint export");

    check_error(
        validate_server_exports(&export_set(&["answer"])),
        "Invalid export 'answer' (valid exports are GET, POST, PATCH, PUT, DELETE, OPTIONS, HEAD, fallback, prerender, trailingSlash, config, entries, or anything with a '_' prefix)",
    );
    check_error(
        validate_server_exports(&export_set(&["csr"])),
        "Invalid export 'csr' ('csr' is a valid export in +layout.js, +page.js, +layout.server.js or +page.server.js)",
    );
}

fn validate_layout_exports_for_path(
    exports: &BTreeSet<String>,
    path: &Utf8Path,
) -> svelte_kit::Result<()> {
    svelte_kit::validate_module_exports(exports, RouteModuleKind::LayoutUniversal, path)
}

fn validate_page_exports_for_path(
    exports: &BTreeSet<String>,
    path: &Utf8Path,
) -> svelte_kit::Result<()> {
    svelte_kit::validate_module_exports(exports, RouteModuleKind::PageUniversal, path)
}
