use svelte_kit::{
    render_prerender_redirect_html, serialize_missing_ids_jsonl, service_worker_prerender_paths,
};

#[test]
fn renders_base_path_redirect_html_like_upstream_prerender() {
    let html = render_prerender_redirect_html("/path-base/dynamic/foo");
    assert_eq!(
        html,
        "<script>location.href=\"/path-base/dynamic/foo\";</script><meta http-equiv=\"refresh\" content=\"0;url=/path-base/dynamic/foo\">"
    );
}

#[test]
fn escapes_redirect_characters_like_upstream_prerender() {
    let html = render_prerender_redirect_html("https://example.com/</script>alert(\"pwned\")");
    assert_eq!(
        html,
        "<script>location.href=\"https://example.com/\\u003C/script\\u003Ealert(\\\"pwned\\\")\";</script><meta http-equiv=\"refresh\" content=\"0;url=https://example.com/&lt;/script&gt;alert(&quot;pwned&quot;)\">"
    );
}

#[test]
fn strips_base_from_service_worker_prerender_paths() {
    let paths = service_worker_prerender_paths(
        "/path-base",
        &[
            "/path-base/dynamic/foo".to_string(),
            "/path-base/assets".to_string(),
        ],
    );

    assert_eq!(paths, vec!["base + \"/dynamic/foo\"", "base + \"/assets\""]);
}

#[test]
fn serializes_missing_ids_as_jsonl() {
    assert_eq!(
        serialize_missing_ids_jsonl(&["missing-id"]),
        "\"missing-id\","
    );
}
