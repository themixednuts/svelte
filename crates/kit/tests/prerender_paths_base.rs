use svelte_kit::{
    prepend_base_path, prerender_output_filename, render_prerender_redirect_html,
    service_worker_prerender_paths,
};

#[test]
fn writes_root_prerendered_page_without_base_prefix() {
    assert_eq!(
        prerender_output_filename("/path-base", "/path-base", true),
        "index.html"
    );
}

#[test]
fn writes_nested_static_pages_with_index_suffix_when_path_has_trailing_slash() {
    assert_eq!(
        prerender_output_filename("/path-base", "/path-base/nested/", true),
        "nested/index.html"
    );
}

#[test]
fn writes_dynamic_pages_without_base_prefix() {
    assert_eq!(
        prerender_output_filename("/path-base", "/path-base/dynamic/foo", true),
        "dynamic/foo.html"
    );
}

#[test]
fn prepends_base_to_root_relative_redirects_and_assets() {
    assert_eq!(
        prepend_base_path("/path-base", "/dynamic/foo"),
        "/path-base/dynamic/foo"
    );
    assert_eq!(
        prepend_base_path("/path-base", "/assets/logo.svg"),
        "/path-base/assets/logo.svg"
    );
    assert_eq!(
        prepend_base_path("/path-base", "/message.csv"),
        "/path-base/message.csv"
    );
}

#[test]
fn keeps_prerender_redirect_html_aligned_with_base_prefixed_locations() {
    let html = render_prerender_redirect_html(&prepend_base_path("/path-base", "/dynamic/foo"));
    assert_eq!(
        html,
        "<script>location.href=\"/path-base/dynamic/foo\";</script><meta http-equiv=\"refresh\" content=\"0;url=/path-base/dynamic/foo\">"
    );
}

#[test]
fn strips_base_from_service_worker_prerender_inventory() {
    let paths = service_worker_prerender_paths(
        "/path-base",
        &[
            "/path-base/trailing-slash/page/".to_string(),
            "/path-base/trailing-slash/page/__data.json".to_string(),
            "/path-base/trailing-slash/standalone-endpoint.json".to_string(),
        ],
    );

    assert_eq!(
        paths,
        vec![
            "base + \"/trailing-slash/page/\"",
            "base + \"/trailing-slash/page/__data.json\"",
            "base + \"/trailing-slash/standalone-endpoint.json\"",
        ]
    );
}
