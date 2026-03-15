use url::Url;

use svelte_kit::{
    CookieError, CookieJar, CookieOptions, CookieParseOptions, Error, ResolvedCookieOptions,
    SameSite, add_data_suffix, domain_matches, path_matches,
};

fn reverse_encode(value: &str) -> String {
    value.chars().rev().collect()
}

fn identity_decode(value: &str) -> String {
    value.to_string()
}

fn cookie_jar(href: &str, header: Option<&str>) -> CookieJar {
    let url = Url::parse(href).expect("valid url");
    let mut jar = CookieJar::new(header.or(Some("a=b")), &url);
    jar.set_trailing_slash("ignore")
        .expect("set trailing slash");
    jar
}

#[test]
fn matches_cookie_domains_like_upstream() {
    let cases = [
        ("localhost", None),
        ("example.com", Some("example.com")),
        ("sub.example.com", Some("example.com")),
        ("example.com", Some(".example.com")),
        ("sub.example.com", Some(".example.com")),
    ];

    for (hostname, constraint) in cases {
        assert!(domain_matches(hostname, constraint));
    }
}

#[test]
fn matches_cookie_paths_like_upstream() {
    let positives = [
        ("/", None),
        ("/foo", Some("/")),
        ("/foo", Some("/foo")),
        ("/foo/", Some("/foo")),
        ("/foo", Some("/foo/")),
    ];
    let negatives = [("/", Some("/foo")), ("/food", Some("/foo"))];

    for (path, constraint) in positives {
        assert!(path_matches(path, constraint));
    }

    for (path, constraint) in negatives {
        assert!(!path_matches(path, constraint));
    }
}

#[test]
fn cookie_is_not_present_after_delete() {
    let mut jar = cookie_jar("https://example.com/", None);
    jar.set(
        "a",
        "b",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set cookie");
    assert_eq!(jar.get("a"), Some("b".to_string()));

    jar.delete(
        "a",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("delete cookie");

    assert_eq!(jar.get("a"), None);
}

#[test]
fn decodes_request_header_cookies_by_default() {
    let jar = cookie_jar("https://example.com/", Some("a=f%C3%BC; b=foo+bar"));
    assert_eq!(jar.get("a"), Some("fü".to_string()));
    assert_eq!(jar.get("b"), Some("foo+bar".to_string()));
}

#[test]
fn supports_custom_decode_for_request_header_cookies() {
    let jar = cookie_jar("https://example.com/", Some("a=f%C3%BC; b=foo+bar"));

    assert_eq!(
        jar.get_with_options(
            "a",
            CookieParseOptions {
                decode: Some(identity_decode),
            },
        ),
        Some("f%C3%BC".to_string())
    );

    let all = jar.get_all_with_options(CookieParseOptions {
        decode: Some(identity_decode),
    });
    assert!(all.contains(&svelte_kit::CookieEntry {
        name: "a".to_string(),
        value: "f%C3%BC".to_string(),
    }));
    assert!(all.contains(&svelte_kit::CookieEntry {
        name: "b".to_string(),
        value: "foo+bar".to_string(),
    }));
}

#[test]
fn rejects_oversized_cookies_in_debug_builds() {
    let mut jar = cookie_jar("https://example.com/", None);
    let error = jar
        .set(
            "a",
            &"a".repeat(4097),
            CookieOptions {
                path: Some("/".to_string()),
                ..Default::default()
            },
        )
        .expect_err("oversized cookie should be rejected");

    assert!(matches!(
        error,
        Error::Cookie(CookieError::OversizedCookie { ref name }) if name == "a"
    ));
    assert_eq!(
        error.to_string(),
        "Cookie \"a\" is too large, and will be discarded by the browser"
    );
}

#[test]
fn applies_default_values_when_setting_cookies() {
    let mut jar = cookie_jar("https://example.com/foo/bar", None);
    jar.set(
        "a",
        "b",
        CookieOptions {
            path: Some(String::new()),
            ..Default::default()
        },
    )
    .expect("set cookie");

    let cookie = jar.new_cookies().get("/foo/bar?a").expect("stored cookie");
    assert_eq!(cookie.options.secure, true);
    assert_eq!(cookie.options.http_only, true);
    assert_eq!(cookie.options.path, "/foo/bar");
    assert_eq!(cookie.options.same_site, SameSite::Lax);
}

#[test]
fn localhost_defaults_to_insecure_cookies() {
    let mut jar = cookie_jar("http://localhost:1234/", None);
    jar.set(
        "a",
        "b",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set cookie");

    let cookie = jar.new_cookies().get("/?a").expect("stored cookie");
    assert!(!cookie.options.secure);
}

#[test]
fn delete_forces_max_age_zero_and_preserves_defaults() {
    let mut jar = cookie_jar("https://example.com/", None);
    jar.delete(
        "a",
        CookieOptions {
            path: Some("/".to_string()),
            max_age: Some(1234),
            ..Default::default()
        },
    )
    .expect("delete cookie");

    let cookie = jar.new_cookies().get("/?a").expect("stored deleted cookie");
    assert_eq!(cookie.options.secure, true);
    assert_eq!(cookie.options.http_only, true);
    assert_eq!(cookie.options.same_site, SameSite::Lax);
    assert_eq!(cookie.options.max_age, Some(0));
}

#[test]
fn cookie_names_are_case_sensitive() {
    let mut jar = cookie_jar("https://example.com/", None);
    jar.set(
        "a",
        "foo",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set lowercase cookie");
    jar.set(
        "A",
        "bar",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set uppercase cookie");

    assert_eq!(
        jar.new_cookies()
            .get("/?a")
            .expect("lowercase cookie")
            .value,
        "foo"
    );
    assert_eq!(
        jar.new_cookies()
            .get("/?A")
            .expect("uppercase cookie")
            .value,
        "bar"
    );
}

#[test]
fn get_prefers_most_specific_matching_cookie_path() {
    let mut root = cookie_jar("https://example.com/", None);
    root.set(
        "key",
        "value_root",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set root cookie");
    root.set(
        "key",
        "value_foo",
        CookieOptions {
            path: Some("/foo".to_string()),
            ..Default::default()
        },
    )
    .expect("set /foo cookie");
    assert_eq!(root.get("key"), Some("value_root".to_string()));

    let mut foo = cookie_jar("https://example.com/foo", None);
    foo.set(
        "key",
        "value_root",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set root cookie");
    foo.set(
        "key",
        "value_foo",
        CookieOptions {
            path: Some("/foo".to_string()),
            ..Default::default()
        },
    )
    .expect("set /foo cookie");
    assert_eq!(foo.get("key"), Some("value_foo".to_string()));
}

#[test]
fn get_prefers_most_specific_matching_cookie_domain() {
    let mut jar = cookie_jar("https://sub.example.com/", None);
    jar.set(
        "key",
        "parent",
        CookieOptions {
            path: Some("/".to_string()),
            domain: Some("example.com".to_string()),
            ..Default::default()
        },
    )
    .expect("set parent-domain cookie");
    jar.set(
        "key",
        "subdomain",
        CookieOptions {
            path: Some("/".to_string()),
            domain: Some("sub.example.com".to_string()),
            ..Default::default()
        },
    )
    .expect("set subdomain cookie");

    assert_eq!(jar.get("key"), Some("subdomain".to_string()));
}

#[test]
fn get_all_prefers_more_specific_paths() {
    let mut jar = cookie_jar("https://example.com/foo/bar", None);
    jar.set(
        "duplicate",
        "foobar_value",
        CookieOptions {
            path: Some("/foo/bar".to_string()),
            ..Default::default()
        },
    )
    .expect("set specific cookie");
    jar.set(
        "duplicate",
        "root_value",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set root cookie");
    jar.set(
        "duplicate",
        "foo_value",
        CookieOptions {
            path: Some("/foo".to_string()),
            ..Default::default()
        },
    )
    .expect("set foo cookie");

    let duplicate = jar
        .get_all()
        .into_iter()
        .find(|cookie| cookie.name == "duplicate")
        .expect("duplicate cookie");

    assert_eq!(duplicate.value, "foobar_value");
}

#[test]
fn supports_multiple_cookies_with_same_name_different_domains() {
    let mut jar = cookie_jar("https://example.com/", None);
    jar.set(
        "key",
        "value1",
        CookieOptions {
            path: Some("/".to_string()),
            domain: Some("example.com".to_string()),
            ..Default::default()
        },
    )
    .expect("set example.com cookie");
    jar.set(
        "key",
        "value2",
        CookieOptions {
            path: Some("/".to_string()),
            domain: Some("sub.example.com".to_string()),
            ..Default::default()
        },
    )
    .expect("set subdomain cookie");

    assert_eq!(
        jar.new_cookies()
            .get("example.com/?key")
            .expect("example.com cookie")
            .value,
        "value1"
    );
    assert_eq!(
        jar.new_cookies()
            .get("sub.example.com/?key")
            .expect("subdomain cookie")
            .value,
        "value2"
    );
}

#[test]
fn merges_cookie_headers_with_correct_precedence() {
    let mut jar = cookie_jar("https://example.com/", Some("a=f%C3%BC; b=foo+bar"));
    jar.set(
        "c",
        "fö",
        CookieOptions {
            path: Some("/".to_string()),
            ..Default::default()
        },
    )
    .expect("set cookie");
    jar.set(
        "d",
        "fö",
        CookieOptions {
            path: Some("/".to_string()),
            encode: Some(reverse_encode),
            ..Default::default()
        },
    )
    .expect("set custom-encoded cookie");

    let destination = Url::parse("https://example.com/").expect("destination url");
    let header = jar.get_cookie_header(&destination, Some("e=f%C3%A4; f=foo+bar"));

    assert_eq!(
        header,
        "a=f%C3%BC; b=foo+bar; c=f%C3%B6; d=öf; e=f%C3%A4; f=foo+bar"
    );
}

#[test]
fn set_internal_is_not_affected_by_defaults() {
    let url = Url::parse("https://example.com/a/b/c").expect("valid url");
    let mut jar = CookieJar::new(Some("a=b"), &url);
    jar.set_trailing_slash("ignore")
        .expect("set trailing slash");

    let options = ResolvedCookieOptions {
        domain: None,
        encode: None,
        http_only: false,
        max_age: None,
        path: "/a/b/c".to_string(),
        same_site: SameSite::None,
        secure: false,
    };

    jar.set_internal("test", "foo", options.clone())
        .expect("set internal cookie");

    assert_eq!(jar.get("test"), Some("foo".to_string()));
    assert_eq!(
        jar.new_cookies()
            .get("/a/b/c?test")
            .expect("stored internal cookie")
            .options,
        options
    );
}

#[test]
fn adds_data_suffix_for_html_cookie_headers() {
    let mut jar = cookie_jar("https://example.com/", None);
    jar.set(
        "session",
        "abc",
        CookieOptions {
            path: Some("/index.html".to_string()),
            ..Default::default()
        },
    )
    .expect("set cookie");

    let headers = jar.set_cookie_headers();

    assert_eq!(headers.len(), 2);
    assert!(headers[0].contains("Path=/index.html"));
    assert!(headers[1].contains(&format!("Path={}", add_data_suffix("/index.html"))));
}
