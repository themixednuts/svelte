use svelte_kit::{
    Csp, RuntimeCspConfig, RuntimeCspDirectives, RuntimeCspMode, RuntimeCspOptions,
    fallback_page_filename, prerender_output_filename, public_asset_output_path,
    should_prerender_linked_server_route,
};

fn directives(
    entries: impl IntoIterator<Item = (&'static str, Vec<&'static str>)>,
) -> RuntimeCspDirectives {
    RuntimeCspDirectives::new(entries)
}

#[test]
fn prerenders_grouped_root_and_nested_pages_with_trailing_slash_always() {
    assert_eq!(
        prerender_output_filename("/path-base", "/path-base/", true),
        "index.html"
    );
    assert_eq!(
        prerender_output_filename("/path-base", "/path-base/nested/", true),
        "nested/index.html"
    );
}

#[test]
fn adds_csp_meta_for_static_prerender_pages() {
    let csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Auto,
            directives: directives([("script-src", vec!["self"])]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: true,
            dev: false,
        },
    )
    .expect("csp config");

    assert_eq!(
        csp.csp_provider.get_meta().as_deref(),
        Some("<meta http-equiv=\"content-security-policy\" content=\"script-src 'self'\">")
    );
}

#[test]
fn includes_known_hydratable_script_hashes_in_prerendered_meta_csp() {
    let mut csp = Csp::new(
        RuntimeCspConfig {
            mode: RuntimeCspMode::Auto,
            directives: directives([("script-src", vec!["self"])]),
            report_only: directives([]),
        },
        RuntimeCspOptions {
            prerender: true,
            dev: false,
        },
    )
    .expect("csp config");

    csp.add_script_hashes(&["sha256-xWnzKGZbZBWKfvJVEFtrpB/s9zyyMDyQZt49JX2PAJQ="]);

    let meta = csp.csp_provider.get_meta().expect("meta csp");
    assert!(meta.contains("'sha256-xWnzKGZbZBWKfvJVEFtrpB/s9zyyMDyQZt49JX2PAJQ='"));
}

#[test]
fn keeps_public_assets_out_of_app_dir() {
    assert_eq!(public_asset_output_path("_app", "robots.txt"), "robots.txt");
}

#[test]
fn keeps_configured_fallback_filename() {
    assert_eq!(fallback_page_filename("200.html"), "200.html");
}

#[test]
fn linked_server_routes_are_not_prerendered_without_explicit_prerender_flag() {
    assert!(!should_prerender_linked_server_route(None));
    assert!(!should_prerender_linked_server_route(Some(false)));
    assert!(should_prerender_linked_server_route(Some(true)));
}
