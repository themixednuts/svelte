use http::{HeaderMap, HeaderName, HeaderValue};
use svelte_kit::validate_headers;

fn header_map<I, K, V>(entries: I) -> HeaderMap
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut headers = HeaderMap::new();
    for (name, value) in entries {
        headers.insert(
            HeaderName::from_bytes(name.as_ref().as_bytes()).expect("valid test header name"),
            HeaderValue::from_str(value.as_ref()).expect("valid test header value"),
        );
    }
    headers
}

#[test]
fn validates_cache_control_headers() {
    assert!(
        validate_headers(&header_map([(
            "cache-control".to_string(),
            "public, max-age=3600".to_string(),
        )]))
        .is_empty()
    );

    let warnings = validate_headers(&header_map([(
        "cache-control".to_string(),
        "public, maxage=3600".to_string(),
    )]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("Invalid cache-control directive \"maxage\""));

    let warnings = validate_headers(&header_map([(
        "cache-control".to_string(),
        "public,, max-age=3600".to_string(),
    )]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("`cache-control` header contains empty directives"));

    let warnings = validate_headers(&header_map([(
        "cache-control".to_string(),
        "public, , max-age=3600".to_string(),
    )]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("`cache-control` header contains empty directives"));

    assert!(
        validate_headers(&header_map([(
            "cache-control".to_string(),
            "max-age=3600, s-maxage=7200".to_string(),
        )]))
        .is_empty()
    );
}

#[test]
fn validates_content_type_headers() {
    assert!(
        validate_headers(&header_map([(
            "content-type".to_string(),
            "text/html; charset=utf-8".to_string(),
        )]))
        .is_empty()
    );

    assert!(
        validate_headers(&header_map([(
            "content-type".to_string(),
            "TEXT/HTML; charset=utf-8".to_string(),
        )]))
        .is_empty()
    );

    assert!(
        validate_headers(&header_map([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]))
        .is_empty()
    );

    assert!(
        validate_headers(&header_map([(
            "content-type".to_string(),
            "application/javascript; charset=utf-8".to_string(),
        )]))
        .is_empty()
    );

    assert!(
        validate_headers(&header_map([(
            "content-type".to_string(),
            "x-custom/whatever".to_string(),
        )]))
        .is_empty()
    );

    let warnings = validate_headers(&header_map([(
        "content-type".to_string(),
        "invalid-content-type".to_string(),
    )]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("Invalid content-type value \"invalid-content-type\""));
}

#[test]
fn ignores_unknown_headers_and_combines_results() {
    assert!(
        validate_headers(&header_map([(
            "x-custom-header".to_string(),
            "some-value".to_string(),
        )]))
        .is_empty()
    );

    let warnings = validate_headers(&header_map([
        ("x-custom".to_string(), "value".to_string()),
        ("cache-control".to_string(), "max-age=3600".to_string()),
        (
            "content-type".to_string(),
            "bad/type; charset=utf-8".to_string(),
        ),
    ]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("Invalid content-type value \"bad/type\""));
}
