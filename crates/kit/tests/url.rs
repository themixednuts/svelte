use std::collections::BTreeMap;

use svelte_kit::{
    Error, UrlError, decode_params, decode_pathname, decode_uri, is_root_relative, normalize_path,
    resolve, strip_hash, try_decode_pathname,
};

#[test]
fn resolves_urls_like_upstream() {
    let cases = [
        ("/a/b/c", "/x/y/z", "/x/y/z"),
        ("/a/b/c", "d", "/a/b/d"),
        ("/a/b/c", "d/", "/a/b/d/"),
        ("/a/b/c", "./d", "/a/b/d"),
        ("/a/b/c", "d/./e/./f", "/a/b/d/e/f"),
        ("/a/b/c", "../d", "/a/d"),
        ("/a/b/c", "d/./e/../f", "/a/b/d/f"),
        ("/a/b/c", "../../../../../d", "/d"),
        ("/a/b/c", "/x/./y/../z", "/x/z"),
        ("/a/b/c", "//example.com/foo", "//example.com/foo"),
        (
            "/a/b/c",
            "https://example.com/foo",
            "https://example.com/foo",
        ),
        (
            "/a/b/c",
            "mailto:hello@svelte.dev",
            "mailto:hello@svelte.dev",
        ),
        ("/a/b/c", "#foo", "/a/b/c#foo"),
        ("/a/b/c", "data:text/plain,hello", "data:text/plain,hello"),
        ("/a/b/c", "", "/a/b/c"),
        ("/a/b/c", ".", "/a/b/"),
    ];

    for (base, path, expected) in cases {
        assert_eq!(resolve(base, path), expected);
    }
}

#[test]
fn normalizes_paths_like_upstream() {
    let cases = [
        ("/", "/", "/", "/"),
        ("/foo", "/foo", "/foo/", "/foo"),
        ("/foo/", "/foo/", "/foo/", "/foo"),
    ];

    for (path, ignore, always, never) in cases {
        assert_eq!(normalize_path(path, "ignore"), ignore);
        assert_eq!(normalize_path(path, "always"), always);
        assert_eq!(normalize_path(path, "never"), never);
    }
}

#[test]
fn detects_root_relative_paths_and_strips_hashes() {
    assert!(is_root_relative("/foo"));
    assert!(!is_root_relative("//example.com/foo"));
    assert_eq!(
        strip_hash("https://example.com/foo#bar"),
        "https://example.com/foo"
    );
}

#[test]
fn decodes_pathnames_without_double_decoding_percent25() {
    assert_eq!(decode_pathname("/blog/%E2%9C%93"), "/blog/\u{2713}");
    assert_eq!(decode_pathname("/blog/%2525"), "/blog/%2525");
}

#[test]
fn decodes_route_params_and_uri_errors_like_upstream() {
    let decoded = decode_params(BTreeMap::from([(
        "slug".to_string(),
        "a%2Fb%20c".to_string(),
    )]))
    .expect("decoded params");
    assert_eq!(decoded.get("slug").map(String::as_str), Some("a/b c"));

    let err = decode_uri("%zz").expect_err("invalid uri should error");
    assert!(matches!(
        err,
        Error::Url(UrlError::InvalidPercentEncoding { ref uri, ref hex, .. })
            if uri == "%zz" && hex == "zz"
    ));
    assert!(err.to_string().starts_with("Failed to decode URI: %zz"));
}

#[test]
fn reports_invalid_pathname_decoding_without_panicking() {
    let err = try_decode_pathname("/%FF").expect_err("invalid pathname should error");
    assert!(matches!(
        err,
        Error::Url(UrlError::InvalidPathnameUtf8 { ref pathname, .. })
            if pathname == "/%FF"
    ));
    assert!(
        err.to_string()
            .starts_with("Failed to decode pathname: /%FF")
    );
}
