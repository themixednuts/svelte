use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde_json::{Map, Value, json};
use svelte_kit::{
    AppState, CookieOptions, DataRequestNode, Error, PrerenderState, RuntimeEvent,
    RuntimeLoadError, RuntimeRenderState, SameSite, ServerDataUses, ServerRequest, UniversalFetch,
    UniversalFetchBody, UniversalFetchContext, UniversalFetchCookieHeader,
    UniversalFetchCookieSetter, UniversalFetchCredentials, UniversalFetchHandle,
    UniversalFetchMode, UniversalFetchOptions, UniversalFetchRawResponse, build_runtime_event,
    create_universal_fetch, load_data, load_server_data,
};
use url::Url;

fn create_fetch<F>(fetcher: F) -> UniversalFetch
where
    F: FnMut(&str, &UniversalFetchOptions) -> UniversalFetchRawResponse + 'static,
{
    let event_url = Url::parse("https://domain-a.com").expect("valid event url");
    let state = RuntimeRenderState::default();

    create_universal_fetch(
        UniversalFetchContext {
            event_url,
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        state,
        true,
        |_, _| false,
        fetcher,
    )
}

fn runtime_event(url: &str, route_id: Option<&str>) -> RuntimeEvent {
    build_runtime_event(
        &ServerRequest {
            method: Method::GET,
            url: Url::parse(url).expect("valid request url"),
            headers: HeaderMap::new(),
        },
        Arc::new(AppState::default()),
        Url::parse(url).expect("valid rewritten url"),
        route_id.map(str::to_string),
        BTreeMap::from([("slug".to_string(), "hello".to_string())]),
        false,
        false,
        0,
    )
}

#[test]
fn sets_body_to_empty_when_mode_is_no_cors() {
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text("foo"));

    let response = fetch
        .fetch(
            "https://domain-b.com",
            UniversalFetchOptions {
                mode: UniversalFetchMode::NoCors,
                ..Default::default()
            },
        )
        .expect("universal fetch should succeed");

    assert_eq!(response.text().expect("read no-cors body"), "");
}

#[test]
fn keeps_body_for_same_origin_requests() {
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text("foo"));

    let response = fetch
        .fetch("https://domain-a.com", UniversalFetchOptions::default())
        .expect("same-origin fetch should succeed");

    assert_eq!(response.text().expect("read same-origin body"), "foo");
}

#[test]
fn load_server_data_tracks_dependency_usage_like_upstream() {
    let event = runtime_event(
        "https://domain-a.com/blog/hello?lang=en",
        Some("/blog/[slug]"),
    );

    let node = load_server_data(
        &event,
        Some("/blog/[slug]"),
        Some("always"),
        || {
            let mut parent = Map::new();
            parent.insert("layout".to_string(), json!(true));
            Ok(parent)
        },
        |context| {
            context.depends("/api/posts")?;
            assert_eq!(context.param("slug").as_deref(), Some("hello"));
            assert_eq!(context.search_param("lang").as_deref(), Some("en"));
            assert_eq!(context.route_id().as_deref(), Some("/blog/[slug]"));
            assert_eq!(context.url().path(), "/blog/hello");
            assert_eq!(context.parent()?.get("layout"), Some(&json!(true)));

            context.untrack(|context| {
                context
                    .depends("/api/ignored")
                    .expect("untracked dependency");
                assert_eq!(context.param("ignored"), None);
            });

            Ok(Some(json!({ "post": true })))
        },
    )
    .expect("load server data");

    assert_eq!(
        node,
        DataRequestNode::Data {
            data: json!({ "post": true }),
            uses: Some(ServerDataUses {
                dependencies: BTreeSet::from(["https://domain-a.com/api/posts".to_string()]),
                search_params: BTreeSet::from(["lang".to_string()]),
                params: BTreeSet::from(["slug".to_string()]),
                parent: true,
                route: true,
                url: true,
            }),
            slash: Some("always".to_string()),
        }
    );
}

#[test]
fn load_server_data_rejects_non_object_payloads() {
    let event = runtime_event("https://domain-a.com/blog/hello", Some("/blog/[slug]"));

    let error = load_server_data(
        &event,
        Some("/blog/[slug]"),
        None,
        || Ok(Map::new()),
        |_| Ok(Some(json!(["bad"]))),
    )
    .expect_err("non-object server load should fail");

    assert_eq!(
        error.to_string(),
        "a load function in /blog/[slug] returned an array, but must return a plain object at the top level (i.e. `return {...}`)"
    );
}

#[test]
fn load_data_returns_server_data_without_universal_load() {
    let event = runtime_event("https://domain-a.com/blog/hello", Some("/blog/[slug]"));
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text("unused"));

    let data = load_data(
        &event,
        Some("/blog/[slug]"),
        Some(json!({ "server": true })),
        || Ok(Map::new()),
        &mut fetch,
        None::<
            fn(
                &mut svelte_kit::UniversalLoadContext<'_>,
            ) -> svelte_kit::Result<Option<serde_json::Value>>,
        >,
    )
    .expect("load data without universal load");

    assert_eq!(data, Some(json!({ "server": true })));
}

#[test]
fn load_data_runs_universal_load_with_fetch_parent_and_headers() {
    let event = runtime_event(
        "https://domain-a.com/blog/hello?lang=en",
        Some("/blog/[slug]"),
    );
    let mut fetch = create_fetch(|url, _| UniversalFetchRawResponse::text(url));

    let data = load_data(
        &event,
        Some("/blog/[slug]"),
        Some(json!({ "server": true })),
        || {
            let mut parent = Map::new();
            parent.insert("layout".to_string(), json!(1));
            Ok(parent)
        },
        &mut fetch,
        Some(|context: &mut svelte_kit::UniversalLoadContext<'_>| {
            assert_eq!(context.route_id(), Some("/blog/[slug]"));
            assert_eq!(context.param("slug"), Some("hello"));
            assert_eq!(context.url().path(), "/blog/hello");
            assert_eq!(context.data(), Some(&json!({ "server": true })));
            assert_eq!(context.parent()?.get("layout"), Some(&json!(1)));

            let fetched = context.fetch("/api", UniversalFetchOptions::default())?;
            assert_eq!(fetched.text()?, "https://domain-a.com/api");

            context.set_headers(&{
                let mut headers = HeaderMap::new();
                headers.insert(
                    HeaderName::from_static("x-load"),
                    HeaderValue::from_static("ok"),
                );
                headers
            })?;

            context.depends(&["/ignored"]);
            context.untrack(|_| ());

            Ok(Some(json!({ "page": true })))
        }),
    )
    .expect("load data with universal load");

    assert_eq!(data, Some(json!({ "page": true })));
    assert_eq!(
        event.capture_response_effects().headers.get("x-load"),
        Some(&HeaderValue::from_static("ok"))
    );

    let fetched = fetch.take_fetched();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].url, "/api");
}

#[test]
fn load_data_rejects_non_object_payloads() {
    fn invalid_load(
        _: &mut svelte_kit::UniversalLoadContext<'_>,
    ) -> svelte_kit::Result<Option<serde_json::Value>> {
        Ok(Some(json!(["bad"])))
    }

    let event = runtime_event("https://domain-a.com/blog/hello", Some("/blog/[slug]"));
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text("unused"));

    let error = load_data(
        &event,
        Some("/blog/[slug]"),
        None,
        || Ok(Map::new()),
        &mut fetch,
        Some(invalid_load),
    )
    .expect_err("non-object universal load should fail");

    assert_eq!(
        error.to_string(),
        "a load function in /blog/[slug] returned an array, but must return a plain object at the top level (i.e. `return {...}`)"
    );
}

#[test]
fn captures_prerender_dependencies_for_same_origin_fetches() {
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState {
            prerendering: Some(PrerenderState::default()),
            ..Default::default()
        },
        true,
        |_, _| false,
        |_, _| UniversalFetchRawResponse::text("prefetched").with_header("etag", "\"abc\""),
    );

    let response = fetch
        .fetch("/api/posts?lang=en", UniversalFetchOptions::default())
        .expect("same-origin prerender fetch");
    assert_eq!(response.text().expect("response text"), "prefetched");

    let dependency = fetch
        .state()
        .prerendering
        .as_ref()
        .expect("prerender state")
        .dependencies
        .get("/api/posts")
        .expect("prerender dependency");
    assert_eq!(dependency.body.as_deref(), Some("prefetched"));
    assert_eq!(dependency.response.status.as_u16(), 200);
    assert_eq!(dependency.response.header("etag"), Some("\"abc\""));
}

#[test]
fn allows_cross_origin_fetch_with_acao_header() {
    let mut fetch = create_fetch(|_, _| {
        UniversalFetchRawResponse::text("foo").with_header("access-control-allow-origin", "*")
    });

    let response = fetch
        .fetch("https://domain-b.com", UniversalFetchOptions::default())
        .expect("cors fetch should succeed");

    assert_eq!(response.text().expect("read cors body"), "foo");
}

#[test]
fn rejects_cross_origin_fetch_without_acao_header() {
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text("foo"));

    let response = fetch
        .fetch("https://domain-b.com", UniversalFetchOptions::default())
        .expect("fetch should produce lazy cors response");

    let error = response.text().expect_err("cors body should fail");
    assert!(matches!(
        error,
        Error::RuntimeLoad(RuntimeLoadError::ResponseBodyUnavailable { .. })
    ));
    assert_eq!(
        error.to_string(),
        "CORS error: No 'Access-Control-Allow-Origin' header is present on the requested resource"
    );
}

#[test]
fn allows_fetches_from_local_schemes() {
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text("foo"));

    let response = fetch
        .fetch("data:text/plain;foo", UniversalFetchOptions::default())
        .expect("data url fetch should succeed");

    assert_eq!(response.text().expect("read local-scheme body"), "foo");
}

#[test]
fn rejects_unserialized_response_headers() {
    let mut fetch = create_fetch(|_, _| {
        UniversalFetchRawResponse::text("foo").with_header("content-type", "text/plain")
    });

    let response = fetch
        .fetch("https://domain-a.com", UniversalFetchOptions::default())
        .expect("same-origin fetch should succeed");

    let error = response
        .headers()
        .get("content-type")
        .expect_err("content-type access should fail");
    assert!(matches!(
        error,
        Error::RuntimeLoad(RuntimeLoadError::FilteredResponseHeader { ref name, .. })
            if name == "content-type"
    ));
    assert_eq!(
        error.to_string(),
        "Failed to get response header \"content-type\" — it must be included by the `filterSerializedResponseHeaders` option: https://svelte.dev/docs/kit/hooks#Server-hooks-handle (at foo)"
    );
}

#[test]
fn parses_json_response_bodies() {
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text("{\"ok\":true}"));

    let response = fetch
        .fetch("https://domain-a.com", UniversalFetchOptions::default())
        .expect("same-origin fetch should succeed");

    assert_eq!(response.json().expect("json body"), json!({ "ok": true }));
}

#[test]
fn returns_null_json_for_empty_bodies() {
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::text(""));

    let response = fetch
        .fetch("https://domain-a.com", UniversalFetchOptions::default())
        .expect("same-origin fetch should succeed");

    assert_eq!(response.json().expect("empty json body"), Value::Null);
}

#[test]
fn allows_x_sveltekit_response_headers_without_filter() {
    let mut fetch = create_fetch(|_, _| {
        UniversalFetchRawResponse::text("ok").with_header("x-sveltekit-routeid", "/blog/[slug]")
    });

    let response = fetch
        .fetch("https://domain-a.com", UniversalFetchOptions::default())
        .expect("same-origin fetch should succeed");

    assert_eq!(
        response
            .headers()
            .get("x-sveltekit-routeid")
            .expect("x-sveltekit header access should succeed"),
        Some("/blog/[slug]".to_string())
    );
}

#[test]
fn exposes_binary_bodies_via_array_buffer_and_records_base64_payloads() {
    let mut fetch = create_fetch(|_, _| UniversalFetchRawResponse::binary([0_u8, 1, 2]));

    let response = fetch
        .fetch("https://domain-a.com", UniversalFetchOptions::default())
        .expect("same-origin fetch should succeed");

    assert_eq!(
        response.array_buffer().expect("binary body"),
        vec![0_u8, 1, 2]
    );
    assert_eq!(
        response.text().expect("binary text body"),
        "\u{0}\u{1}\u{2}"
    );

    let fetched = fetch.take_fetched();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].response_body, "AAEC");
    assert!(fetched[0].is_b64);
}

#[test]
fn records_same_origin_and_cross_origin_fetches() {
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |name, _| name == "etag",
        |url, options| {
            let mut headers = HeaderMap::new();
            headers.insert(
                HeaderName::from_static("etag"),
                HeaderValue::from_static("abc"),
            );
            UniversalFetchRawResponse {
                status: StatusCode::OK,
                status_text: String::new(),
                headers,
                body: UniversalFetchBody::Text(
                    options.body.clone().unwrap_or_else(|| url.to_string()),
                ),
            }
        },
    );

    let same_origin = fetch
        .fetch(
            "/api",
            UniversalFetchOptions {
                method: Method::POST,
                body: Some("payload".to_string()),
                ..Default::default()
            },
        )
        .expect("same-origin fetch");
    assert_eq!(same_origin.text().expect("same-origin body"), "payload");

    let cross_origin = fetch
        .fetch("https://domain-b.com/x", UniversalFetchOptions::default())
        .expect("cross-origin fetch");
    let _ = cross_origin.text().expect_err("missing acao should fail");

    let fetched = fetch.take_fetched();
    assert_eq!(fetched.len(), 2);
    assert_eq!(fetched[0].url, "/api");
    assert_eq!(fetched[0].method, Method::GET);
    assert_eq!(fetched[0].request_body.as_deref(), Some("payload"));
    assert_eq!(
        fetched[0].response_headers.get("etag").map(String::as_str),
        Some("abc")
    );
    assert_eq!(fetched[1].url, "https://domain-b.com/x");
}

#[test]
fn records_event_request_method_in_fetched_metadata() {
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::POST,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        |_, _| UniversalFetchRawResponse::text("ok"),
    );

    let response = fetch
        .fetch(
            "/api",
            UniversalFetchOptions {
                method: Method::PUT,
                ..Default::default()
            },
        )
        .expect("fetch should succeed");
    assert_eq!(response.text().expect("response text"), "ok");

    let fetched = fetch.take_fetched();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].method, Method::POST);
}

#[test]
fn includes_origin_header_on_non_get_internal_request() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let response = fetch
        .fetch(
            "/internal",
            UniversalFetchOptions {
                method: Method::POST,
                ..Default::default()
            },
        )
        .expect("internal non-get fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert_eq!(
        headers.get("origin").and_then(|value| value.to_str().ok()),
        Some("https://domain-a.com")
    );
}

#[test]
fn includes_origin_header_on_non_get_external_request() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok").with_header("access-control-allow-origin", "*")
        },
    );

    let response = fetch
        .fetch(
            "https://domain-b.com/submit",
            UniversalFetchOptions {
                method: Method::PUT,
                ..Default::default()
            },
        )
        .expect("external non-get fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert_eq!(
        headers.get("origin").and_then(|value| value.to_str().ok()),
        Some("https://domain-a.com")
    );
}

#[test]
fn removes_origin_header_for_same_origin_get_requests() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("origin"),
        HeaderValue::from_static("https://domain-a.com"),
    );

    let response = fetch
        .fetch(
            "/internal",
            UniversalFetchOptions {
                method: Method::GET,
                headers,
                ..Default::default()
            },
        )
        .expect("same-origin get fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert!(!headers.contains_key("origin"));
}

#[test]
fn removes_origin_header_for_no_cors_cross_origin_get_requests() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("origin"),
        HeaderValue::from_static("https://domain-a.com"),
    );

    let response = fetch
        .fetch(
            "https://domain-b.com/resource",
            UniversalFetchOptions {
                method: Method::GET,
                mode: UniversalFetchMode::NoCors,
                headers,
                ..Default::default()
            },
        )
        .expect("no-cors cross-origin fetch");
    assert_eq!(response.text().expect("body"), "");

    let headers = seen.borrow().clone().expect("captured headers");
    assert!(!headers.contains_key("origin"));
}

#[test]
fn adds_default_accept_header() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut fetch = create_fetch(move |_, options| {
        *captured.borrow_mut() = Some(options.headers.clone());
        UniversalFetchRawResponse::text("ok")
    });

    let response = fetch
        .fetch("/internal", UniversalFetchOptions::default())
        .expect("default accept fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert_eq!(
        headers.get("accept").and_then(|value| value.to_str().ok()),
        Some("*/*")
    );
}

#[test]
fn forwards_accept_language_from_event_request() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut event_headers = HeaderMap::new();
    event_headers.insert(
        HeaderName::from_static("accept-language"),
        HeaderValue::from_static("en-US,en;q=0.9"),
    );

    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: event_headers,
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let response = fetch
        .fetch("/internal", UniversalFetchOptions::default())
        .expect("accept-language fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert_eq!(
        headers
            .get("accept-language")
            .and_then(|value| value.to_str().ok()),
        Some("en-US,en;q=0.9")
    );
}

#[test]
fn forwards_authorization_for_same_origin_requests_when_credentials_are_not_omit() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut event_headers = HeaderMap::new();
    event_headers.insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_static("Bearer secret"),
    );

    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: event_headers,
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let response = fetch
        .fetch(
            "/internal",
            UniversalFetchOptions {
                credentials: UniversalFetchCredentials::SameOrigin,
                ..Default::default()
            },
        )
        .expect("authorization fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert_eq!(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer secret")
    );
}

#[test]
fn omits_authorization_when_credentials_are_omit() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let mut event_headers = HeaderMap::new();
    event_headers.insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_static("Bearer secret"),
    );

    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: event_headers,
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let response = fetch
        .fetch(
            "/internal",
            UniversalFetchOptions {
                credentials: UniversalFetchCredentials::Omit,
                ..Default::default()
            },
        )
        .expect("omit authorization fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert!(!headers.contains_key("authorization"));
}

#[test]
fn forwards_cookie_header_for_same_origin_requests() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let get_cookie_header =
        UniversalFetchCookieHeader::new(|_: &Url, header: Option<&str>| match header {
            Some(existing) => format!("session=abc; {existing}"),
            None => "session=abc".to_string(),
        });

    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: Some(get_cookie_header),
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let response = fetch
        .fetch("/internal", UniversalFetchOptions::default())
        .expect("same-origin cookie fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert_eq!(
        headers.get("cookie").and_then(|value| value.to_str().ok()),
        Some("session=abc")
    );
}

#[test]
fn forwards_cookie_header_for_subdomain_requests_when_credentials_are_not_omit() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let get_cookie_header =
        UniversalFetchCookieHeader::new(|_: &Url, header: Option<&str>| match header {
            Some(existing) => format!("shared=1; {existing}"),
            None => "shared=1".to_string(),
        });

    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://my.domain.com/blog").expect("valid event url"),
            get_cookie_header: Some(get_cookie_header),
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok").with_header("access-control-allow-origin", "*")
        },
    );

    let response = fetch
        .fetch(
            "https://sub.my.domain.com/api",
            UniversalFetchOptions {
                credentials: UniversalFetchCredentials::Include,
                ..Default::default()
            },
        )
        .expect("subdomain cookie fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert_eq!(
        headers.get("cookie").and_then(|value| value.to_str().ok()),
        Some("shared=1")
    );
}

#[test]
fn does_not_forward_cookie_header_when_credentials_are_omit() {
    let seen = Rc::new(RefCell::new(None));
    let captured = Rc::clone(&seen);
    let get_cookie_header =
        UniversalFetchCookieHeader::new(|_: &Url, _header: Option<&str>| "session=abc".to_string());

    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: Some(get_cookie_header),
            handle_fetch: None,
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *captured.borrow_mut() = Some(options.headers.clone());
            UniversalFetchRawResponse::text("ok")
        },
    );

    let response = fetch
        .fetch(
            "/internal",
            UniversalFetchOptions {
                credentials: UniversalFetchCredentials::Omit,
                ..Default::default()
            },
        )
        .expect("omit cookie fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let headers = seen.borrow().clone().expect("captured headers");
    assert!(!headers.contains_key("cookie"));
}

#[test]
fn captures_set_cookie_headers_into_internal_cookie_store() {
    let captured = Arc::new(Mutex::new(Vec::<(String, String, CookieOptions)>::new()));
    let set_internal_cookie = {
        let captured = Arc::clone(&captured);
        UniversalFetchCookieSetter::new(move |name: &str, value: &str, options: CookieOptions| {
            captured
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((name.to_string(), value.to_string(), options));
            Ok(())
        })
    };

    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog/page").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: None,
            set_internal_cookie: Some(set_internal_cookie),
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, _| {
            let mut headers = HeaderMap::new();
            headers.append(
                HeaderName::from_static("set-cookie"),
                HeaderValue::from_static("session=abc; HttpOnly; Secure; SameSite=Strict"),
            );
            headers.append(
                HeaderName::from_static("set-cookie"),
                HeaderValue::from_static("theme=dark; Path=/prefs; Max-Age=60"),
            );
            UniversalFetchRawResponse {
                status: StatusCode::OK,
                status_text: String::new(),
                headers,
                body: UniversalFetchBody::Text("ok".to_string()),
            }
        },
    );

    let response = fetch
        .fetch("/internal", UniversalFetchOptions::default())
        .expect("set-cookie capture fetch");
    assert_eq!(response.text().expect("body"), "ok");

    let captured = captured.lock().unwrap_or_else(|error| error.into_inner());
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].0, "session");
    assert_eq!(captured[0].1, "abc");
    assert_eq!(captured[0].2.path.as_deref(), Some("/"));
    assert_eq!(captured[0].2.http_only, Some(true));
    assert_eq!(captured[0].2.secure, Some(true));
    assert_eq!(captured[0].2.same_site, Some(SameSite::Strict));

    assert_eq!(captured[1].0, "theme");
    assert_eq!(captured[1].1, "dark");
    assert_eq!(captured[1].2.path.as_deref(), Some("/prefs"));
    assert_eq!(captured[1].2.max_age, Some(60));
}

#[test]
fn handle_fetch_can_short_circuit_response() {
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: Some(UniversalFetchHandle::new(|context| {
                assert_eq!(context.url.as_str(), "https://domain-a.com/internal");
                Ok(UniversalFetchRawResponse::text("hooked"))
            })),
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        |_, _| UniversalFetchRawResponse::text("unreachable"),
    );

    let response = fetch
        .fetch("/internal", UniversalFetchOptions::default())
        .expect("hooked fetch");

    assert_eq!(response.text().expect("body"), "hooked");
}

#[test]
fn handle_fetch_can_delegate_to_underlying_fetch() {
    let seen = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mut fetch = create_universal_fetch(
        UniversalFetchContext {
            event_url: Url::parse("https://domain-a.com/blog").expect("valid event url"),
            get_cookie_header: None,
            handle_fetch: Some(UniversalFetchHandle::new(move |context| {
                let mut options = context.options.clone();
                options.body = Some("via-hook".to_string());
                (context.fetch)(context.url.as_str(), &options)
            })),
            set_internal_cookie: None,
            request_headers: HeaderMap::new(),
            request_method: Method::GET,
            route_id: Some("foo".to_string()),
        },
        RuntimeRenderState::default(),
        true,
        |_, _| false,
        move |_, options| {
            *seen_clone.lock().unwrap_or_else(|error| error.into_inner()) = options.body.clone();
            UniversalFetchRawResponse::text(options.body.as_deref().unwrap_or("missing"))
        },
    );

    let response = fetch
        .fetch("/internal", UniversalFetchOptions::default())
        .expect("delegated fetch");

    assert_eq!(response.text().expect("body"), "via-hook");
    assert_eq!(
        seen.lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_deref(),
        Some("via-hook")
    );
}
