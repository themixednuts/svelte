use std::collections::BTreeMap;

use svelte_kit::{
    Error, ParsedRouteId, RoutingError, exec_route_match, find_route, get_route_segments,
    parse_route_id, remove_optional_params, resolve_route, sort_routes,
};

#[test]
fn sorts_routes_like_upstream() {
    let expected = vec![
        "/",
        "/a",
        "/b",
        "/b/[required]",
        "/c",
        "/c/bar",
        "/c/b[x].json",
        "/c/b[x]",
        "/c/foo",
        "/d/e",
        "/d/e[...rest]",
        "/e/f",
        "/e/[...rest]/f",
        "/f/static[...rest]",
        "/f/[...rest]static",
        "/g/[[optional]]/static",
        "/g/[required]",
        "/g/[...rest]/[required]",
        "/h/a/b",
        "/h/a/[required]/b",
        "/h/a/[...rest]/b",
        "/x/[...rest]",
        "/[...rest]/x",
        "/[...rest]/x/[...deep_rest]/y",
        "/[...rest]/x/[...deep_rest]",
        "/[required=matcher]",
        "/[required]",
        "/[...rest]",
    ];

    let mut actual = expected.iter().rev().copied().collect::<Vec<_>>();
    sort_routes(&mut actual, |route| route);

    assert_eq!(actual, expected);
}

#[test]
fn sorts_rest_tail_routes_like_upstream() {
    let expected = vec![
        "/[...rest]/x",
        "/[...rest]/x/[...deep_rest]/y",
        "/[...rest]/x/[...deep_rest]",
    ];

    let mut actual = vec![
        "/[...rest]/x/[...deep_rest]",
        "/[...rest]/x",
        "/[...rest]/x/[...deep_rest]/y",
    ];
    sort_routes(&mut actual, |route| route);

    assert_eq!(actual, expected);
}

#[test]
fn sorts_rest_tail_route_before_deeper_rest_route() {
    let mut actual = vec!["/[...rest]/x/[...deep_rest]", "/[...rest]/x"];
    sort_routes(&mut actual, |route| route);

    assert_eq!(actual, vec!["/[...rest]/x", "/[...rest]/x/[...deep_rest]"]);
}

#[test]
fn sorts_static_tail_after_rest_tail_route() {
    let mut actual = vec![
        "/[...rest]/x/[...deep_rest]",
        "/[...rest]/x/[...deep_rest]/y",
    ];
    sort_routes(&mut actual, |route| route);

    assert_eq!(
        actual,
        vec![
            "/[...rest]/x/[...deep_rest]/y",
            "/[...rest]/x/[...deep_rest]"
        ]
    );
}

#[test]
fn exec_extracts_params_like_upstream() {
    let parsed = parse_route_id("/[...a=matches]/[b]/[c]").expect("parse route");
    let captures = parsed.pattern.captures("/foo/bar").expect("match route");
    let params = exec_route_match(&captures, &parsed.params, |matcher, _| matcher == "matches");

    assert_eq!(
        params,
        Some(BTreeMap::from([
            ("a".to_string(), "".to_string()),
            ("b".to_string(), "foo".to_string()),
            ("c".to_string(), "bar".to_string()),
        ]))
    );
}

#[test]
fn resolve_route_generates_paths_like_upstream() {
    let params = BTreeMap::from([
        ("one".to_string(), "one".to_string()),
        ("two".to_string(), "two/three".to_string()),
    ]);

    assert_eq!(
        resolve_route("/blog/[one=matcher]/[...two]/", &params).expect("resolve route"),
        "/blog/one/two/three/"
    );
}

#[test]
fn resolve_route_errors_on_missing_required_params() {
    let error = resolve_route(
        "/blog/[one]/[two]",
        &BTreeMap::from([("one".to_string(), "one".to_string())]),
    )
    .expect_err("missing param should fail");

    assert!(matches!(
        error,
        Error::Routing(RoutingError::MissingRouteParameter { ref name, ref route_id })
            if name == "two" && route_id == "/blog/[one]/[two]"
    ));
    assert_eq!(
        error.to_string(),
        "Missing parameter 'two' in route /blog/[one]/[two]"
    );
}

#[test]
fn gets_route_segments_like_upstream() {
    assert_eq!(get_route_segments("/"), Vec::<String>::new());
    assert_eq!(get_route_segments("/a/(group)/b"), vec!["a", "b"]);
    assert_eq!(get_route_segments("/blog/[slug]"), vec!["blog", "[slug]"]);
}

#[test]
fn removes_optional_params_like_upstream() {
    assert_eq!(
        remove_optional_params("/blog/[[lang]]/[slug]"),
        "/blog/[slug]"
    );
    assert_eq!(remove_optional_params("/[[lang]]"), "/");
}

#[derive(Clone)]
struct TestRoute {
    id: &'static str,
    parsed: ParsedRouteId,
}

#[test]
fn finds_routes_and_decodes_params_like_upstream() {
    let routes = ["/blog", "/blog/[slug]", "/about"]
        .into_iter()
        .map(|id| TestRoute {
            id,
            parsed: parse_route_id(id).expect("parse route"),
        })
        .collect::<Vec<_>>();

    let result = find_route(
        "/blog/hello%20world",
        &routes,
        |route| (&route.parsed.pattern, &route.parsed.params),
        |_, _| true,
    )
    .expect("find matching route");

    assert_eq!(result.route.id, "/blog/[slug]");
    assert_eq!(
        result.params,
        BTreeMap::from([("slug".to_string(), "hello world".to_string())])
    );
}
