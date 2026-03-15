use svelte_kit::normalize_url;

#[test]
fn normalize_url_is_noop_for_regular_urls() {
    let normalized = normalize_url("http://example.com/foo/bar").expect("normalize_url succeeds");
    assert!(!normalized.was_normalized);
    assert_eq!(normalized.url.as_str(), "http://example.com/foo/bar");
    assert_eq!(
        normalized
            .denormalize(None)
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/foo/bar"
    );
    assert_eq!(
        normalized
            .denormalize(Some("/baz"))
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/baz"
    );
    assert_eq!(
        normalized
            .denormalize(Some("?some=query#hash"))
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/foo/bar?some=query#hash"
    );
    assert_eq!(
        normalized
            .denormalize(Some("http://somethingelse.com/"))
            .expect("denormalize succeeds")
            .as_str(),
        "http://somethingelse.com/"
    );
}

#[test]
fn normalizes_trailing_slash() {
    let normalized = normalize_url("http://example.com/foo/bar/").expect("normalize_url succeeds");
    assert!(normalized.was_normalized);
    assert_eq!(normalized.url.as_str(), "http://example.com/foo/bar");
    assert_eq!(
        normalized
            .denormalize(None)
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/foo/bar/"
    );
    assert_eq!(
        normalized
            .denormalize(Some("/baz"))
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/baz/"
    );
}

#[test]
fn normalizes_data_and_route_requests() {
    let data = normalize_url("http://example.com/foo/__data.json").expect("normalize_url succeeds");
    assert!(data.was_normalized);
    assert_eq!(data.url.as_str(), "http://example.com/foo");
    assert_eq!(
        data.denormalize(None)
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/foo/__data.json"
    );
    assert_eq!(
        data.denormalize(Some("/baz"))
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/baz/__data.json"
    );

    let route = normalize_url("http://example.com/foo/__route.js").expect("normalize_url succeeds");
    assert!(route.was_normalized);
    assert_eq!(route.url.as_str(), "http://example.com/foo");
    assert_eq!(
        route
            .denormalize(None)
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/foo/__route.js"
    );
    assert_eq!(
        route
            .denormalize(Some("/baz"))
            .expect("denormalize succeeds")
            .as_str(),
        "http://example.com/baz/__route.js"
    );
}
