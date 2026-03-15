use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::Utf8PathBuf;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use serde_json::{Map, Value, json};
use svelte_kit::{
    ActionJsonResult, ActionRequestResult, AppState, ClientRoute, CookieJar, CookieOptions,
    DataRequestNode, EndpointError, EndpointModule, Error, ErrorPageRenderPlan,
    ErrorPageRequestResult, ExecutedErrorPage, KitManifest, ManifestConfig, ManifestRoute,
    PageActionExecution, PageErrorBoundary, PageExecutionResult, PageLoadResult, PageLoadedNode,
    PageRenderPlan, PageRequestResult, PageRuntimeDecision, PreparedDataRequest, RenderedPage,
    RequestEventState, RequestKind, RouteResolutionAssets, RuntimeDevalueError,
    RuntimeEndpointError, RuntimeEvent, RuntimeExecutionResult, RuntimeFatalError,
    RuntimeHttpError, RuntimePageError, RuntimePageNodes, RuntimeRenderState,
    RuntimeRequestOptions, RuntimeRespondResult, RuntimeRouteBehavior, RuntimeRouteDispatch,
    ServerDataNode, ServerDataUses, ServerRequest, ServerRequestEvent, ServerResponse,
    ServerTransportEncoder, ShellPageResponse, SpecialRuntimeRequestOptions, action_json_response,
    allowed_methods, apply_page_prerender_policy, build_runtime_event, check_csrf,
    check_incorrect_fail_use, check_remote_request_origin, clarify_devalue_error,
    create_server_routing_response, data_json_response, data_request_not_found_response,
    dispatch_special_runtime_request, execute_data_request, execute_error_page_load,
    execute_error_page_request, execute_page_load, execute_page_request,
    execute_prepared_runtime_request, execute_runtime_page_request, execute_runtime_page_stage,
    finalize_route_response, format_server_error, get_global_name, get_node_type,
    get_remote_action, get_remote_id, handle_fatal_error, invalidated_data_node_flags,
    is_action_json_request, is_action_request, is_endpoint_request, is_pojo, load_server_data,
    materialize_page_request_result, maybe_not_modified_response, method_not_allowed_message,
    method_not_allowed_response, no_actions_action_json_response, no_actions_action_request_result,
    page_method_response, page_request_requires_shell_only, prepare_data_request,
    prepare_error_page_render, prepare_page_render_plan, prepare_request_url,
    prepare_runtime_execution, preprocess_runtime_request, public_env_response,
    redirect_data_response, redirect_response, render_data_request, render_endpoint,
    render_shell_page_response, request_normalization_redirect, resolve_page_runtime_decision,
    resolve_remote_request_url, resolve_route_request_response, resolve_runtime_request,
    resolve_runtime_route_behavior, resolve_runtime_route_dispatch, respond_runtime_request,
    respond_runtime_request_materialized_with_named_page_stage,
    respond_runtime_request_materialized_with_page_stage,
    respond_runtime_request_with_named_page_stage, respond_runtime_request_with_page_stage,
    response_with_vary_accept, route_data_node_indexes, runtime_normalization_response,
    serialize_uses, set_response_cookies, set_response_header, static_error_page,
};
use url::Url;

fn app_state() -> AppState {
    AppState::default()
}

fn date_encoder_app_state() -> AppState {
    AppState {
        decoders: BTreeMap::new(),
        encoders: BTreeMap::from([(
            "date".to_string(),
            Arc::new(|value: &Value| {
                if value.get("$type") == Some(&json!("date")) {
                    Ok(Some(json!(
                        value.get("value").cloned().unwrap_or(Value::Null)
                    )))
                } else {
                    Ok(None)
                }
            }) as ServerTransportEncoder,
        )]),
    }
}

#[test]
fn allows_head_via_get_like_upstream() {
    let methods = BTreeSet::from(["GET".to_string()]);
    assert_eq!(
        allowed_methods(&methods),
        vec!["GET".to_string(), "HEAD".to_string()]
    );
}

#[test]
fn prefers_endpoints_for_non_html_requests() {
    assert!(is_endpoint_request(
        &Method::GET,
        Some("application/json"),
        None
    ));
    assert!(!is_endpoint_request(&Method::GET, Some("text/html"), None));
    assert!(is_endpoint_request(&Method::PUT, Some("text/html"), None));
    assert!(!is_endpoint_request(
        &Method::POST,
        Some("*/*"),
        Some("true")
    ));
}

#[test]
fn formats_method_not_allowed_message() {
    assert_eq!(
        method_not_allowed_message(&Method::PATCH),
        "PATCH method not allowed"
    );
}

#[test]
fn builds_method_not_allowed_response() {
    let methods = BTreeSet::from(["GET".to_string()]);
    let response = method_not_allowed_response(&Method::PATCH, &methods);

    assert_eq!(response.status, 405);
    assert_eq!(response.body.as_deref(), Some("PATCH method not allowed"));
    assert_eq!(response.header("allow"), Some("GET, HEAD"));
}

#[test]
fn builds_redirect_and_static_error_responses() {
    let redirect = redirect_response(302, "/foo");
    assert_eq!(redirect.status, 302);
    assert_eq!(redirect.header("location"), Some("/foo"));

    let error = static_error_page(
        500,
        "bad <error> & worse",
        |status, message| format!("<html><head></head><body>{status}:{message}</body></html>"),
        true,
    );
    assert_eq!(error.status, 500);
    assert_eq!(
        error.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    let body = error.body.expect("error body");
    assert!(body.contains("500:bad &lt;error> &amp; worse"));
    assert!(body.contains("/@vite/client"));
}

#[test]
fn exposes_global_name_and_pojo_rules() {
    assert_eq!(get_global_name("abc123", true), "__sveltekit_dev");
    assert_eq!(get_global_name("abc123", false), "__sveltekit_abc123");
    assert!(is_pojo(&json!({ "ok": true })));
    assert!(is_pojo(&json!(null)));
    assert!(!is_pojo(&json!(["nope"])));
}

fn request_event(method: &str, url: &str) -> ServerRequestEvent {
    ServerRequestEvent {
        request: ServerRequest {
            method: method.parse().expect("valid method"),
            url: Url::parse(url).expect("valid url"),
            headers: HeaderMap::new(),
        },
        route_id: Some("/api".to_string()),
    }
}

fn test_header_map(name: &'static str, value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    );
    headers
}

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

fn repo_root() -> Utf8PathBuf {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    for candidate in manifest_dir.ancestors() {
        if candidate.join("kit").is_dir() {
            return candidate.to_path_buf();
        }
    }

    panic!("failed to locate repository root");
}

fn temp_dir(label: &str) -> Utf8PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let dir = repo_root()
        .join("tmp")
        .join(format!("svelte-kit-{label}-{unique}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(path: &Utf8PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write file");
}

#[test]
fn renders_endpoint_with_head_fallback_and_redirects() {
    let event = request_event("HEAD", "https://example.com/api");
    let mut event_state = RequestEventState::default();
    let mut state = RuntimeRenderState::default();
    let module = EndpointModule::new()
        .with_handler(Method::GET, |_| {
            let mut response = ServerResponse::new(200);
            response.body = Some("ok".to_string());
            Ok(response)
        })
        .with_handler(Method::POST, |_| {
            Err(EndpointError::Redirect {
                status: 303,
                location: "/login".to_string(),
            })
        });

    let response = render_endpoint(&event, &mut event_state, &module, &mut state)
        .expect("head fallback response");
    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_deref(), Some("ok"));
    assert!(event_state.allows_commands);

    let post_event = request_event("POST", "https://example.com/api");
    let redirect = render_endpoint(
        &post_event,
        &mut RequestEventState::default(),
        &module,
        &mut state,
    )
    .expect("redirect response");
    assert_eq!(redirect.status, 303);
    assert_eq!(redirect.header("location"), Some("/login"));
}

#[test]
fn handles_prerender_rules_for_endpoints() {
    let event = request_event("GET", "https://example.com/api");
    let module = EndpointModule::new().with_handler(Method::GET, |_| Ok(ServerResponse::new(200)));

    let mut non_prerenderable_state = RuntimeRenderState {
        error: false,
        prerender_default: false,
        prerendering: Some(Default::default()),
        depth: 1,
        ..Default::default()
    };
    let error = render_endpoint(
        &event,
        &mut RequestEventState::default(),
        &module,
        &mut non_prerenderable_state,
    )
    .expect_err("nested non-prerenderable request should fail");
    assert!(matches!(
        error,
        Error::RuntimeEndpoint(RuntimeEndpointError::NotPrerenderable { ref route_id })
            if route_id == "/api"
    ));
    assert_eq!(error.to_string(), "/api is not prerenderable");

    let mut top_level_prerender_state = RuntimeRenderState {
        error: false,
        prerender_default: false,
        prerendering: Some(Default::default()),
        depth: 0,
        ..Default::default()
    };
    let response = render_endpoint(
        &event,
        &mut RequestEventState::default(),
        &module,
        &mut top_level_prerender_state,
    )
    .expect("top level prerender response");
    assert_eq!(response.status, 204);

    let mut prerendered_module = EndpointModule::new().with_handler(Method::GET, |_| {
        let mut response = ServerResponse::new(200);
        response.body = Some("ok".to_string());
        Ok(response)
    });
    prerendered_module.prerender = Some(true);

    let mut reroute_state = RuntimeRenderState {
        error: false,
        prerender_default: false,
        prerendering: Some(svelte_kit::PrerenderState {
            inside_reroute: true,
            ..Default::default()
        }),
        depth: 0,
        ..Default::default()
    };
    let response = render_endpoint(
        &event,
        &mut RequestEventState::default(),
        &prerendered_module,
        &mut reroute_state,
    )
    .expect("reroute response");
    assert_eq!(response.status, 200);
    let dependency = reroute_state
        .prerendering
        .as_ref()
        .expect("prerender state")
        .dependencies
        .get("/api")
        .expect("prerender dependency");
    assert_eq!(
        dependency.response.header("x-sveltekit-prerender"),
        Some("true")
    );
}

#[test]
fn prepares_data_and_route_resolution_requests_like_upstream() {
    let data = Url::parse(
        "https://example.com/blog/__data.json?x-sveltekit-trailing-slash=1&x-sveltekit-invalidated=101&q=1",
    )
    .expect("valid data url");
    let prepared = prepare_request_url(&data).expect("prepared data request");
    assert_eq!(prepared.kind, RequestKind::Data);
    assert_eq!(prepared.url.path(), "/blog/");
    assert_eq!(prepared.url.query(), Some("q=1"));
    assert_eq!(
        prepared.invalidated_data_nodes,
        Some(vec![true, false, true])
    );

    let route_resolution =
        Url::parse("https://example.com/blog/__route.js?answer=42").expect("valid route url");
    let prepared = prepare_request_url(&route_resolution).expect("prepared route request");
    assert_eq!(prepared.kind, RequestKind::RouteResolution);
    assert_eq!(prepared.url.path(), "/blog");
    assert_eq!(prepared.url.query(), Some("answer=42"));
    assert_eq!(prepared.invalidated_data_nodes, None);
}

#[test]
fn normalizes_runtime_request_paths_like_upstream() {
    let url = Url::parse("https://example.com/foo/?a=1").expect("valid url");
    assert_eq!(
        request_normalization_redirect(&url, "never", false),
        Some("/foo?a=1".to_string())
    );

    let url = Url::parse("https://example.com//foo/?x=1").expect("valid url");
    assert_eq!(
        request_normalization_redirect(&url, "never", false),
        Some("https://example.com//foo?x=1".to_string())
    );

    let data = Url::parse("https://example.com/foo/__data.json").expect("valid data request");
    assert_eq!(request_normalization_redirect(&data, "always", true), None);
}

#[test]
fn converts_matching_etag_responses_to_not_modified() {
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: header_map([("if-none-match".to_string(), "W/\"abc\"".to_string())]),
    };
    let mut response = ServerResponse::new(200);
    response.set_header("etag", "\"abc\"");
    response.set_header("cache-control", "public, max-age=30");
    response.append_header("set-cookie", "a=1; Path=/");
    response.body = Some("body".to_string());

    let not_modified = maybe_not_modified_response(&request, &response).expect("304 response");
    assert_eq!(not_modified.status, 304);
    assert_eq!(not_modified.body, None);
    assert_eq!(not_modified.header("etag"), Some("\"abc\""));
    assert_eq!(
        not_modified.header("cache-control"),
        Some("public, max-age=30")
    );
    assert_eq!(
        not_modified.header_values("set-cookie"),
        Some(vec!["a=1; Path=/"])
    );
}

#[test]
fn appends_vary_accept_when_needed() {
    let response = ServerResponse::new(200);
    let response = response_with_vary_accept(&response);
    assert_eq!(response.header("vary"), Some("Accept"));

    let mut response = ServerResponse::new(200);
    response.set_header("vary", "Origin");
    let response = response_with_vary_accept(&response);
    assert_eq!(response.header("vary"), Some("Origin, Accept"));

    let mut response = ServerResponse::new(200);
    response.set_header("vary", "accept");
    let response = response_with_vary_accept(&response);
    assert_eq!(response.header("vary"), Some("accept"));
}

#[test]
fn resolves_runtime_requests_against_manifest_with_base_and_data_suffixes() {
    let cwd = temp_dir("resolve-runtime-request");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("api").join("+server.js"),
        "export function GET() {}",
    );

    let manifest = KitManifest::discover(&ManifestConfig::new(routes_dir, cwd.clone()))
        .expect("discover manifest");

    let resolved = resolve_runtime_request(
        &manifest,
        &Url::parse("https://example.com/base/blog/hello/__data.json?x-sveltekit-invalidated=1")
            .expect("valid data url"),
        "/base",
        |_, _| true,
    )
    .expect("resolved request")
    .expect("matched route");
    assert_eq!(resolved.prepared.kind, RequestKind::Data);
    assert_eq!(resolved.resolved_path, "/blog/hello");
    assert_eq!(resolved.route.id, "/blog/[slug]");
    assert_eq!(
        resolved.params.get("slug").map(String::as_str),
        Some("hello")
    );

    let endpoint = resolve_runtime_request(
        &manifest,
        &Url::parse("https://example.com/base/api").expect("valid endpoint url"),
        "/base",
        |_, _| true,
    )
    .expect("resolved endpoint request")
    .expect("matched endpoint route");
    assert_eq!(endpoint.route.id, "/api");

    let missing = resolve_runtime_request(
        &manifest,
        &Url::parse("https://example.com/elsewhere/blog/hello").expect("valid url"),
        "/base",
        |_, _| true,
    )
    .expect("resolved non-base request");
    assert!(missing.is_none());
}

#[test]
fn reduces_runtime_page_node_options_like_upstream() {
    let cwd = temp_dir("runtime-page-nodes");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("+layout.server.js"),
        "export const trailingSlash = 'always'; export const config = { root: true, shared: 'root' }; export function load() {}",
    );
    write_file(
        &routes_dir.join("dashboard").join("+layout.js"),
        "export const csr = false; export const config = { shared: 'dashboard', dashboard: true };",
    );
    write_file(
        &routes_dir
            .join("dashboard")
            .join("reports")
            .join("+page.server.js"),
        "export const prerender = true; export const config = { page: 'server' };",
    );
    write_file(
        &routes_dir
            .join("dashboard")
            .join("reports")
            .join("+page.js"),
        "export const ssr = false; export const config = { page: 'universal' };",
    );
    write_file(
        &routes_dir
            .join("dashboard")
            .join("reports")
            .join("+page.svelte"),
        "<h1>report</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/dashboard/reports")
        .and_then(|route| route.page.as_ref())
        .expect("page route");

    let nodes = RuntimePageNodes::from_route(route, &manifest);
    assert_eq!(nodes.layouts().len(), 2);
    assert!(nodes.page().is_some());
    assert!(!nodes.csr());
    assert!(!nodes.ssr());
    assert!(nodes.prerender());
    assert_eq!(nodes.trailing_slash(), "always");
    assert!(nodes.should_prerender_data());
    assert_eq!(
        nodes.get_config(),
        Some(Map::from_iter([
            ("dashboard".to_string(), json!(true)),
            ("page".to_string(), json!("universal")),
            ("root".to_string(), json!(true)),
            ("shared".to_string(), json!("dashboard")),
        ]))
    );
}

#[test]
fn resolves_runtime_route_behavior_for_pages_endpoints_and_base_root() {
    let cwd = temp_dir("runtime-route-behavior");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("+layout.server.js"),
        "export const trailingSlash = 'always'; export const config = { root: true };",
    );
    write_file(
        &routes_dir.join("dashboard").join("+page.svelte"),
        "<h1>dashboard</h1>",
    );
    write_file(
        &routes_dir.join("dashboard").join("+page.js"),
        "export const prerender = true; export const config = { page: true };",
    );
    write_file(
        &routes_dir.join("api").join("+server.js"),
        "export const trailingSlash = 'ignore'; export const prerender = true; export const config = { api: 'edge' };",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let root_route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/")
        .expect("root route");
    let page_route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/dashboard")
        .expect("page route");
    let endpoint_route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/api")
        .expect("endpoint route");

    assert_eq!(
        resolve_runtime_route_behavior(&manifest, root_route, "/base", "/base"),
        RuntimeRouteBehavior {
            trailing_slash: "always".to_string(),
            prerender: false,
            config: Map::new(),
        }
    );
    assert_eq!(
        resolve_runtime_route_behavior(&manifest, page_route, "/base/dashboard", "/base"),
        RuntimeRouteBehavior {
            trailing_slash: "always".to_string(),
            prerender: true,
            config: Map::from_iter([
                ("page".to_string(), json!(true)),
                ("root".to_string(), json!(true)),
            ]),
        }
    );
    assert_eq!(
        resolve_runtime_route_behavior(&manifest, endpoint_route, "/base/api", "/base"),
        RuntimeRouteBehavior {
            trailing_slash: "ignore".to_string(),
            prerender: true,
            config: Map::from_iter([("api".to_string(), json!("edge"))]),
        }
    );
}

#[test]
fn builds_runtime_normalization_redirect_response() {
    let request_url = Url::parse("https://example.com/blog/?q=1").expect("valid url");
    let prepared = prepare_request_url(&request_url).expect("prepared request");
    let response = runtime_normalization_response(
        &request_url,
        &prepared,
        &RuntimeRouteBehavior {
            trailing_slash: "never".to_string(),
            prerender: false,
            config: Map::new(),
        },
    )
    .expect("normalization response");

    assert_eq!(response.status, 308);
    assert_eq!(response.header("location"), Some("/blog?q=1"));
    assert_eq!(response.header("x-sveltekit-normalize"), Some("1"));

    let data_url = Url::parse("https://example.com/blog/__data.json").expect("valid data url");
    let prepared = prepare_request_url(&data_url).expect("prepared data request");
    assert!(
        runtime_normalization_response(
            &data_url,
            &prepared,
            &RuntimeRouteBehavior {
                trailing_slash: "always".to_string(),
                prerender: false,
                config: Map::new(),
            },
        )
        .is_none()
    );
}

#[test]
fn preprocesses_runtime_requests_with_normalization_redirects() {
    let cwd = temp_dir("preprocess-normalize");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+page.js"),
        "export const trailingSlash = 'always';",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid request url"),
        headers: HeaderMap::new(),
    };
    let preprocessed = preprocess_runtime_request(
        &manifest,
        &request,
        &RuntimeRequestOptions {
            base: String::new(),
            app_dir: "_app".to_string(),
            hash_routing: false,
            csrf_check_origin: true,
            csrf_trusted_origins: Vec::new(),
            public_env: Map::new(),
            route_assets: RouteResolutionAssets::default(),
        },
        &RuntimeRenderState::default(),
        |_matcher, _value| true,
    )
    .expect("preprocess request");

    assert!(preprocessed.resolved.is_some());
    let response = preprocessed
        .early_response
        .expect("normalization redirect response");
    assert_eq!(response.status, 308);
    assert_eq!(response.header("location"), Some("/blog/"));
}

#[test]
fn resolves_runtime_route_dispatch_like_upstream() {
    let cwd = temp_dir("runtime-route-dispatch");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+server.js"),
        "export function GET() {}",
    );
    write_file(
        &routes_dir.join("docs").join("+page.svelte"),
        "<h1>docs</h1>",
    );
    write_file(
        &routes_dir.join("api").join("+server.js"),
        "export function GET() {}",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let page_and_endpoint = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .expect("blog route");
    let endpoint_only = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/api")
        .expect("api route");
    let page_only = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/docs")
        .expect("docs route");

    let html_get = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };
    assert_eq!(
        resolve_runtime_route_dispatch(page_and_endpoint, &html_get, false, false)
            .expect("dispatch"),
        RuntimeRouteDispatch::Page
    );

    let json_get = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: header_map([("accept".to_string(), "application/json".to_string())]),
    };
    assert_eq!(
        resolve_runtime_route_dispatch(page_and_endpoint, &json_get, false, false)
            .expect("dispatch"),
        RuntimeRouteDispatch::Endpoint
    );

    let action_post = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: header_map([
            ("accept".to_string(), "*/*".to_string()),
            ("x-sveltekit-action".to_string(), "true".to_string()),
        ]),
    };
    assert_eq!(
        resolve_runtime_route_dispatch(page_and_endpoint, &action_post, false, true)
            .expect("dispatch"),
        RuntimeRouteDispatch::Page
    );

    let data_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog/__data.json").expect("valid url"),
        headers: HeaderMap::new(),
    };
    assert_eq!(
        resolve_runtime_route_dispatch(page_and_endpoint, &data_request, true, false)
            .expect("dispatch"),
        RuntimeRouteDispatch::Data
    );

    let options_request = ServerRequest {
        method: Method::OPTIONS,
        url: Url::parse("https://example.com/docs").expect("valid url"),
        headers: HeaderMap::new(),
    };
    let RuntimeRouteDispatch::PageMethodNotAllowed(response) =
        resolve_runtime_route_dispatch(page_only, &options_request, false, true).expect("dispatch")
    else {
        panic!("expected page method response");
    };
    assert_eq!(response.status, 204);
    assert_eq!(response.header("allow"), Some("GET, HEAD, OPTIONS, POST"));

    let endpoint_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/api").expect("valid url"),
        headers: HeaderMap::new(),
    };
    assert_eq!(
        resolve_runtime_route_dispatch(endpoint_only, &endpoint_request, false, false)
            .expect("dispatch"),
        RuntimeRouteDispatch::Endpoint
    );
}

#[test]
fn finalizes_route_responses_with_vary_accept_for_page_and_endpoint_routes() {
    let route = ManifestRoute {
        id: "/blog".to_string(),
        pattern: regex::Regex::new("^/blog/?$").expect("regex"),
        params: Vec::new(),
        page: Some(svelte_kit::ManifestRoutePage {
            layouts: Vec::new(),
            errors: Vec::new(),
            leaf: 0,
        }),
        endpoint: Some(svelte_kit::ManifestEndpoint {
            file: Utf8PathBuf::from("src/routes/blog/+server.js"),
            page_options: None,
        }),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: HeaderMap::new(),
    };
    let response = finalize_route_response(&request, &route, &ServerResponse::new(200));
    assert_eq!(response.header("vary"), Some("Accept"));

    let post_request = ServerRequest {
        method: Method::POST,
        ..request.clone()
    };
    let response = finalize_route_response(&post_request, &route, &ServerResponse::new(200));
    assert!(response.header("vary").is_none());
}

#[test]
fn exposes_data_request_foundations_like_upstream() {
    let route = ManifestRoute {
        id: "/blog".to_string(),
        pattern: regex::Regex::new("^/blog/?$").expect("regex"),
        params: Vec::new(),
        page: Some(svelte_kit::ManifestRoutePage {
            layouts: vec![Some(1), None, Some(3)],
            errors: Vec::new(),
            leaf: 4,
        }),
        endpoint: None,
    };

    assert_eq!(
        route_data_node_indexes(&route),
        Some(vec![Some(1), None, Some(3), Some(4)])
    );
    assert_eq!(
        invalidated_data_node_flags(4, Some(&[true, false])),
        vec![true, false, false, false]
    );
    assert_eq!(invalidated_data_node_flags(3, None), vec![true, true, true]);

    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: header_map([("accept".to_string(), "application/json".to_string())]),
    };
    assert!(is_action_json_request(&request));
    let html_request = ServerRequest {
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
        ..request.clone()
    };
    assert!(!is_action_json_request(&html_request));

    let json = data_json_response(json!({ "type": "data" }), 200);
    assert_eq!(json.status, 200);
    assert_eq!(json.header("content-type"), Some("application/json"));
    assert_eq!(json.header("cache-control"), Some("private, no-store"));
    assert_eq!(json.body.as_deref(), Some("{\"type\":\"data\"}"));

    let redirect = redirect_data_response("/login");
    assert_eq!(
        redirect.body.as_deref(),
        Some("{\"location\":\"/login\",\"type\":\"redirect\"}")
    );

    let not_found = data_request_not_found_response();
    assert_eq!(not_found.status, 404);
    assert!(not_found.body.is_none());

    let skipped = DataRequestNode::Skip;
    assert_eq!(skipped, DataRequestNode::Skip);
}

#[test]
fn prepares_data_requests_from_resolved_routes() {
    let cwd = temp_dir("prepare-data-request");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("+layout.server.js"),
        "export const trailingSlash = 'always'; export function load() {}",
    );
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+page.server.js"),
        "export const trailingSlash = 'ignore';",
    );
    write_file(
        &routes_dir.join("api").join("+server.js"),
        "export function GET() {}",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let page_url = Url::parse(
        "https://example.com/blog/__data.json?x-sveltekit-trailing-slash=1&x-sveltekit-invalidated=10",
    )
    .expect("valid data url");
    let resolved = resolve_runtime_request(&manifest, &page_url, "", |_matcher, _value| true)
        .expect("resolve request")
        .expect("resolved route");
    let behavior = resolve_runtime_route_behavior(&manifest, resolved.route, "/blog/", "");
    let prepared = prepare_data_request(&resolved, &behavior).expect("prepared data request");
    assert_eq!(
        prepared,
        PreparedDataRequest {
            normalized_pathname: "/blog/".to_string(),
            node_indexes: vec![Some(0), Some(2)],
            invalidated: vec![true, false],
        }
    );

    let endpoint_url = Url::parse("https://example.com/api/__data.json").expect("valid data url");
    let resolved = resolve_runtime_request(&manifest, &endpoint_url, "", |_matcher, _value| true)
        .expect("resolve request")
        .expect("resolved route");
    let behavior = resolve_runtime_route_behavior(&manifest, resolved.route, "/api", "");
    assert!(prepare_data_request(&resolved, &behavior).is_none());
}

#[test]
fn renders_data_requests_with_parent_merging_errors_and_redirects() {
    let prepared = PreparedDataRequest {
        normalized_pathname: "/blog/".to_string(),
        node_indexes: vec![Some(0), Some(1), None, Some(2)],
        invalidated: vec![true, true, true, true],
    };

    let response = render_data_request(
        &prepared,
        &app_state(),
        |node_index, parent: &Map<String, Value>| match node_index {
            0 => {
                assert!(parent.is_empty());
                Ok(DataRequestNode::Data {
                    data: json!({ "root": 1 }),
                    uses: None,
                    slash: Some("always".to_string()),
                })
            }
            1 => {
                assert_eq!(parent.get("root"), Some(&json!(1)));
                Ok(DataRequestNode::Error {
                    status: Some(500),
                    error: json!({ "message": "broken" }),
                })
            }
            2 => panic!("aborted nodes should not execute"),
            _ => unreachable!(),
        },
    )
    .expect("render data request");

    assert_eq!(response.status, 200);
    assert_eq!(response.header("cache-control"), Some("private, no-store"));
    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"nodes\":[{\"data\":{\"root\":1},\"slash\":\"always\",\"type\":\"data\",\"uses\":{}},{\"error\":{\"message\":\"broken\"},\"status\":500,\"type\":\"error\"},{\"type\":\"skip\"},{\"type\":\"skip\"}],\"type\":\"data\"}"
        )
    );

    let redirect =
        render_data_request(
            &prepared,
            &app_state(),
            |node_index, _parent| match node_index {
                0 => Ok(DataRequestNode::Redirect {
                    location: "/login".to_string(),
                }),
                _ => unreachable!(),
            },
        )
        .expect("redirect response");
    assert_eq!(
        redirect.body.as_deref(),
        Some("{\"location\":\"/login\",\"type\":\"redirect\"}")
    );
}

#[test]
fn renders_data_requests_with_transport_encoded_payloads() {
    let app_state = date_encoder_app_state();

    let prepared = PreparedDataRequest {
        normalized_pathname: "/blog/".to_string(),
        node_indexes: vec![Some(0)],
        invalidated: vec![true],
    };

    let response = render_data_request(
        &prepared,
        &app_state,
        |node_index, parent: &Map<String, Value>| match node_index {
            0 => {
                assert!(parent.is_empty());
                Ok(DataRequestNode::Data {
                    data: json!({
                        "published": { "$type": "date", "value": "2026-03-12" }
                    }),
                    uses: None,
                    slash: None,
                })
            }
            _ => unreachable!(),
        },
    )
    .expect("render transport-encoded data request");

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"nodes\":[{\"data\":{\"published\":{\"kind\":\"date\",\"type\":\"Transport\",\"value\":\"2026-03-12\"}},\"type\":\"data\",\"uses\":{}}],\"type\":\"data\"}"
        )
    );
}

#[test]
fn rejects_non_object_data_request_payloads() {
    let prepared = PreparedDataRequest {
        normalized_pathname: "/blog/".to_string(),
        node_indexes: vec![Some(0)],
        invalidated: vec![true],
    };

    let error = render_data_request(
        &prepared,
        &app_state(),
        |node_index, parent: &Map<String, Value>| match node_index {
            0 => {
                assert!(parent.is_empty());
                Ok(DataRequestNode::Data {
                    data: json!(["bad"]),
                    uses: None,
                    slash: None,
                })
            }
            _ => unreachable!(),
        },
    )
    .expect_err("invalid data request payload should fail");

    assert_eq!(
        error.to_string(),
        "a load function while rendering data request node 0 returned an array, but must return a plain object at the top level (i.e. `return {...}`)"
    );
}

#[test]
fn executes_data_requests_from_resolved_runtime_routes() {
    let cwd = temp_dir("execute-data-request");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("api").join("+server.js"),
        "export function GET() {}",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");

    let page_url = Url::parse("https://example.com/blog/__data.json").expect("valid page data url");
    let page_resolved = resolve_runtime_request(&manifest, &page_url, "", |_matcher, _value| true)
        .expect("resolve page request")
        .expect("resolved page route");
    let page_behavior = resolve_runtime_route_behavior(&manifest, page_resolved.route, "/blog", "");
    let response = execute_data_request(
        &page_resolved,
        &page_behavior,
        &app_state(),
        |node_index, parent| match node_index {
            0 => {
                assert!(parent.is_empty());
                Ok(DataRequestNode::Data {
                    data: json!({ "root": true }),
                    uses: None,
                    slash: None,
                })
            }
            2 => {
                assert_eq!(parent.get("root"), Some(&json!(true)));
                Ok(DataRequestNode::Data {
                    data: json!({ "page": true }),
                    uses: None,
                    slash: None,
                })
            }
            _ => panic!("unexpected node index {node_index}"),
        },
    )
    .expect("execute data request")
    .expect("page data response");
    assert_eq!(response.status, 200);
    assert!(
        response
            .body
            .as_deref()
            .is_some_and(|body| body.contains("\"type\":\"data\""))
    );

    let endpoint_url =
        Url::parse("https://example.com/api/__data.json").expect("valid endpoint data url");
    let endpoint_resolved =
        resolve_runtime_request(&manifest, &endpoint_url, "", |_matcher, _value| true)
            .expect("resolve endpoint request")
            .expect("resolved endpoint route");
    let endpoint_behavior =
        resolve_runtime_route_behavior(&manifest, endpoint_resolved.route, "/api", "");
    assert!(
        execute_data_request(
            &endpoint_resolved,
            &endpoint_behavior,
            &app_state(),
            |_node_index, _parent| { panic!("endpoint routes should not execute page data loads") }
        )
        .expect("execute endpoint data request")
        .is_none()
    );
}

#[test]
fn executes_data_requests_with_tracked_server_load_uses() {
    let cwd = temp_dir("execute-data-request-tracked-uses");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog/hello/__data.json?lang=en")
            .expect("valid data request"),
        headers: HeaderMap::new(),
    };
    let resolved = resolve_runtime_request(&manifest, &request.url, "", |_matcher, _value| true)
        .expect("resolve tracked request")
        .expect("resolved tracked route");
    let behavior = resolve_runtime_route_behavior(&manifest, resolved.route, "/blog/hello", "");
    let event = build_runtime_event(
        &request,
        Arc::new(app_state()),
        resolved.prepared.url.clone(),
        Some(resolved.route.id.clone()),
        resolved.params.clone(),
        true,
        false,
        0,
    );

    let response =
        execute_data_request(&resolved, &behavior, &app_state(), |node_index, parent| {
            load_server_data(
                &event,
                Some(resolved.route.id.as_str()),
                None,
                || Ok(parent.clone()),
                |context| {
                    let _ = node_index;
                    context.depends("/api/posts")?;
                    assert_eq!(context.param("slug").as_deref(), Some("hello"));
                    assert_eq!(context.search_param("lang").as_deref(), Some("en"));
                    assert_eq!(context.route_id().as_deref(), Some("/blog/[slug]"));
                    assert_eq!(context.url().path(), "/blog/hello");
                    let _ = context.parent()?;
                    Ok(Some(json!({ "post": true })))
                },
            )
        })
        .expect("execute tracked data request")
        .expect("tracked data response");

    let body: Value = serde_json::from_str(response.body.as_deref().expect("response body"))
        .expect("tracked data json");

    assert_eq!(body["type"], json!("data"));
    assert_eq!(body["nodes"][0]["data"], json!({ "post": true }));
    assert_eq!(
        body["nodes"][0]["uses"]["dependencies"],
        json!(["https://example.com/api/posts"])
    );
    assert_eq!(body["nodes"][0]["uses"]["params"], json!(["slug"]));
    assert_eq!(body["nodes"][0]["uses"]["search_params"], json!(["lang"]));
    assert_eq!(body["nodes"][0]["uses"]["parent"], json!(1));
    assert_eq!(body["nodes"][0]["uses"]["route"], json!(1));
    assert_eq!(body["nodes"][0]["uses"]["url"], json!(1));
}

#[test]
fn shapes_action_json_runtime_responses() {
    let success = action_json_response(
        &ActionJsonResult::Success {
            status: 200,
            data: Some(json!({ "ok": true })),
        },
        &app_state(),
    )
    .expect("success action json");
    assert_eq!(success.status, 200);
    assert_eq!(
        success.body.as_deref(),
        Some("{\"data\":{\"ok\":true},\"status\":200,\"type\":\"success\"}")
    );

    let failure = action_json_response(
        &ActionJsonResult::Failure {
            status: 422,
            data: json!({ "field": "missing" }),
        },
        &app_state(),
    )
    .expect("failure action json");
    assert_eq!(failure.status, 200);
    assert_eq!(
        failure.body.as_deref(),
        Some("{\"data\":{\"field\":\"missing\"},\"status\":422,\"type\":\"failure\"}")
    );

    let redirect = action_json_response(
        &ActionJsonResult::Redirect {
            status: 303,
            location: "/login".to_string(),
        },
        &app_state(),
    )
    .expect("redirect action json");
    assert_eq!(
        redirect.body.as_deref(),
        Some("{\"location\":\"/login\",\"status\":303,\"type\":\"redirect\"}")
    );

    let no_actions = no_actions_action_json_response(Some("/login"), true);
    assert_eq!(no_actions.status, 405);
    assert_eq!(no_actions.header("allow"), Some("GET"));
    assert!(
        no_actions
            .body
            .as_deref()
            .is_some_and(|body| body.contains("No form actions exist for the page at /login"))
    );

    assert_eq!(
        check_incorrect_fail_use("boom", true),
        "Cannot \"throw fail()\". Use \"return fail()\""
    );
    assert_eq!(check_incorrect_fail_use("boom", false), "boom");

    assert!(is_action_request(&ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/login").expect("valid url"),
        headers: HeaderMap::new(),
    }));
    assert!(!is_action_request(&ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/login").expect("valid url"),
        headers: HeaderMap::new(),
    }));

    assert_eq!(
        no_actions_action_request_result(Some("/login"), true),
        ActionRequestResult::Error {
            error: json!({
                "status": 405,
                "message": "POST method not allowed. No form actions exist for the page at /login",
                "allow": "GET",
            }),
        }
    );
}

#[test]
fn shapes_action_json_runtime_responses_with_transport_encoded_payloads() {
    let app_state = date_encoder_app_state();

    let success = action_json_response(
        &ActionJsonResult::Success {
            status: 200,
            data: Some(json!({
                "published": { "$type": "date", "value": "2026-03-12" }
            })),
        },
        &app_state,
    )
    .expect("transport success action json");
    assert_eq!(
        success.body.as_deref(),
        Some(
            "{\"data\":{\"published\":{\"kind\":\"date\",\"type\":\"Transport\",\"value\":\"2026-03-12\"}},\"status\":200,\"type\":\"success\"}"
        )
    );

    let failure = action_json_response(
        &ActionJsonResult::Failure {
            status: 422,
            data: json!({
                "published": { "$type": "date", "value": "2026-03-12" }
            }),
        },
        &app_state,
    )
    .expect("transport failure action json");
    assert_eq!(
        failure.body.as_deref(),
        Some(
            "{\"data\":{\"published\":{\"kind\":\"date\",\"type\":\"Transport\",\"value\":\"2026-03-12\"}},\"status\":422,\"type\":\"failure\"}"
        )
    );

    let error = action_json_response(
        &ActionJsonResult::Error {
            status: 500,
            error: json!({
                "details": { "$type": "date", "value": "2026-03-12" }
            }),
        },
        &app_state,
    )
    .expect("transport error action json");
    assert_eq!(
        error.body.as_deref(),
        Some(
            "{\"error\":{\"details\":{\"kind\":\"date\",\"type\":\"Transport\",\"value\":\"2026-03-12\"}},\"type\":\"error\"}"
        )
    );
}

#[test]
fn prepares_page_and_error_render_plans() {
    let cwd = temp_dir("page-render-plan");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("+layout.server.js"),
        "export const csr = false; export function load() {}",
    );
    write_file(&routes_dir.join("+error.svelte"), "<h1>error</h1>");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+page.js"),
        "export const prerender = true; export const ssr = false;",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .expect("blog route");

    let plan = prepare_page_render_plan(&manifest, route, "/blog").expect("page render plan");
    assert_eq!(
        plan,
        PageRenderPlan {
            ssr: false,
            csr: false,
            prerender: true,
            should_prerender_data: true,
            data_pathname: "/blog/__data.json".to_string(),
        }
    );
    assert!(!page_request_requires_shell_only(&plan, true));
    assert!(page_request_requires_shell_only(&plan, false));

    let error_plan = prepare_error_page_render(&manifest, 500, json!({ "message": "boom" }))
        .expect("error render plan");
    assert_eq!(
        error_plan,
        ErrorPageRenderPlan {
            status: 500,
            error: json!({ "message": "boom" }),
            ssr: true,
            csr: false,
            branch_node_indexes: vec![0, 1],
        }
    );
}

#[test]
fn applies_page_prerender_policy_and_shell_rendering() {
    let plan = PageRenderPlan {
        ssr: false,
        csr: true,
        prerender: true,
        should_prerender_data: false,
        data_pathname: "/blog/__data.json".to_string(),
    };
    let mut state = RuntimeRenderState::default();
    assert!(
        apply_page_prerender_policy(Some("/blog"), &plan, false, &mut state)
            .expect("prerender policy")
            .is_none()
    );
    assert!(state.prerender_default);

    let mut state = RuntimeRenderState {
        prerendering: Some(Default::default()),
        ..Default::default()
    };
    let non_prerenderable = PageRenderPlan {
        prerender: false,
        ..plan.clone()
    };
    let response =
        apply_page_prerender_policy(Some("/blog"), &non_prerenderable, false, &mut state)
            .expect("prerender policy")
            .expect("204 response");
    assert_eq!(response.status, 204);

    let mut state = RuntimeRenderState {
        prerendering: Some(Default::default()),
        depth: 1,
        ..Default::default()
    };
    let error = apply_page_prerender_policy(Some("/blog"), &non_prerenderable, false, &mut state)
        .expect_err("nested non-prerenderable request should fail");
    assert!(matches!(
        error,
        Error::RuntimePage(RuntimePageError::NotPrerenderable { ref route_id })
            if route_id == "/blog"
    ));
    assert_eq!(error.to_string(), "/blog is not prerenderable");

    let error = apply_page_prerender_policy(
        Some("/blog"),
        &plan,
        true,
        &mut RuntimeRenderState::default(),
    )
    .expect_err("prerendered action page should fail");
    assert!(matches!(
        error,
        Error::RuntimePage(RuntimePageError::PrerenderActions)
    ));
    assert_eq!(error.to_string(), "Cannot prerender pages with actions");

    assert_eq!(
        render_shell_page_response(200, true),
        ShellPageResponse {
            status: 200,
            ssr: false,
            csr: true,
            action: None,
            effects: Default::default(),
        }
    );

    let mut state = RuntimeRenderState::default();
    assert_eq!(
        resolve_page_runtime_decision(Some("/blog"), plan.clone(), false, &mut state, 200)
            .expect("page runtime decision"),
        PageRuntimeDecision::Shell(ShellPageResponse {
            status: 200,
            ssr: false,
            csr: true,
            action: None,
            effects: Default::default(),
        })
    );

    let mut state = RuntimeRenderState::default();
    let shell_plan = PageRenderPlan {
        ssr: false,
        csr: true,
        prerender: false,
        should_prerender_data: false,
        data_pathname: "/blog/__data.json".to_string(),
    };
    assert_eq!(
        resolve_page_runtime_decision(Some("/blog"), shell_plan, false, &mut state, 200)
            .expect("shell decision"),
        PageRuntimeDecision::Shell(ShellPageResponse {
            status: 200,
            ssr: false,
            csr: true,
            action: None,
            effects: Default::default(),
        })
    );

    let mut state = RuntimeRenderState {
        prerendering: Some(Default::default()),
        ..Default::default()
    };
    let early_plan = PageRenderPlan {
        prerender: false,
        ..plan
    };
    let PageRuntimeDecision::Early(response) =
        resolve_page_runtime_decision(Some("/blog"), early_plan, false, &mut state, 200)
            .expect("early decision")
    else {
        panic!("expected early prerender response");
    };
    assert_eq!(response.status, 204);
}

#[test]
fn executes_page_load_branch_with_parent_merging() {
    let cwd = temp_dir("execute-page-load");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("blog").join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("blog route");

    let result =
        execute_page_load(
            &manifest,
            route,
            |node_index, server_parent, parent| match node_index {
                0 => {
                    assert!(server_parent.is_empty());
                    assert!(parent.is_empty());
                    Ok(PageLoadResult::Loaded {
                        server_data: Some(json!({ "root_server": true })),
                        data: Some(json!({ "root": true })),
                    })
                }
                2 => {
                    assert_eq!(server_parent.get("root_server"), Some(&json!(true)));
                    assert_eq!(parent.get("root"), Some(&json!(true)));
                    Ok(PageLoadResult::Loaded {
                        server_data: Some(json!({ "blog_server": true })),
                        data: Some(json!({ "blog": true })),
                    })
                }
                3 => {
                    assert_eq!(server_parent.get("root_server"), Some(&json!(true)));
                    assert_eq!(server_parent.get("blog_server"), Some(&json!(true)));
                    assert_eq!(parent.get("root"), Some(&json!(true)));
                    assert_eq!(parent.get("blog"), Some(&json!(true)));
                    Ok(PageLoadResult::Loaded {
                        server_data: None,
                        data: Some(json!({ "page": true })),
                    })
                }
                _ => panic!("unexpected node index {node_index}"),
            },
        )
        .expect("execute page load");

    assert_eq!(
        result,
        PageExecutionResult::Rendered {
            branch: vec![
                PageLoadedNode {
                    node_index: 0,
                    server_data: Some(json!({ "root_server": true })),
                    data: Some(json!({ "root": true })),
                },
                PageLoadedNode {
                    node_index: 2,
                    server_data: Some(json!({ "blog_server": true })),
                    data: Some(json!({ "blog": true })),
                },
                PageLoadedNode {
                    node_index: 3,
                    server_data: None,
                    data: Some(json!({ "page": true })),
                },
            ],
        }
    );
}

#[test]
fn returns_redirects_and_nearest_page_error_boundaries() {
    let cwd = temp_dir("execute-page-load-boundary");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("+error.svelte"), "<h1>root error</h1>");
    write_file(&routes_dir.join("blog").join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+layout.js"),
        "export const csr = false;",
    );
    write_file(
        &routes_dir.join("blog").join("+error.svelte"),
        "<h1>blog error</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("blog route");

    let redirect = execute_page_load(&manifest, route, |node_index, _, _| match node_index {
        0 => Ok(PageLoadResult::Loaded {
            server_data: None,
            data: Some(json!({ "root": true })),
        }),
        2 => Ok(PageLoadResult::Redirect {
            status: 307,
            location: "/login".to_string(),
        }),
        _ => panic!("unexpected node index {node_index}"),
    })
    .expect("execute page load");
    let PageExecutionResult::Redirect(response) = redirect else {
        panic!("expected redirect result");
    };
    assert_eq!(response.status, 307);
    assert_eq!(response.header("location"), Some("/login"));

    let boundary = execute_page_load(&manifest, route, |node_index, _, _| match node_index {
        0 => Ok(PageLoadResult::Loaded {
            server_data: None,
            data: Some(json!({ "root": true })),
        }),
        2 => Ok(PageLoadResult::Loaded {
            server_data: None,
            data: Some(json!({ "blog": true })),
        }),
        4 => Ok(PageLoadResult::Error {
            status: 503,
            error: json!({ "message": "broken" }),
        }),
        _ => panic!("unexpected node index {node_index}"),
    })
    .expect("execute page load");

    assert_eq!(
        boundary,
        PageExecutionResult::ErrorBoundary(PageErrorBoundary {
            status: 503,
            error: json!({ "message": "broken" }),
            branch: vec![
                PageLoadedNode {
                    node_index: 0,
                    server_data: None,
                    data: Some(json!({ "root": true })),
                },
                PageLoadedNode {
                    node_index: 2,
                    server_data: None,
                    data: Some(json!({ "blog": true })),
                },
            ],
            error_node_index: 3,
            ssr: true,
            csr: false,
            action: None,
            effects: Default::default(),
        })
    );
}

#[test]
fn treats_root_layout_page_load_failures_as_fatal() {
    let cwd = temp_dir("execute-page-load-fatal");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("+error.svelte"), "<h1>root error</h1>");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .expect("blog route");

    let result = execute_page_load(&manifest, route, |node_index, _, _| match node_index {
        0 => Ok(PageLoadResult::Error {
            status: 500,
            error: json!({ "message": "root exploded" }),
        }),
        _ => panic!("unexpected node index {node_index}"),
    })
    .expect("execute page load");

    assert_eq!(
        result,
        PageExecutionResult::Fatal {
            status: 500,
            error: json!({ "message": "root exploded" }),
        }
    );
}

#[test]
fn rejects_non_object_page_load_results() {
    let cwd = temp_dir("execute-page-load-invalid-shape");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .expect("blog route");

    let error = execute_page_load(&manifest, route, |node_index, _, _| match node_index {
        0 => Ok(PageLoadResult::Loaded {
            server_data: None,
            data: Some(json!({ "layout": true })),
        }),
        2 => Ok(PageLoadResult::Loaded {
            server_data: None,
            data: Some(json!(["bad"])),
        }),
        _ => panic!("unexpected node index {node_index}"),
    })
    .expect_err("invalid load shape should fail");

    assert_eq!(
        error.to_string(),
        "a load function while rendering /blog (node 2) returned an array, but must return a plain object at the top level (i.e. `return {...}`)"
    );
}

#[test]
fn executes_page_requests_across_shell_render_and_error_boundary_paths() {
    let cwd = temp_dir("execute-page-request");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("spa").join("+page.svelte"), "<h1>spa</h1>");
    write_file(
        &routes_dir.join("spa").join("+page.js"),
        "export const ssr = false; export const csr = true;",
    );
    write_file(&routes_dir.join("blog").join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+error.svelte"),
        "<h1>blog error</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");

    let spa_route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/spa")
        .expect("spa route");
    let shell = execute_page_request(
        &manifest,
        spa_route,
        "/spa",
        false,
        &mut RuntimeRenderState::default(),
        200,
        |_node_index, _, _| panic!("shell pages should not load on the server"),
    )
    .expect("execute page request");
    assert_eq!(
        shell,
        PageRequestResult::Shell(ShellPageResponse {
            status: 200,
            ssr: false,
            csr: true,
            action: None,
            effects: Default::default(),
        })
    );

    let blog_route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("blog route");
    let blog_page = blog_route.page.as_ref().expect("blog page");
    let root_layout = blog_page.layouts[0].expect("root layout");
    let blog_layout = blog_page.layouts[1].expect("blog layout");
    let blog_leaf = blog_page.leaf;
    let blog_error = blog_page.errors[1].expect("blog error boundary");
    let rendered = execute_page_request(
        &manifest,
        blog_route,
        "/blog/hello",
        false,
        &mut RuntimeRenderState::default(),
        200,
        |node_index, _, _| match node_index {
            node if node == root_layout => Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "root": true })),
            }),
            node if node == blog_layout => Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "blog": true })),
            }),
            node if node == blog_leaf => Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            }),
            _ => panic!("unexpected node index {node_index}"),
        },
    )
    .expect("execute page request");
    assert_eq!(
        rendered,
        PageRequestResult::Rendered(RenderedPage {
            plan: PageRenderPlan {
                ssr: true,
                csr: true,
                prerender: false,
                should_prerender_data: false,
                data_pathname: "/blog/hello/__data.json".to_string(),
            },
            branch: vec![
                PageLoadedNode {
                    node_index: root_layout,
                    server_data: None,
                    data: Some(json!({ "root": true })),
                },
                PageLoadedNode {
                    node_index: blog_layout,
                    server_data: None,
                    data: Some(json!({ "blog": true })),
                },
                PageLoadedNode {
                    node_index: blog_leaf,
                    server_data: None,
                    data: Some(json!({ "page": true })),
                },
            ],
            action: None,
            effects: Default::default(),
        })
    );

    let boundary = execute_page_request(
        &manifest,
        blog_route,
        "/blog/hello",
        false,
        &mut RuntimeRenderState::default(),
        200,
        |node_index, _, _| match node_index {
            node if node == root_layout => Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "root": true })),
            }),
            node if node == blog_layout => Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "blog": true })),
            }),
            node if node == blog_leaf => Ok(PageLoadResult::Error {
                status: 500,
                error: json!({ "message": "boom" }),
            }),
            _ => panic!("unexpected node index {node_index}"),
        },
    )
    .expect("execute page request");
    assert_eq!(
        boundary,
        PageRequestResult::ErrorBoundary(PageErrorBoundary {
            status: 500,
            error: json!({ "message": "boom" }),
            branch: vec![
                PageLoadedNode {
                    node_index: root_layout,
                    server_data: None,
                    data: Some(json!({ "root": true })),
                },
                PageLoadedNode {
                    node_index: blog_layout,
                    server_data: None,
                    data: Some(json!({ "blog": true })),
                },
            ],
            error_node_index: blog_error,
            ssr: true,
            csr: true,
            action: None,
            effects: Default::default(),
        })
    );
}

#[test]
fn executes_error_page_loads_from_root_layout_only() {
    let cwd = temp_dir("execute-error-page-load");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("+layout.server.js"),
        "export const csr = false; export function load() {}",
    );
    write_file(&routes_dir.join("+error.svelte"), "<h1>error</h1>");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");

    let executed = execute_error_page_load(
        &manifest,
        500,
        json!({ "message": "boom" }),
        |node_index, server_parent, parent| {
            assert_eq!(node_index, 0);
            assert!(server_parent.is_empty());
            assert!(parent.is_empty());
            Ok(PageLoadResult::Loaded {
                server_data: Some(json!({ "root_server": true })),
                data: Some(json!({ "root": true })),
            })
        },
    )
    .expect("execute error page load");

    assert_eq!(
        executed,
        ExecutedErrorPage {
            plan: ErrorPageRenderPlan {
                status: 500,
                error: json!({ "message": "boom" }),
                ssr: true,
                csr: false,
                branch_node_indexes: vec![0, 1],
            },
            branch: vec![
                PageLoadedNode {
                    node_index: 0,
                    server_data: Some(json!({ "root_server": true })),
                    data: Some(json!({ "root": true })),
                },
                PageLoadedNode {
                    node_index: 1,
                    server_data: None,
                    data: None,
                },
            ],
        }
    );
}

#[test]
fn executes_error_page_requests_for_render_redirect_and_static_paths() {
    let cwd = temp_dir("execute-error-page-request");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("+error.svelte"), "<h1>error</h1>");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");

    let rendered = execute_error_page_request(
        &manifest,
        500,
        json!({ "message": "boom" }),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |node_index, server_parent, parent| {
            assert_eq!(node_index, 0);
            assert!(server_parent.is_empty());
            assert!(parent.is_empty());
            Ok(PageLoadResult::Loaded {
                server_data: Some(json!({ "root_server": true })),
                data: Some(json!({ "root": true })),
            })
        },
    )
    .expect("execute error page request");
    assert_eq!(
        rendered,
        ErrorPageRequestResult::Rendered(ExecutedErrorPage {
            plan: ErrorPageRenderPlan {
                status: 500,
                error: json!({ "message": "boom" }),
                ssr: true,
                csr: true,
                branch_node_indexes: vec![0, 1],
            },
            branch: vec![
                PageLoadedNode {
                    node_index: 0,
                    server_data: Some(json!({ "root_server": true })),
                    data: Some(json!({ "root": true })),
                },
                PageLoadedNode {
                    node_index: 1,
                    server_data: None,
                    data: None,
                },
            ],
        })
    );

    let redirect = execute_error_page_request(
        &manifest,
        500,
        json!({ "message": "boom" }),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Redirect {
                status: 307,
                location: "/login".to_string(),
            })
        },
    )
    .expect("execute error page redirect");
    let ErrorPageRequestResult::Redirect(redirect) = redirect else {
        panic!("expected redirect result");
    };
    assert_eq!(redirect.status, 307);
    assert_eq!(redirect.header("location"), Some("/login"));

    let static_page = execute_error_page_request(
        &manifest,
        500,
        json!({ "message": "boom" }),
        true,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_node_index, _server_parent, _parent| {
            panic!("recursive error handling should skip layout loading")
        },
    )
    .expect("execute static error page");
    let ErrorPageRequestResult::Static(static_page) = static_page else {
        panic!("expected static error page");
    };
    assert_eq!(static_page.status, 500);
    assert_eq!(static_page.body.as_deref(), Some("<h1>500:boom</h1>"));
}

#[test]
fn creates_server_route_resolution_response() {
    let cwd = temp_dir("server-routing-response");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest = KitManifest::discover(&ManifestConfig::new(routes_dir, cwd.clone()))
        .expect("discover manifest");
    let route: ClientRoute = manifest
        .build_client_routes()
        .into_iter()
        .find(|route| route.id == "/blog/[slug]")
        .expect("client route");
    let params = std::collections::BTreeMap::from([("slug".to_string(), "hello".to_string())]);
    let assets = RouteResolutionAssets {
        base: "/base".to_string(),
        assets: "".to_string(),
        relative: false,
        start: Some("_app/immutable/start.js".to_string()),
        nodes: vec![
            Some("_app/immutable/nodes/0.js".to_string()),
            Some("_app/immutable/nodes/1.js".to_string()),
        ],
        css: std::collections::BTreeMap::from([
            (0, vec!["_app/immutable/assets/0.css".to_string()]),
            (1, vec!["_app/immutable/assets/1.css".to_string()]),
        ]),
    };

    let response = create_server_routing_response(
        Some(&route),
        &params,
        &Url::parse("https://example.com/base/blog/hello/__route.js").expect("valid route url"),
        &assets,
    );

    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type"),
        Some("application/javascript; charset=utf-8")
    );
    let body = response.body.expect("route body");
    assert!(body.contains("load_css"));
    assert!(body.contains("export const route ="));
    assert!(body.contains("\"/blog/[slug]\""));
    assert!(body.contains("\"slug\":\"hello\""));
    assert!(body.contains("import(\"/base/_app/immutable/start.js\")"));
    assert!(body.contains("'1': () => import(\"/base/_app/immutable/nodes/1.js\")"));
}

#[test]
fn resolves_route_request_urls_to_server_route_response() {
    let cwd = temp_dir("resolve-route-request-response");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest = KitManifest::discover(&ManifestConfig::new(routes_dir, cwd.clone()))
        .expect("discover manifest");
    let assets = RouteResolutionAssets {
        base: "/base".to_string(),
        assets: "".to_string(),
        relative: false,
        start: Some("_app/immutable/start.js".to_string()),
        nodes: vec![
            Some("_app/immutable/nodes/0.js".to_string()),
            Some("_app/immutable/nodes/1.js".to_string()),
        ],
        css: Default::default(),
    };

    let response = resolve_route_request_response(
        &manifest,
        &Url::parse("https://example.com/base/blog/hello/__route.js").expect("valid route request"),
        "/base",
        &assets,
        |_, _| true,
    )
    .expect("route response");
    assert_eq!(response.status, 200);
    let body = response.body.expect("route response body");
    assert!(body.contains("\"/blog/[slug]\""));
    assert!(body.contains("\"slug\":\"hello\""));

    let missing = resolve_route_request_response(
        &manifest,
        &Url::parse("https://example.com/base/missing/__route.js")
            .expect("valid missing route request"),
        "/base",
        &assets,
        |_, _| true,
    )
    .expect("missing route response");
    assert_eq!(missing.status, 200);
    assert_eq!(missing.body.as_deref(), Some(""));
}

#[test]
fn serves_public_env_module_with_etag_handling() {
    let public_env = Map::from_iter([("PUBLIC_FOO".to_string(), Value::String("bar".to_string()))]);
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/_app/env.js").expect("valid env url"),
        headers: HeaderMap::new(),
    };

    let response = public_env_response(&request, &public_env);
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("content-type"),
        Some("application/javascript; charset=utf-8")
    );
    let etag = response
        .header("etag")
        .map(str::to_string)
        .expect("etag header");
    assert_eq!(
        response.body.as_deref(),
        Some("export const env={\"PUBLIC_FOO\":\"bar\"}")
    );

    let cached_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/_app/env.js").expect("valid env url"),
        headers: header_map([("if-none-match".to_string(), etag)]),
    };
    let cached = public_env_response(&cached_request, &public_env);
    assert_eq!(cached.status, 304);
    assert_eq!(cached.body, None);
}

#[test]
fn rejects_cross_site_form_submissions_like_upstream() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/form").expect("valid form url"),
        headers: header_map([
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("origin".to_string(), "https://evil.example".to_string()),
            ("accept".to_string(), "application/json".to_string()),
        ]),
    };

    let response = check_csrf(&request, true, &["https://trusted.example".to_string()])
        .expect("csrf response");
    assert_eq!(response.status, 403);
    assert_eq!(
        response.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(
        response.body.as_deref(),
        Some("{\"message\":\"Cross-site POST form submissions are forbidden\"}")
    );

    let trusted = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/form").expect("valid form url"),
        headers: header_map([
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("origin".to_string(), "https://trusted.example".to_string()),
        ]),
    };
    assert!(check_csrf(&trusted, true, &["https://trusted.example".to_string()]).is_none());

    let similar = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/form").expect("valid form url"),
        headers: header_map([
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            (
                "origin".to_string(),
                "https://trusted.example.evil.com".to_string(),
            ),
        ]),
    };
    let similar_response = check_csrf(&similar, true, &["https://trusted.example".to_string()])
        .expect("similar origin should be blocked");
    assert_eq!(similar_response.status, 403);

    let subdomain = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/form").expect("valid form url"),
        headers: header_map([
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            (
                "origin".to_string(),
                "https://evil.trusted.example".to_string(),
            ),
        ]),
    };
    let subdomain_response = check_csrf(&subdomain, true, &["https://trusted.example".to_string()])
        .expect("subdomain attack should be blocked");
    assert_eq!(subdomain_response.status, 403);

    let get = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/form").expect("valid form url"),
        headers: header_map([
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("origin".to_string(), "https://evil.example".to_string()),
        ]),
    };
    assert!(check_csrf(&get, true, &[]).is_none());

    let json = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/form").expect("valid form url"),
        headers: header_map([
            ("content-type".to_string(), "application/json".to_string()),
            ("origin".to_string(), "https://evil.example".to_string()),
        ]),
    };
    assert!(check_csrf(&json, true, &[]).is_none());
}

#[test]
fn dispatches_special_runtime_requests_like_upstream() {
    let cwd = temp_dir("dispatch-special-runtime-request");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest = KitManifest::discover(&ManifestConfig::new(routes_dir, cwd.clone()))
        .expect("discover manifest");
    let assets = RouteResolutionAssets {
        base: "/base".to_string(),
        assets: "".to_string(),
        relative: false,
        start: Some("_app/immutable/start.js".to_string()),
        nodes: vec![
            Some("_app/immutable/nodes/0.js".to_string()),
            Some("_app/immutable/nodes/1.js".to_string()),
        ],
        css: Default::default(),
    };
    let public_env = Map::from_iter([("PUBLIC_FOO".to_string(), Value::String("bar".to_string()))]);
    let options = SpecialRuntimeRequestOptions {
        app_dir: "_app".to_string(),
        base: "/base".to_string(),
        hash_routing: false,
        public_env: &public_env,
        route_assets: &assets,
    };

    let env_request = dispatch_special_runtime_request(
        &manifest,
        &ServerRequest {
            method: Method::GET,
            url: Url::parse("https://example.com/base/_app/env.js").expect("valid env url"),
            headers: HeaderMap::new(),
        },
        &options,
        |_, _| true,
    )
    .expect("env dispatch")
    .expect("env response");
    assert_eq!(env_request.status, 200);
    assert_eq!(
        env_request.body.as_deref(),
        Some("export const env={\"PUBLIC_FOO\":\"bar\"}")
    );

    let route_request = dispatch_special_runtime_request(
        &manifest,
        &ServerRequest {
            method: Method::GET,
            url: Url::parse("https://example.com/base/blog/hello/__route.js")
                .expect("valid route url"),
            headers: HeaderMap::new(),
        },
        &options,
        |_, _| true,
    )
    .expect("route dispatch")
    .expect("route response");
    assert_eq!(route_request.status, 200);
    assert!(
        route_request
            .body
            .as_deref()
            .expect("route body")
            .contains("\"/blog/[slug]\"")
    );

    let missing_asset = dispatch_special_runtime_request(
        &manifest,
        &ServerRequest {
            method: Method::GET,
            url: Url::parse("https://example.com/base/_app/missing.js").expect("valid asset url"),
            headers: HeaderMap::new(),
        },
        &options,
        |_, _| true,
    )
    .expect("asset dispatch")
    .expect("asset 404");
    assert_eq!(missing_asset.status, 404);
    assert_eq!(
        missing_asset.header("cache-control"),
        Some("public, max-age=0, must-revalidate")
    );

    let hash_options = SpecialRuntimeRequestOptions {
        app_dir: "_app".to_string(),
        base: "/base".to_string(),
        hash_routing: true,
        public_env: &public_env,
        route_assets: &assets,
    };
    let hash_not_found = dispatch_special_runtime_request(
        &manifest,
        &ServerRequest {
            method: Method::GET,
            url: Url::parse("https://example.com/base/blog/hello").expect("valid hash url"),
            headers: HeaderMap::new(),
        },
        &hash_options,
        |_, _| true,
    )
    .expect("hash dispatch")
    .expect("hash 404");
    assert_eq!(hash_not_found.status, 404);
    assert_eq!(hash_not_found.body.as_deref(), Some("Not found"));
}

#[test]
fn detects_prerendered_paths_like_upstream() {
    let prerendered =
        std::collections::BTreeSet::from(["/base".to_string(), "/base/about".to_string()]);

    assert!(svelte_kit::has_prerendered_path(&prerendered, "/base"));
    assert!(svelte_kit::has_prerendered_path(
        &prerendered,
        "/base/about/"
    ));
    assert!(!svelte_kit::has_prerendered_path(
        &prerendered,
        "/base/missing"
    ));
}

#[test]
fn handles_fatal_errors_as_json_or_html_like_upstream() {
    let html_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/fail").expect("valid fail url"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };
    let html_response = handle_fatal_error(
        &html_request,
        false,
        &RuntimeFatalError::Kit {
            status: 418,
            text: "teapot".to_string(),
        },
        |status, message| format!("<html><head></head><body>{status}:{message}</body></html>"),
        false,
    );
    assert_eq!(html_response.status, 418);
    assert_eq!(
        html_response.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        html_response.body.as_deref(),
        Some("<html><head></head><body>418:teapot</body></html>")
    );

    let data_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/fail").expect("valid fail url"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };
    let data_response = handle_fatal_error(
        &data_request,
        true,
        &RuntimeFatalError::Http {
            status: 400,
            body: Map::from_iter([("code".to_string(), Value::String("bad_request".to_string()))]),
        },
        |status, message| format!("<html><head></head><body>{status}:{message}</body></html>"),
        false,
    );
    assert_eq!(data_response.status, 400);
    assert_eq!(
        data_response.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(
        data_response.body.as_deref(),
        Some("{\"code\":\"bad_request\",\"message\":\"Unknown Error\"}")
    );
}

#[test]
fn formats_server_errors_like_upstream() {
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/missing").expect("valid url"),
        headers: HeaderMap::new(),
    };

    let not_found = format_server_error(404, None, &request, false);
    assert!(not_found.contains("[404] GET /missing"));
    assert!(!not_found.contains("stack line"));

    let error = format_server_error(500, Some("stack line 1\nstack line 2"), &request, false);
    assert!(error.contains("[500] GET /missing"));
    assert!(error.contains("stack line 1"));
}

#[test]
fn applies_response_header_rules_like_upstream() {
    let mut headers = HeaderMap::new();
    let mut prerender = svelte_kit::PrerenderState::default();

    set_response_header(
        &mut headers,
        "cache-control",
        "public, max-age=30",
        Some(&mut prerender),
    )
    .expect("set cache-control");
    set_response_header(
        &mut headers,
        "server-timing",
        "db;dur=1",
        Some(&mut prerender),
    )
    .expect("set first server-timing");
    set_response_header(
        &mut headers,
        "server-timing",
        "app;dur=2",
        Some(&mut prerender),
    )
    .expect("append server-timing");

    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=30")
    );
    assert_eq!(
        headers
            .get("server-timing")
            .and_then(|value| value.to_str().ok()),
        Some("db;dur=1, app;dur=2")
    );
    assert_eq!(prerender.cache.as_deref(), Some("public, max-age=30"));

    let duplicate = set_response_header(
        &mut headers,
        "content-type",
        "text/plain; charset=utf-8",
        Some(&mut prerender),
    )
    .expect("first content-type");
    assert_eq!(duplicate, ());
    let err = set_response_header(
        &mut headers,
        "content-type",
        "application/json",
        Some(&mut prerender),
    )
    .expect_err("duplicate non-server-timing should error");
    assert!(matches!(
        err,
        Error::RuntimeHttp(RuntimeHttpError::DuplicateHeader { .. })
    ));
    assert_eq!(err.to_string(), "\"content-type\" header is already set");

    let cookie_err = set_response_header(&mut headers, "set-cookie", "a=1", Some(&mut prerender))
        .expect_err("set-cookie should be rejected");
    assert!(matches!(
        cookie_err,
        Error::RuntimeHttp(RuntimeHttpError::SetCookieViaHeaders)
    ));
    assert_eq!(
        cookie_err.to_string(),
        "Use `event.cookies.set(name, value, options)` instead of `event.setHeaders` to set cookies"
    );
}

#[test]
fn handles_page_route_methods_like_upstream() {
    let options = page_method_response(&Method::OPTIONS, true).expect("options response");
    assert_eq!(options.status, 204);
    assert_eq!(options.header("allow"), Some("GET, HEAD, OPTIONS, POST"));

    let no_actions = page_method_response(&Method::DELETE, false).expect("405 response");
    assert_eq!(no_actions.status, 405);
    assert_eq!(no_actions.header("allow"), Some("GET, HEAD, OPTIONS"));
    assert_eq!(
        no_actions.body.as_deref(),
        Some("DELETE method not allowed")
    );
}

#[test]
fn resolves_runtime_requests_with_decoded_params_like_upstream() {
    let cwd = temp_dir("resolve-runtime-request-decoded-params");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest = KitManifest::discover(&ManifestConfig::new(routes_dir, cwd.clone()))
        .expect("discover manifest");

    let resolved = resolve_runtime_request(
        &manifest,
        &Url::parse("https://example.com/blog/a%252Fb%2520c").expect("valid encoded url"),
        "",
        |_, _| true,
    )
    .expect("resolved request")
    .expect("matched route");

    assert_eq!(
        resolved.params.get("slug").map(String::as_str),
        Some("a/b c")
    );
}

#[test]
fn serializes_server_data_uses_and_node_type_like_upstream() {
    let uses = ServerDataUses {
        dependencies: std::collections::BTreeSet::from(["/api/foo".to_string()]),
        search_params: std::collections::BTreeSet::from(["q".to_string()]),
        params: std::collections::BTreeSet::from(["slug".to_string()]),
        parent: true,
        route: true,
        url: false,
    };
    let node = ServerDataNode { uses: Some(uses) };
    let serialized = serialize_uses(&node);
    assert_eq!(serialized.get("dependencies"), Some(&json!(["/api/foo"])));
    assert_eq!(serialized.get("search_params"), Some(&json!(["q"])));
    assert_eq!(serialized.get("params"), Some(&json!(["slug"])));
    assert_eq!(serialized.get("parent"), Some(&json!(1)));
    assert_eq!(serialized.get("route"), Some(&json!(1)));
    assert_eq!(serialized.get("url"), None);

    assert_eq!(
        get_node_type(Some("routes/blog/+page.server.js")),
        "+page.server"
    );
    assert_eq!(get_node_type(Some("routes/+layout.svelte")), "+layout");
    assert_eq!(get_node_type(None), "");
}

#[test]
fn clarifies_devalue_errors_like_upstream() {
    let with_path = clarify_devalue_error(
        "/blog/[slug]",
        &RuntimeDevalueError {
            message: "Cannot stringify arbitrary non-POJOs".to_string(),
            path: Some(".foo.bar".to_string()),
        },
    );
    assert!(with_path.contains("while rendering /blog/[slug]"));
    assert!(with_path.contains("(.foo.bar)"));

    let top_level = clarify_devalue_error(
        "/blog/[slug]",
        &RuntimeDevalueError {
            message: "Cannot stringify arbitrary non-POJOs".to_string(),
            path: Some(String::new()),
        },
    );
    assert_eq!(
        top_level,
        "Data returned from `load` while rendering /blog/[slug] is not a plain object"
    );
}

#[test]
fn appends_serialized_cookies_to_response_headers() {
    let url = Url::parse("https://example.com/blog/post.html").expect("valid url");
    let mut jar = CookieJar::new(None, &url);
    jar.set_trailing_slash("never").expect("set trailing slash");
    jar.set(
        "session",
        "abc",
        CookieOptions {
            path: Some("/blog/post.html".to_string()),
            ..Default::default()
        },
    )
    .expect("set cookie");

    let mut response = ServerResponse::new(200);
    set_response_cookies(&mut response, &jar);

    let set_cookie = response
        .header_values("set-cookie")
        .expect("set-cookie headers");
    assert_eq!(set_cookie.len(), 2);
    assert!(set_cookie[0].starts_with("session=abc;"));
    assert!(set_cookie[1].contains("/blog/post.html__data.json"));
}

#[test]
fn detects_remote_request_ids_and_rejects_cross_site_posts() {
    let url =
        Url::parse("https://example.com/base/_app/remote/hash/name/arg").expect("valid remote url");
    assert_eq!(
        get_remote_id(&url, "/base", "_app"),
        Some("hash/name/arg".to_string())
    );
    assert_eq!(
        get_remote_action(
            &Url::parse("https://example.com/blog?/remote=hash/name/action")
                .expect("valid remote action url")
        ),
        Some("hash/name/action".to_string())
    );
    assert_eq!(get_remote_action(&url), None);

    let request = ServerRequest {
        method: Method::POST,
        url,
        headers: header_map([("origin".to_string(), "https://evil.example".to_string())]),
    };
    let response = check_remote_request_origin(&request, Some("hash/name/arg"))
        .expect("cross-site remote should be rejected");
    assert_eq!(response.status, 403);
    assert_eq!(
        response.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(
        response.body.as_deref(),
        Some("{\"message\":\"Cross-site remote requests are forbidden\"}")
    );
}

#[test]
fn rewrites_remote_requests_from_forwarded_path_headers() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/base/_app/remote/hash/name/arg?payload=1")
            .expect("valid remote url"),
        headers: header_map([
            (
                "x-sveltekit-pathname".to_string(),
                "/base/blog/hello".to_string(),
            ),
            ("x-sveltekit-search".to_string(), "?q=1".to_string()),
        ]),
    };

    let resolved = resolve_remote_request_url(&request, "/base", Some("hash/name/arg"))
        .expect("rewritten remote url");
    assert_eq!(resolved.path(), "/base/blog/hello");
    assert_eq!(resolved.query(), Some("q=1"));

    let untouched = resolve_remote_request_url(&request, "/base", None).expect("untouched url");
    assert_eq!(untouched.path(), "/base/_app/remote/hash/name/arg");
    assert_eq!(untouched.query(), Some("payload=1"));
}

#[test]
fn preprocesses_runtime_requests_into_early_response_or_route_context() {
    let cwd = temp_dir("preprocess-runtime-request");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("blog").join("[slug]").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest = KitManifest::discover(&ManifestConfig::new(routes_dir, cwd.clone()))
        .expect("discover manifest");
    let public_env = Map::from_iter([("PUBLIC_FOO".to_string(), Value::String("bar".to_string()))]);
    let assets = RouteResolutionAssets {
        base: "/base".to_string(),
        ..Default::default()
    };
    let options = RuntimeRequestOptions {
        base: "/base".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec!["https://trusted.example".to_string()],
        public_env,
        route_assets: assets,
    };

    let env_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/base/_app/env.js").expect("valid env request"),
        headers: HeaderMap::new(),
    };
    let env = preprocess_runtime_request(
        &manifest,
        &env_request,
        &options,
        &RuntimeRenderState::default(),
        |_, _| true,
    )
    .expect("preprocess env request");
    assert!(env.early_response.is_some());
    assert!(env.resolved.is_none());

    let route_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/base/blog/hello").expect("valid route request"),
        headers: HeaderMap::new(),
    };
    let route = preprocess_runtime_request(
        &manifest,
        &route_request,
        &options,
        &RuntimeRenderState::default(),
        |_, _| true,
    )
    .expect("preprocess route request");
    assert!(route.early_response.is_none());
    let resolved = route.resolved.expect("resolved route");
    assert_eq!(resolved.route.id, "/blog/[slug]");
    assert_eq!(
        resolved.params.get("slug").map(String::as_str),
        Some("hello")
    );
    assert_eq!(route.remote_id, None);
}

#[test]
fn prepares_and_executes_runtime_requests_across_endpoint_page_and_early_paths() {
    let cwd = temp_dir("prepare-runtime-execution");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+server.js"),
        "export function GET() {}",
    );
    write_file(&routes_dir.join("spa").join("+page.svelte"), "<h1>spa</h1>");
    write_file(
        &routes_dir.join("spa").join("+page.js"),
        "export const ssr = false; export const csr = true;",
    );

    let manifest = KitManifest::discover(&ManifestConfig::new(routes_dir, cwd.clone()))
        .expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };

    let early_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/_app/env.js").expect("valid early url"),
        headers: HeaderMap::new(),
    };
    let early = prepare_runtime_execution(
        &manifest,
        &early_request,
        &options,
        &RuntimeRenderState::default(),
        0,
        false,
        |_, _| true,
    )
    .expect("prepare early execution");
    assert!(early.preprocessed.early_response.is_some());
    assert!(early.behavior.is_none());
    assert!(early.dispatch.is_none());
    assert!(early.event.is_none());
    assert_eq!(
        execute_prepared_runtime_request(
            &early,
            &mut RuntimeRenderState::default(),
            None,
            |_resolved, _behavior, _event| panic!("early response should short-circuit"),
            |_resolved, _behavior, _event, _state| panic!("early response should short-circuit"),
        )
        .expect("execute early response"),
        RuntimeExecutionResult::Response(
            early
                .preprocessed
                .early_response
                .clone()
                .expect("early response")
        )
    );

    let endpoint_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid endpoint url"),
        headers: header_map([("accept".to_string(), "application/json".to_string())]),
    };
    let prepared_endpoint = prepare_runtime_execution(
        &manifest,
        &endpoint_request,
        &options,
        &RuntimeRenderState::default(),
        0,
        false,
        |_, _| true,
    )
    .expect("prepare endpoint execution");
    assert_eq!(
        prepared_endpoint.dispatch,
        Some(RuntimeRouteDispatch::Endpoint)
    );
    let endpoint_response = execute_prepared_runtime_request(
        &prepared_endpoint,
        &mut RuntimeRenderState::default(),
        Some(&EndpointModule::new().with_handler(Method::GET, |_event| {
            let mut response = ServerResponse::new(200);
            response.set_header("content-type", "application/json");
            response.body = Some("{\"ok\":true}".to_string());
            Ok(response)
        })),
        |_resolved, _behavior, _event| panic!("endpoint request should not execute data"),
        |_resolved, _behavior, _event, _state| panic!("endpoint request should not execute page"),
    )
    .expect("execute endpoint response");
    let RuntimeExecutionResult::Response(endpoint_response) = endpoint_response else {
        panic!("expected endpoint response");
    };
    assert_eq!(endpoint_response.header("vary"), Some("Accept"));
    assert_eq!(endpoint_response.body.as_deref(), Some("{\"ok\":true}"));

    let page_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/spa?draft=1").expect("valid page url"),
        headers: HeaderMap::new(),
    };
    let prepared_page = prepare_runtime_execution(
        &manifest,
        &page_request,
        &options,
        &RuntimeRenderState {
            prerendering: Some(Default::default()),
            ..Default::default()
        },
        0,
        false,
        |_, _| true,
    )
    .expect("prepare page execution");
    assert_eq!(prepared_page.dispatch, Some(RuntimeRouteDispatch::Page));
    assert_eq!(
        prepared_page
            .event
            .as_ref()
            .expect("prepared event")
            .url
            .query(),
        None
    );
    let page_response = execute_prepared_runtime_request(
        &prepared_page,
        &mut RuntimeRenderState::default(),
        None,
        |_resolved, _behavior, _event| panic!("page request should not execute data"),
        |resolved, _behavior, _event, state| {
            execute_page_request(
                &manifest,
                resolved.route,
                resolved.prepared.url.path(),
                false,
                state,
                200,
                |_node_index, _, _| panic!("shell page should not execute load callbacks"),
            )
        },
    )
    .expect("execute page response");
    assert_eq!(
        page_response,
        RuntimeExecutionResult::Page(PageRequestResult::Shell(ShellPageResponse {
            status: 200,
            ssr: false,
            csr: true,
            action: None,
            effects: Default::default(),
        }))
    );
}

#[test]
fn executes_runtime_page_requests_with_remote_actions() {
    let cwd = temp_dir("runtime-page-request-remote-action");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .expect("blog route");
    let event = build_runtime_event(
        &ServerRequest {
            method: Method::POST,
            url: Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22")
                .expect("valid remote page action request"),
            headers: HeaderMap::new(),
        },
        Arc::new(app_state()),
        Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22")
            .expect("valid remote page action url"),
        Some(route.id.clone()),
        Default::default(),
        false,
        false,
        0,
    );

    let result = execute_runtime_page_request(
        &manifest,
        route,
        &event,
        true,
        false,
        &mut RuntimeRenderState::default(),
        200,
        || panic!("remote page action should bypass local action"),
        |id| {
            assert_eq!(id, "hash/name/\"abc\"");
            Ok(svelte_kit::RemoteFormExecutionResult::Redirect {
                status: 303,
                location: "/next".to_string(),
            })
        },
        |_node_index, _, _| panic!("redirect action should bypass page load"),
    )
    .expect("runtime page request");

    let PageRequestResult::Redirect(redirect) = result else {
        panic!("expected redirect page result");
    };
    assert_eq!(redirect.status, 303);
}

#[test]
fn executes_runtime_page_action_json_branch_before_page_rendering() {
    let cwd = temp_dir("runtime-page-action-json");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .expect("blog route");
    let event = build_runtime_event(
        &ServerRequest {
            method: Method::POST,
            url: Url::parse("https://example.com/blog").expect("valid action json request"),
            headers: header_map([("accept".to_string(), "application/json".to_string())]),
        },
        Arc::new(app_state()),
        Url::parse("https://example.com/blog").expect("valid action json url"),
        Some(route.id.clone()),
        Default::default(),
        false,
        false,
        0,
    );

    let result = execute_runtime_page_stage(
        &manifest,
        route,
        &event,
        true,
        false,
        &mut RuntimeRenderState::default(),
        200,
        || ActionJsonResult::Failure {
            status: 422,
            data: json!({ "field": "missing" }),
        },
        || panic!("action-json request should bypass normal action handling"),
        |_id| panic!("action-json request should bypass remote actions"),
        |_node_index, _, _| panic!("action-json request should bypass page load"),
    )
    .expect("runtime page stage");

    let RuntimeExecutionResult::Response(response) = result else {
        panic!("expected action-json response");
    };
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_deref(),
        Some("{\"data\":{\"field\":\"missing\"},\"status\":422,\"type\":\"failure\"}")
    );
}

#[test]
fn stops_deep_runtime_page_cycles_before_action_or_load() {
    let cwd = temp_dir("runtime-page-depth-guard");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let route = manifest
        .manifest_routes
        .iter()
        .find(|route| route.id == "/blog")
        .expect("blog route");
    let event = build_runtime_event(
        &ServerRequest {
            method: Method::POST,
            url: Url::parse("https://example.com/blog").expect("valid deep page request"),
            headers: header_map([("accept".to_string(), "application/json".to_string())]),
        },
        Arc::new(app_state()),
        Url::parse("https://example.com/blog").expect("valid deep page url"),
        Some(route.id.clone()),
        Default::default(),
        false,
        false,
        11,
    );

    let mut state = RuntimeRenderState {
        depth: 11,
        ..Default::default()
    };

    let result = execute_runtime_page_stage(
        &manifest,
        route,
        &event,
        true,
        false,
        &mut state,
        200,
        || panic!("deep page request should not execute action-json"),
        || panic!("deep page request should not execute local action"),
        |_id| panic!("deep page request should not execute remote action"),
        |_node_index, _, _| panic!("deep page request should not execute page load"),
    )
    .expect("runtime page stage");

    let RuntimeExecutionResult::Response(response) = result else {
        panic!("expected depth-guard response");
    };
    assert_eq!(response.status, 404);
    assert_eq!(response.body.as_deref(), Some("Not found: /blog"));
}

#[test]
fn responds_with_page_stage_action_json_before_page_rendering() {
    let cwd = temp_dir("respond-runtime-request-page-stage");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid action json request"),
        headers: header_map([("accept".to_string(), "application/json".to_string())]),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        true,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("action-json request should not execute data"),
        |_resolved, _behavior, event, _state| {
            event
                .set_headers(
                    &header_map([("x-test".to_string(), "action-json".to_string())]),
                    None,
                )
                .expect("set action-json response header");
            event
                .cookies
                .set(
                    "session",
                    "abc",
                    CookieOptions {
                        path: Some("/".to_string()),
                        ..Default::default()
                    },
                )
                .expect("set action-json cookie");
            Ok(ActionJsonResult::Failure {
                status: 422,
                data: json!({ "field": "missing" }),
            })
        },
        |_resolved, _behavior, _event, _state| {
            panic!("action-json request should not execute normal action")
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("action-json request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("action-json request should not execute page load")
        },
        |_status, _error, _recursive| panic!("action-json request should not render error page"),
        |_remote_id, _event, _state| {
            panic!("action-json request should not execute remote request")
        },
        |_request| panic!("action-json request should not fetch"),
    )
    .expect("respond runtime request with page stage");

    let RuntimeRespondResult::Response(response) = result else {
        panic!("expected response result");
    };
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_deref(),
        Some("{\"data\":{\"field\":\"missing\"},\"status\":422,\"type\":\"failure\"}")
    );
    assert_eq!(response.header("x-test"), Some("action-json"));
    assert!(
        response
            .header_values("set-cookie")
            .expect("set-cookie header")
            .iter()
            .any(|header| header.contains("session=abc"))
    );
}

#[test]
fn responds_with_named_page_stage_action_json_before_page_rendering() {
    let cwd = temp_dir("respond-runtime-request-named-page-stage");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid named action request"),
        headers: header_map([
            ("accept".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("origin".to_string(), "https://example.com".to_string()),
        ]),
    };

    let result = respond_runtime_request_with_named_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        1,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("action-json request should not execute data"),
        |_resolved, _behavior, _event, _state, name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionJsonResult::Failure {
                status: 422,
                data: json!({ "field": "missing" }),
            }))
        },
        |_resolved, _behavior, _event, _state, _name| {
            panic!("action-json request should bypass normal action handling")
        },
        |_resolved, _behavior, _event, _state, _id| {
            panic!("action-json request should bypass remote actions")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("action-json request should bypass page load")
        },
        |_status, _error, _recursive| panic!("action-json request should not execute error page"),
        |_remote_id, _event, _state| panic!("action-json request should not execute remote"),
        |_request| panic!("action-json request should not fetch"),
    )
    .expect("respond with named page stage");

    let RuntimeRespondResult::Response(response) = result else {
        panic!("expected action-json response");
    };
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_deref(),
        Some("{\"data\":{\"field\":\"missing\"},\"status\":422,\"type\":\"failure\"}")
    );
}

#[test]
fn materializes_named_page_stage_rendered_pages() {
    let cwd = temp_dir("materialized-named-page-stage");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid named action request"),
        headers: header_map([
            ("accept".to_string(), "text/html".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("origin".to_string(), "https://example.com".to_string()),
        ]),
    };

    let response = respond_runtime_request_materialized_with_named_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        1,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("named page request should not execute data"),
        |_resolved, _behavior, _event, _state, _name| {
            panic!("html named page request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state, name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionRequestResult::Success {
                status: 200,
                data: Some(json!({ "saved": true })),
            }))
        },
        |_resolved, _behavior, _event, _state, _id| {
            panic!("named local page request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
        |_status, _error, _recursive| panic!("named page request should not execute error page"),
        |_remote_id, _event, _state| panic!("named page request should not execute remote"),
        |_request| panic!("named page request should not fetch"),
        |_shell| panic!("named page request should render a full page"),
        |rendered| {
            assert_eq!(
                rendered.action,
                Some(PageActionExecution {
                    headers: HeaderMap::new(),
                    result: ActionRequestResult::Success {
                        status: 200,
                        data: Some(json!({ "saved": true })),
                    },
                })
            );
            let mut response = ServerResponse::new(200);
            response.body = Some("rendered named page".to_string());
            Ok(response)
        },
        |_boundary| panic!("named page request should not render boundary"),
        |_error_page| panic!("named page request should not render error page"),
    )
    .expect("materialized named page stage response");

    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_deref(), Some("rendered named page"));
}

#[test]
fn materializes_named_page_stage_action_json_short_circuit() {
    let cwd = temp_dir("materialized-named-page-stage-action-json");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid named action request"),
        headers: header_map([
            ("accept".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("origin".to_string(), "https://example.com".to_string()),
        ]),
    };

    let response = respond_runtime_request_materialized_with_named_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        1,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("named action-json request should not execute data"),
        |_resolved, _behavior, _event, _state, name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionJsonResult::Success {
                status: 200,
                data: Some(json!({ "saved": true })),
            }))
        },
        |_resolved, _behavior, _event, _state, _name| {
            panic!("named action-json request should not execute normal actions")
        },
        |_resolved, _behavior, _event, _state, _id| {
            panic!("named action-json request should not execute remote actions")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("named action-json request should not execute page load")
        },
        |_status, _error, _recursive| {
            panic!("named action-json request should not execute error page")
        },
        |_remote_id, _event, _state| panic!("named action-json request should not execute remote"),
        |_request| panic!("named action-json request should not fetch"),
        |_shell| panic!("named action-json request should not render shell"),
        |_rendered| panic!("named action-json request should not render page"),
        |_boundary| panic!("named action-json request should not render boundary"),
        |_error_page| panic!("named action-json request should not render error page"),
    )
    .expect("materialized named action-json response");

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_deref(),
        Some("{\"data\":{\"saved\":true},\"status\":200,\"type\":\"success\"}")
    );
}

#[test]
fn materializes_named_page_stage_redirects() {
    let cwd = temp_dir("materialized-named-page-stage-redirect");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid named action request"),
        headers: header_map([
            ("accept".to_string(), "text/html".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("origin".to_string(), "https://example.com".to_string()),
        ]),
    };

    let response = respond_runtime_request_materialized_with_named_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        1,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("named redirect request should not execute data"),
        |_resolved, _behavior, _event, _state, _name| {
            panic!("named redirect request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state, name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionRequestResult::Redirect {
                status: 303,
                location: "/done".to_string(),
            }))
        },
        |_resolved, _behavior, _event, _state, _id| {
            panic!("named redirect request should not execute remote actions")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("named redirect request should not execute page load")
        },
        |_status, _error, _recursive| {
            panic!("named redirect request should not execute error page")
        },
        |_remote_id, _event, _state| panic!("named redirect request should not execute remote"),
        |_request| panic!("named redirect request should not fetch"),
        |_shell| panic!("named redirect request should not render shell"),
        |_rendered| panic!("named redirect request should not render page"),
        |_boundary| panic!("named redirect request should not render boundary"),
        |_error_page| panic!("named redirect request should not render error page"),
    )
    .expect("materialized named redirect response");

    assert_eq!(response.status, 303);
    assert_eq!(response.header("location"), Some("/done"));
}

#[test]
fn responds_to_page_options_requests_without_executing_loads() {
    let cwd = temp_dir("respond-runtime-request-page-options");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };

    let request = ServerRequest {
        method: Method::OPTIONS,
        url: Url::parse("https://example.com/blog").expect("valid options request"),
        headers: HeaderMap::new(),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("options request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("options request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("options request should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("options request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("options request should not execute page load")
        },
        |_status, _error, _recursive| panic!("options request should not render error page"),
        |_remote_id, _event, _state| panic!("options request should not execute remote"),
        |_request| panic!("options request should not fetch"),
    )
    .expect("respond to options request");

    let RuntimeRespondResult::Response(response) = result else {
        panic!("expected response result");
    };
    assert_eq!(response.status, 204);
    assert!(response.body.as_deref().unwrap_or("").is_empty());
    assert_eq!(response.header("allow"), Some("GET, HEAD, OPTIONS"));
}

#[test]
fn responds_to_page_options_requests_with_actions_allowing_post() {
    let cwd = temp_dir("respond-runtime-request-page-options-actions");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };

    let request = ServerRequest {
        method: Method::OPTIONS,
        url: Url::parse("https://example.com/blog").expect("valid options request"),
        headers: HeaderMap::new(),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        true,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("options request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("options request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("options request should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("options request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("options request should not execute page load")
        },
        |_status, _error, _recursive| panic!("options request should not render error page"),
        |_remote_id, _event, _state| panic!("options request should not execute remote"),
        |_request| panic!("options request should not fetch"),
    )
    .expect("respond to options request");

    let RuntimeRespondResult::Response(response) = result else {
        panic!("expected response result");
    };
    assert_eq!(response.status, 204);
    assert_eq!(response.header("allow"), Some("GET, HEAD, OPTIONS, POST"));
}

#[test]
fn responds_with_malformed_uri_before_route_execution() {
    let cwd = temp_dir("respond-runtime-request-malformed-uri");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/%FF").expect("valid request url"),
        headers: HeaderMap::new(),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("malformed uri should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("malformed uri should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("malformed uri should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("malformed uri should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("malformed uri should not execute page load")
        },
        |_status, _error, _recursive| panic!("malformed uri should not render error page"),
        |_remote_id, _event, _state| panic!("malformed uri should not execute remote request"),
        |_request| panic!("malformed uri should not fetch"),
    )
    .expect("respond malformed uri");

    let RuntimeRespondResult::Response(response) = result else {
        panic!("expected malformed uri response");
    };
    assert_eq!(response.status, 400);
    assert_eq!(response.body.as_deref(), Some("Malformed URI"));
}

#[test]
fn responds_with_plain_not_found_for_paths_outside_base() {
    let cwd = temp_dir("respond-runtime-request-outside-base");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "/base".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid request url"),
        headers: HeaderMap::new(),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("outside-base request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("outside-base request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("outside-base request should not execute action")
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("outside-base request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("outside-base request should not execute page load")
        },
        |_status, _error, _recursive| panic!("outside-base request should not render error page"),
        |_remote_id, _event, _state| {
            panic!("outside-base request should not execute remote request")
        },
        |_request| panic!("outside-base request should not fetch"),
    )
    .expect("respond outside base");

    let RuntimeRespondResult::Response(response) = result else {
        panic!("expected outside-base response");
    };
    assert_eq!(response.status, 404);
    assert_eq!(response.body.as_deref(), Some("Not found"));
}

#[test]
fn prerender_fallback_ignores_paths_outside_base() {
    let cwd = temp_dir("respond-runtime-request-outside-base-fallback");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "/base".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid request url"),
        headers: HeaderMap::new(),
    };
    let mut state = RuntimeRenderState {
        prerendering: Some(svelte_kit::PrerenderState {
            fallback: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("outside-base fallback should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("outside-base fallback should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("outside-base fallback should not execute action")
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("outside-base fallback should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("outside-base fallback should not execute page load")
        },
        |_status, _error, _recursive| panic!("outside-base fallback should not render error page"),
        |_remote_id, _event, _state| {
            panic!("outside-base fallback should not execute remote request")
        },
        |_request| panic!("outside-base fallback should not fetch"),
    )
    .expect("respond outside base fallback");

    assert_eq!(
        result,
        RuntimeRespondResult::Page(PageRequestResult::Shell(render_shell_page_response(
            200, true
        )))
    );
}

#[test]
fn responds_with_page_stage_data_redirects_as_json() {
    let cwd = temp_dir("respond-runtime-request-data-redirect");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog/__data.json").expect("valid data url"),
        headers: HeaderMap::new(),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| {
            let mut response = ServerResponse::new(307);
            response.set_header("location", "/next");
            Ok(Some(response))
        },
        |_resolved, _behavior, _event, _state| {
            panic!("data request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("data request should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("data request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("data request should not execute page load")
        },
        |_status, _error, _recursive| panic!("data request should not render error page"),
        |_remote_id, _event, _state| panic!("data request should not execute remote request"),
        |_request| panic!("data request should not fetch"),
    )
    .expect("respond runtime data redirect");

    let RuntimeRespondResult::Response(response) = result else {
        panic!("expected response result");
    };
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_deref(),
        Some("{\"location\":\"/next\",\"type\":\"redirect\"}")
    );
    assert_eq!(response.header("content-type"), Some("application/json"));
}

#[test]
fn responds_with_page_stage_remote_page_action_redirect() {
    let cwd = temp_dir("respond-runtime-request-page-stage-remote");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22")
            .expect("valid remote action request"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        true,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("remote page action should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("remote page action should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("remote page action should not execute local action")
        },
        |_resolved, _behavior, _event, _state, remote_id| {
            assert_eq!(remote_id, "hash/name/\"abc\"");
            Ok(svelte_kit::RemoteFormExecutionResult::Redirect {
                status: 303,
                location: "/next".to_string(),
            })
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("remote page action redirect should bypass page load")
        },
        |_status, _error, _recursive| panic!("remote page action should not render error page"),
        |_remote_id, _event, _state| panic!("remote page action should not execute remote request"),
        |_request| panic!("remote page action should not fetch"),
    )
    .expect("respond runtime request with page stage");

    let RuntimeRespondResult::Page(PageRequestResult::Redirect(redirect)) = result else {
        panic!("expected page redirect result");
    };
    assert_eq!(redirect.status, 303);
    assert_eq!(redirect.header("location"), Some("/next"));
}

#[test]
fn materialized_page_stage_remote_redirect_preserves_runtime_effects() {
    let cwd = temp_dir("respond-runtime-request-materialized-remote-redirect");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22")
            .expect("valid remote action request"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        true,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("remote page action should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("remote page action should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("remote page action should not execute local action")
        },
        |_resolved, _behavior, event, _state, remote_id| {
            assert_eq!(remote_id, "hash/name/\"abc\"");
            event
                .set_headers(
                    &header_map([("x-test".to_string(), "redirect".to_string())]),
                    None,
                )
                .expect("set redirect header");
            event
                .cookies
                .set(
                    "session",
                    "abc",
                    CookieOptions {
                        path: Some("/".to_string()),
                        ..Default::default()
                    },
                )
                .expect("set redirect cookie");
            Ok(svelte_kit::RemoteFormExecutionResult::Redirect {
                status: 303,
                location: "/next".to_string(),
            })
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("remote page action redirect should bypass page load")
        },
        |_status, _error, _recursive| panic!("remote page action should not render error page"),
        |_remote_id, _event, _state| panic!("remote page action should not execute remote request"),
        |_request| panic!("remote page action should not fetch"),
        |_shell| panic!("redirect should not use shell renderer"),
        |_rendered| panic!("redirect should not use rendered page"),
        |_boundary| panic!("redirect should not use boundary renderer"),
        |_error_page| panic!("redirect should not use error page renderer"),
    )
    .expect("materialize remote redirect response");

    assert_eq!(response.status, 303);
    assert_eq!(response.header("location"), Some("/next"));
    assert_eq!(response.header("x-test"), Some("redirect"));
    assert!(
        response
            .header_values("set-cookie")
            .expect("set-cookie header")
            .iter()
            .any(|header| header.contains("session=abc"))
    );
}

#[test]
fn responds_with_page_stage_local_action_failure_render() {
    let cwd = temp_dir("respond-runtime-request-page-stage-local");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid local action request"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };

    let result = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        true,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("local page action should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("local page action should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            Ok(ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "missing" }),
            })
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("local page action should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
        |_status, _error, _recursive| panic!("local page action should not render error page"),
        |_remote_id, _event, _state| panic!("local page action should not execute remote request"),
        |_request| panic!("local page action should not fetch"),
    )
    .expect("respond runtime request with page stage");

    let RuntimeRespondResult::Page(PageRequestResult::Rendered(rendered)) = result else {
        panic!("expected rendered page result");
    };
    assert_eq!(
        rendered.action,
        Some(svelte_kit::PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "missing" }),
            },
        })
    );
}

#[test]
fn materializes_page_stage_rendered_pages_into_final_responses() {
    let cwd = temp_dir("respond-runtime-request-materialized-page");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid local action request"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        true,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("local page action should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("local page action should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            Ok(ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "missing" }),
            })
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("local page action should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
        |_status, _error, _recursive| panic!("local page action should not render error page"),
        |_remote_id, _event, _state| panic!("local page action should not execute remote request"),
        |_request| panic!("local page action should not fetch"),
        |_shell| panic!("rendered page should not use shell renderer"),
        |rendered| {
            let mut response = ServerResponse::new(200);
            response.body = Some(format!(
                "rendered:{}:{}",
                rendered.plan.data_pathname,
                rendered
                    .action
                    .as_ref()
                    .map(|action| action.status().to_string())
                    .unwrap_or_else(|| "none".to_string())
            ));
            Ok(response)
        },
        |_boundary| panic!("rendered page should not use boundary renderer"),
        |_error_page| panic!("rendered page should not use error-page renderer"),
    )
    .expect("materialize runtime page response");

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body.as_deref(),
        Some("rendered:/blog/__data.json:422")
    );
}

#[test]
fn materialized_page_stage_adds_vary_accept_for_mixed_page_routes() {
    let cwd = temp_dir("respond-runtime-request-materialized-vary");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+server.js"),
        "export function GET() {}",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid page request"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("page request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("page request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("page request should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("page request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
        |_status, _error, _recursive| panic!("page request should not render error page"),
        |_remote_id, _event, _state| panic!("page request should not execute remote request"),
        |_request| panic!("page request should not fetch"),
        |_shell| panic!("page request should not use shell renderer"),
        |_rendered| Ok(ServerResponse::new(200)),
        |_boundary| panic!("page request should not use boundary renderer"),
        |_error_page| panic!("page request should not use error-page renderer"),
    )
    .expect("materialize mixed page response");

    assert_eq!(response.header("vary"), Some("Accept"));
}

#[test]
fn materialized_prerendered_page_sets_routeid_header() {
    let cwd = temp_dir("respond-runtime-request-materialized-routeid");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid page request"),
        headers: HeaderMap::new(),
    };
    let mut state = RuntimeRenderState {
        prerendering: Some(Default::default()),
        ..Default::default()
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("page request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("page request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("page request should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("page request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
        |_status, _error, _recursive| panic!("page request should not render error page"),
        |_remote_id, _event, _state| panic!("page request should not execute remote request"),
        |_request| panic!("page request should not fetch"),
        |_shell| panic!("page request should not use shell renderer"),
        |_rendered| Ok(ServerResponse::new(200)),
        |_boundary| panic!("page request should not use boundary renderer"),
        |_error_page| panic!("page request should not use error-page renderer"),
    )
    .expect("materialize prerender page response");

    assert_eq!(response.header("x-sveltekit-routeid"), Some("/blog"));
}

#[test]
fn materialized_page_stage_returns_not_modified_from_etag() {
    let cwd = temp_dir("respond-runtime-request-materialized-etag");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid page request"),
        headers: header_map([("if-none-match".to_string(), "\"abc\"".to_string())]),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("page request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("page request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("page request should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("page request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
        |_status, _error, _recursive| panic!("page request should not render error page"),
        |_remote_id, _event, _state| panic!("page request should not execute remote request"),
        |_request| panic!("page request should not fetch"),
        |_shell| panic!("page request should not use shell renderer"),
        |_rendered| {
            let mut response = ServerResponse::new(200);
            response.set_header("etag", "\"abc\"");
            response.set_header("cache-control", "private, max-age=60");
            response.body = Some("rendered".to_string());
            Ok(response)
        },
        |_boundary| panic!("page request should not use boundary renderer"),
        |_error_page| panic!("page request should not use error-page renderer"),
    )
    .expect("materialize not-modified page response");

    assert_eq!(response.status, 304);
    assert_eq!(response.body, None);
    assert_eq!(response.header("etag"), Some("\"abc\""));
    assert_eq!(
        response.header("cache-control"),
        Some("private, max-age=60")
    );
}

#[test]
fn materializes_page_stage_fatal_pages_through_error_page_renderer() {
    let cwd = temp_dir("respond-runtime-request-materialized-fatal");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("+error.svelte"), "<h1>error</h1>");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid page request"),
        headers: HeaderMap::new(),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("fatal page should not execute data"),
        |_resolved, _behavior, _event, _state| panic!("fatal page should not execute action-json"),
        |_resolved, _behavior, _event, _state| panic!("fatal page should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("fatal page should not execute remote action")
        },
        |_resolved, _behavior, event, _state, node_index, _server_parent, _parent| {
            if node_index == 0 {
                event
                    .set_headers(
                        &header_map([("x-test".to_string(), "fatal".to_string())]),
                        None,
                    )
                    .expect("set fatal page header");
                event
                    .cookies
                    .set(
                        "session",
                        "abc",
                        CookieOptions {
                            path: Some("/".to_string()),
                            ..Default::default()
                        },
                    )
                    .expect("set fatal page cookie");
                Ok(PageLoadResult::Error {
                    status: 500,
                    error: json!({ "message": "boom" }),
                })
            } else {
                Ok(PageLoadResult::Loaded {
                    server_data: None,
                    data: Some(json!({ "leaf": true })),
                })
            }
        },
        |_status, error, _recursive| {
            Ok(ErrorPageRequestResult::Rendered(ExecutedErrorPage {
                plan: ErrorPageRenderPlan {
                    status: 500,
                    error,
                    ssr: true,
                    csr: true,
                    branch_node_indexes: vec![0, 1],
                },
                branch: vec![
                    PageLoadedNode {
                        node_index: 0,
                        server_data: None,
                        data: Some(json!({ "layout": true })),
                    },
                    PageLoadedNode {
                        node_index: 1,
                        server_data: None,
                        data: None,
                    },
                ],
            }))
        },
        |_remote_id, _event, _state| panic!("fatal page should not execute remote request"),
        |_request| panic!("fatal page should not fetch"),
        |_shell| panic!("fatal page should not use shell renderer"),
        |_rendered| panic!("fatal page should not use page renderer"),
        |_boundary| panic!("fatal page should not use boundary renderer"),
        |error_page| {
            let mut response = ServerResponse::new(error_page.plan.status);
            response.body = Some(format!(
                "error-page:{}:{}",
                error_page.plan.status,
                error_page.branch.len()
            ));
            Ok(response)
        },
    )
    .expect("materialize fatal runtime page response");

    assert_eq!(response.status, 500);
    assert_eq!(response.body.as_deref(), Some("error-page:500:2"));
    assert_eq!(response.header("x-test"), Some("fatal"));
    assert!(
        response
            .header_values("set-cookie")
            .expect("set-cookie header")
            .iter()
            .any(|header| header.contains("session=abc"))
    );
}

#[test]
fn materializes_page_stage_shell_pages_into_final_responses() {
    let cwd = temp_dir("respond-runtime-request-materialized-shell");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+page.js"),
        "export const ssr = false;",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid page request"),
        headers: HeaderMap::new(),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("shell page should not execute data"),
        |_resolved, _behavior, _event, _state| panic!("shell page should not execute action-json"),
        |_resolved, _behavior, _event, _state| panic!("shell page should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("shell page should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("shell page should not execute page load")
        },
        |_status, _error, _recursive| panic!("shell page should not render error page"),
        |_remote_id, _event, _state| panic!("shell page should not execute remote request"),
        |_request| panic!("shell page should not fetch"),
        |shell| {
            let mut response = ServerResponse::new(shell.status);
            response.body = Some(format!("shell:{}:{}", shell.ssr, shell.csr));
            Ok(response)
        },
        |_rendered| panic!("shell page should not use page renderer"),
        |_boundary| panic!("shell page should not use boundary renderer"),
        |_error_page| panic!("shell page should not use error-page renderer"),
    )
    .expect("materialize shell runtime page response");

    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_deref(), Some("shell:false:true"));
}

#[test]
fn materializes_page_stage_error_boundaries_into_final_responses() {
    let cwd = temp_dir("respond-runtime-request-materialized-boundary");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("+error.svelte"), "<h1>error</h1>");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid page request"),
        headers: HeaderMap::new(),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("boundary page should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("boundary page should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| panic!("boundary page should not execute action"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("boundary page should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, node_index, _server_parent, _parent| {
            if node_index != 0 {
                Ok(PageLoadResult::Error {
                    status: 500,
                    error: json!({ "message": "leaf exploded" }),
                })
            } else {
                Ok(PageLoadResult::Loaded {
                    server_data: None,
                    data: Some(json!({ "layout": true })),
                })
            }
        },
        |_status, _error, _recursive| panic!("boundary page should not render error page"),
        |_remote_id, _event, _state| panic!("boundary page should not execute remote request"),
        |_request| panic!("boundary page should not fetch"),
        |_shell| panic!("boundary page should not use shell renderer"),
        |_rendered| panic!("boundary page should not use page renderer"),
        |boundary| {
            let mut response = ServerResponse::new(boundary.status);
            response.body = Some(format!(
                "boundary:{}:{}:{}",
                boundary.status,
                boundary.error_node_index,
                boundary.branch.len()
            ));
            Ok(response)
        },
        |_error_page| panic!("boundary page should not use error-page renderer"),
    )
    .expect("materialize boundary runtime page response");

    assert_eq!(response.status, 500);
    assert_eq!(response.body.as_deref(), Some("boundary:500:1:1"));
}

#[test]
fn materialize_page_request_result_merges_rendered_action_headers() {
    let response = materialize_page_request_result(
        PageRequestResult::Rendered(RenderedPage {
            plan: PageRenderPlan {
                ssr: true,
                csr: true,
                prerender: false,
                should_prerender_data: false,
                data_pathname: "/blog/__data.json".to_string(),
            },
            branch: vec![],
            action: Some(svelte_kit::PageActionExecution {
                headers: test_header_map("x-action", "failure"),
                result: ActionRequestResult::Failure {
                    status: 422,
                    data: json!({ "field": "missing" }),
                },
            }),
            effects: Default::default(),
        }),
        |_status, _error, _recursive| panic!("rendered page should not execute error page"),
        |_shell| panic!("rendered page should not use shell renderer"),
        |_rendered| {
            let mut response = ServerResponse::new(200);
            response.body = Some("rendered".to_string());
            Ok(response)
        },
        |_boundary| panic!("rendered page should not use boundary renderer"),
        |_error_page| panic!("rendered page should not use error page renderer"),
    )
    .expect("materialize rendered page");

    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_deref(), Some("rendered"));
    assert_eq!(response.header("x-action"), Some("failure"));
}

#[test]
fn materialize_page_request_result_merges_shell_action_headers() {
    let response = materialize_page_request_result(
        PageRequestResult::Shell(ShellPageResponse {
            status: 422,
            ssr: false,
            csr: true,
            action: Some(svelte_kit::PageActionExecution {
                headers: test_header_map("x-action", "failure"),
                result: ActionRequestResult::Failure {
                    status: 422,
                    data: json!({ "field": "missing" }),
                },
            }),
            effects: Default::default(),
        }),
        |_status, _error, _recursive| panic!("shell page should not execute error page"),
        |_shell| {
            let mut response = ServerResponse::new(422);
            response.body = Some("shell".to_string());
            Ok(response)
        },
        |_rendered| panic!("shell page should not use page renderer"),
        |_boundary| panic!("shell page should not use boundary renderer"),
        |_error_page| panic!("shell page should not use error page renderer"),
    )
    .expect("materialize shell page");

    assert_eq!(response.status, 422);
    assert_eq!(response.body.as_deref(), Some("shell"));
    assert_eq!(response.header("x-action"), Some("failure"));
}

#[test]
fn materialize_page_request_result_merges_boundary_action_headers() {
    let response = materialize_page_request_result(
        PageRequestResult::ErrorBoundary(PageErrorBoundary {
            status: 409,
            error: json!({ "message": "conflict" }),
            branch: vec![],
            error_node_index: 1,
            ssr: true,
            csr: true,
            action: Some(svelte_kit::PageActionExecution {
                headers: test_header_map("x-action", "error"),
                result: ActionRequestResult::Error {
                    error: json!({ "status": 409, "message": "conflict" }),
                },
            }),
            effects: Default::default(),
        }),
        |_status, _error, _recursive| panic!("boundary page should not execute error page"),
        |_shell| panic!("boundary page should not use shell renderer"),
        |_rendered| panic!("boundary page should not use page renderer"),
        |_boundary| {
            let mut response = ServerResponse::new(409);
            response.body = Some("boundary".to_string());
            Ok(response)
        },
        |_error_page| panic!("boundary page should not use error page renderer"),
    )
    .expect("materialize boundary page");

    assert_eq!(response.status, 409);
    assert_eq!(response.body.as_deref(), Some("boundary"));
    assert_eq!(response.header("x-action"), Some("error"));
}

#[test]
fn materialized_page_responses_include_runtime_event_effects() {
    let cwd = temp_dir("materialized-page-runtime-effects");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid page url"),
        headers: HeaderMap::new(),
    };

    let response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("page request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("page request should not execute action json")
        },
        |_resolved, _behavior, _event, _state| panic!("page request should not execute actions"),
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("page request should not execute remote actions")
        },
        |_resolved, _behavior, event, _state, node_index, _server_parent, _parent| {
            if event.response_headers.is_empty() {
                event
                    .set_headers(
                        &header_map([("x-test".to_string(), "page".to_string())]),
                        None,
                    )
                    .expect("set page response header");
                event
                    .cookies
                    .set(
                        "session",
                        "abc",
                        CookieOptions {
                            path: Some("/".to_string()),
                            ..Default::default()
                        },
                    )
                    .expect("set page cookie");
            }

            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "node": node_index })),
            })
        },
        |_status, _error, _recursive| panic!("page request should not execute error page"),
        |_remote_id, _event, _state| panic!("page request should not execute remote request"),
        |_request| panic!("page request should not fetch"),
        |_shell| panic!("page request should not use shell renderer"),
        |_rendered| {
            let mut response = ServerResponse::new(200);
            response.body = Some("rendered".to_string());
            Ok(response)
        },
        |_boundary| panic!("page request should not use boundary renderer"),
        |_error_page| panic!("page request should not use error-page renderer"),
    )
    .expect("materialize runtime page response");

    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_deref(), Some("rendered"));
    assert_eq!(response.header("x-test"), Some("page"));
    assert!(
        response
            .header_values("set-cookie")
            .expect("set-cookie header")
            .iter()
            .any(|header| header.contains("session=abc"))
    );
}

#[test]
fn top_level_head_requests_omit_response_bodies() {
    let cwd = temp_dir("respond-runtime-request-head-body");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("api").join("+server.js"),
        "export const GET = true;",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };

    let request = ServerRequest {
        method: Method::HEAD,
        url: Url::parse("https://example.com/api").expect("valid head url"),
        headers: HeaderMap::new(),
    };
    let mut state = RuntimeRenderState::default();

    let response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| {
            Some(EndpointModule::new().with_handler(Method::GET, |_| {
                let mut response = ServerResponse::new(200);
                response.set_header("x-head", "true");
                response.body = Some("{\"ok\":true}".to_string());
                Ok(response)
            }))
        },
        |_resolved, _behavior, _event| panic!("head request should not execute data"),
        |_resolved, _behavior, _event, _state| panic!("head request should not execute page"),
        |_status, _error, _recursive| panic!("head request should not render error page"),
        |_remote_id, _event, _state| panic!("head request should not execute remote"),
        |_request| panic!("head request should not fetch"),
    )
    .expect("respond to head request");

    let RuntimeRespondResult::Response(response) = response else {
        panic!("expected response result");
    };
    assert_eq!(response.status, 200);
    assert_eq!(response.body, None);
    assert_eq!(response.header("x-head"), Some("true"));
}

#[test]
fn materialized_head_requests_choose_page_or_endpoint_by_accept() {
    let cwd = temp_dir("respond-runtime-request-materialized-head-accept");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("head").join("+page.svelte"),
        "<h1>head</h1>",
    );
    write_file(
        &routes_dir.join("head").join("+server.js"),
        "export const GET = true;",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };

    let page_request = ServerRequest {
        method: Method::HEAD,
        url: Url::parse("https://example.com/head").expect("valid page head request"),
        headers: header_map([("accept".to_string(), "text/html".to_string())]),
    };
    let page_response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &page_request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| {
            Some(
                EndpointModule::new()
                    .with_handler(Method::GET, |_| {
                        let mut response = ServerResponse::new(200);
                        response.set_header("x-sveltekit-endpoint", "true");
                        response.body = Some("endpoint-get".to_string());
                        Ok(response)
                    })
                    .with_handler(Method::HEAD, |_| {
                        let mut response = ServerResponse::new(200);
                        response.set_header("x-sveltekit-head-endpoint", "true");
                        response.body = Some("endpoint-head".to_string());
                        Ok(response)
                    }),
            )
        },
        |_resolved, _behavior, _event| panic!("page head request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("page head request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("page head request should not execute action")
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("page head request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
        |_status, _error, _recursive| panic!("page head request should not render error page"),
        |_remote_id, _event, _state| panic!("page head request should not execute remote request"),
        |_request| panic!("page head request should not fetch"),
        |_shell| panic!("page head request should not use shell renderer"),
        |_rendered| {
            let mut response = ServerResponse::new(200);
            response.set_header("x-sveltekit-page", "true");
            response.body = Some("page".to_string());
            Ok(response)
        },
        |_boundary| panic!("page head request should not use boundary renderer"),
        |_error_page| panic!("page head request should not use error-page renderer"),
    )
    .expect("materialize page head request");

    assert_eq!(page_response.status, 200);
    assert_eq!(page_response.body, None);
    assert_eq!(page_response.header("x-sveltekit-page"), Some("true"));
    assert!(
        !page_response
            .headers
            .contains_key("x-sveltekit-head-endpoint")
    );

    let endpoint_request = ServerRequest {
        method: Method::HEAD,
        url: Url::parse("https://example.com/head").expect("valid endpoint head request"),
        headers: header_map([("accept".to_string(), "application/json".to_string())]),
    };
    let endpoint_response = respond_runtime_request_materialized_with_page_stage(
        &manifest,
        &endpoint_request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| {
            Some(
                EndpointModule::new()
                    .with_handler(Method::GET, |_| {
                        let mut response = ServerResponse::new(200);
                        response.set_header("x-sveltekit-endpoint", "true");
                        response.body = Some("endpoint-get".to_string());
                        Ok(response)
                    })
                    .with_handler(Method::HEAD, |_| {
                        let mut response = ServerResponse::new(200);
                        response.set_header("x-sveltekit-head-endpoint", "true");
                        response.body = Some("endpoint-head".to_string());
                        Ok(response)
                    }),
            )
        },
        |_resolved, _behavior, _event| panic!("endpoint head request should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("endpoint head request should not execute action-json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("endpoint head request should not execute action")
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("endpoint head request should not execute remote action")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("endpoint head request should not execute page load")
        },
        |_status, _error, _recursive| panic!("endpoint head request should not render error page"),
        |_remote_id, _event, _state| {
            panic!("endpoint head request should not execute remote request")
        },
        |_request| panic!("endpoint head request should not fetch"),
        |_shell| panic!("endpoint head request should not use shell renderer"),
        |_rendered| panic!("endpoint head request should not use page renderer"),
        |_boundary| panic!("endpoint head request should not use boundary renderer"),
        |_error_page| panic!("endpoint head request should not use error-page renderer"),
    )
    .expect("materialize endpoint head request");

    assert_eq!(endpoint_response.status, 200);
    assert_eq!(endpoint_response.body, None);
    assert_eq!(
        endpoint_response.header("x-sveltekit-head-endpoint"),
        Some("true")
    );
    assert!(!endpoint_response.has_header("x-sveltekit-page"));
}

#[test]
fn responds_to_missing_runtime_routes_without_faking_unimplemented_branches() {
    let cwd = temp_dir("respond-runtime-request-missing");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(&routes_dir.join("+error.svelte"), "<h1>error</h1>");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };

    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/missing").expect("valid missing url"),
        headers: HeaderMap::new(),
    };
    let mut state = RuntimeRenderState::default();
    let response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("missing route should not execute data"),
        |_resolved, _behavior, _event, _state| panic!("missing route should not execute page"),
        |status, error, recursive| {
            execute_error_page_request(
                &manifest,
                status,
                error,
                recursive,
                false,
                |code, message| format!("<h1>{code}:{message}</h1>"),
                |_node_index, _server_parent, _parent| {
                    Ok(PageLoadResult::Loaded {
                        server_data: None,
                        data: Some(json!({ "root": true })),
                    })
                },
            )
        },
        |_remote_id, _event, _state| panic!("missing route should not execute remote"),
        |_request| panic!("top-level missing route should not fetch"),
    )
    .expect("respond to missing route");
    assert_eq!(
        response,
        RuntimeRespondResult::ErrorPage(ErrorPageRequestResult::Rendered(ExecutedErrorPage {
            plan: ErrorPageRenderPlan {
                status: 404,
                error: json!({ "message": "Not found: /missing" }),
                ssr: true,
                csr: true,
                branch_node_indexes: vec![0, 1],
            },
            branch: vec![
                PageLoadedNode {
                    node_index: 0,
                    server_data: None,
                    data: Some(json!({ "root": true })),
                },
                PageLoadedNode {
                    node_index: 1,
                    server_data: None,
                    data: None,
                },
            ],
        }))
    );

    let mut prerender_state = RuntimeRenderState {
        prerendering: Some(Default::default()),
        ..Default::default()
    };
    let prerender_response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut prerender_state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("missing prerender route should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("missing prerender route should not execute page")
        },
        |_status, _error, _recursive| {
            panic!("prerender missing route should not render error page")
        },
        |_remote_id, _event, _state| panic!("missing prerender route should not execute remote"),
        |_request| panic!("missing prerender route should not fetch"),
    )
    .expect("respond to missing prerender route");
    assert_eq!(
        prerender_response,
        RuntimeRespondResult::Response({
            let mut response = ServerResponse::new(404);
            response.set_header("content-type", "text/plain; charset=utf-8");
            response.body = Some("not found".to_string());
            response
        })
    );

    let mut nested_error_state = RuntimeRenderState {
        depth: 1,
        ..Default::default()
    };
    nested_error_state.error = true;
    let nested_error_response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut nested_error_state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("nested error missing route should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("nested error missing route should not execute page")
        },
        |_status, _error, _recursive| panic!("nested error missing route should fetch"),
        |_remote_id, _event, _state| panic!("nested error missing route should not execute remote"),
        |delegated_request| {
            assert_eq!(
                delegated_request
                    .headers
                    .get("x-sveltekit-error")
                    .and_then(|value| value.to_str().ok()),
                Some("true")
            );
            let mut response = ServerResponse::new(502);
            response.set_header("content-type", "text/plain; charset=utf-8");
            response.body = Some("nested".to_string());
            Ok(response)
        },
    )
    .expect("respond to nested missing route");
    assert_eq!(
        nested_error_response,
        RuntimeRespondResult::Response({
            let mut response = ServerResponse::new(502);
            response.set_header("content-type", "text/plain; charset=utf-8");
            response.body = Some("nested".to_string());
            response
        })
    );

    let mut nested_state = RuntimeRenderState {
        depth: 1,
        ..Default::default()
    };
    let nested_response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut nested_state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("nested missing route should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("nested missing route should not execute page")
        },
        |_status, _error, _recursive| panic!("nested missing route should fetch"),
        |_remote_id, _event, _state| panic!("nested missing route should not execute remote"),
        |nested_request| {
            assert!(nested_request.header("x-sveltekit-error").is_none());
            let mut response = ServerResponse::new(404);
            response.set_header("content-type", "text/plain; charset=utf-8");
            response.body = Some("fetched".to_string());
            Ok(response)
        },
    )
    .expect("respond to nested missing route");
    assert_eq!(
        nested_response,
        RuntimeRespondResult::Response({
            let mut response = ServerResponse::new(404);
            response.set_header("content-type", "text/plain; charset=utf-8");
            response.body = Some("fetched".to_string());
            response
        })
    );
}

#[test]
fn responds_with_shell_page_for_hash_routing_root_requests() {
    let cwd = temp_dir("respond-runtime-request-hash-shell");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: true,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/").expect("valid root url"),
        headers: HeaderMap::new(),
    };

    let response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("hash root should not execute data"),
        |_resolved, _behavior, _event, _state| panic!("hash root should not execute page"),
        |_status, _error, _recursive| panic!("hash root should not execute error page"),
        |_remote_id, _event, _state| panic!("hash root should not execute remote"),
        |_request| panic!("hash root should not fetch"),
    )
    .expect("respond to hash root request");

    assert_eq!(
        response,
        RuntimeRespondResult::Page(PageRequestResult::Shell(render_shell_page_response(
            200, true
        )))
    );
}

#[test]
fn responds_with_shell_page_for_prerender_fallback_requests() {
    let cwd = temp_dir("respond-runtime-request-prerender-fallback");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/missing").expect("valid missing url"),
        headers: HeaderMap::new(),
    };
    let mut state = RuntimeRenderState {
        prerendering: Some(svelte_kit::PrerenderState {
            fallback: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let response = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("prerender fallback should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("prerender fallback should not execute action json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("prerender fallback should not execute actions")
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("prerender fallback should not execute remote actions")
        },
        |_resolved, _behavior, _event, _state, _leaf, _server_parent, _parent| {
            panic!("prerender fallback should not load page")
        },
        |_status, _error, _recursive| panic!("prerender fallback should not execute error page"),
        |_remote_id, _event, _state| panic!("prerender fallback should not execute remote"),
        |_request| panic!("prerender fallback should not fetch"),
    )
    .expect("respond to prerender fallback request");

    assert_eq!(
        response,
        RuntimeRespondResult::Page(PageRequestResult::Shell(render_shell_page_response(
            200, true
        )))
    );
}

#[test]
fn prerender_fallback_ignores_normalization_redirects() {
    let cwd = temp_dir("respond-runtime-request-prerender-fallback-normalize");
    let routes_dir = cwd.join("src").join("routes");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );
    write_file(
        &routes_dir.join("blog").join("+page.js"),
        "export const trailingSlash = 'always';",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid blog url"),
        headers: HeaderMap::new(),
    };
    let mut state = RuntimeRenderState {
        prerendering: Some(svelte_kit::PrerenderState {
            fallback: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let response = respond_runtime_request_with_page_stage(
        &manifest,
        &request,
        &options,
        &mut state,
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("fallback normalize should not execute data"),
        |_resolved, _behavior, _event, _state| {
            panic!("fallback normalize should not execute action json")
        },
        |_resolved, _behavior, _event, _state| {
            panic!("fallback normalize should not execute actions")
        },
        |_resolved, _behavior, _event, _state, _remote_id| {
            panic!("fallback normalize should not execute remote actions")
        },
        |_resolved, _behavior, _event, _state, _node_index, _server_parent, _parent| {
            panic!("fallback normalize should not load page")
        },
        |_status, _error, _recursive| panic!("fallback normalize should not execute error page"),
        |_remote_id, _event, _state| panic!("fallback normalize should not execute remote"),
        |_request| panic!("fallback normalize should not fetch"),
    )
    .expect("respond to prerender fallback normalize request");

    assert_eq!(
        response,
        RuntimeRespondResult::Page(PageRequestResult::Shell(render_shell_page_response(
            200, true
        )))
    );
}

#[test]
fn applies_runtime_event_response_effects_to_data_responses() {
    let cwd = temp_dir("respond-runtime-request-data-effects");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog/__data.json?x-sveltekit-invalidated=1")
            .expect("valid data url"),
        headers: HeaderMap::new(),
    };

    let response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, event| {
            event
                .set_headers(
                    &header_map([("x-test".to_string(), "runtime".to_string())]),
                    None,
                )
                .expect("set runtime response header");
            event
                .cookies
                .set(
                    "session",
                    "abc",
                    CookieOptions {
                        path: Some("/".to_string()),
                        ..Default::default()
                    },
                )
                .expect("set runtime cookie");

            let mut response = ServerResponse::new(200);
            response.set_header("content-type", "application/json; charset=utf-8");
            response.body = Some("{\"type\":\"data\"}".to_string());
            Ok(Some(response))
        },
        |_resolved, _behavior, _event, _state| panic!("data response should not execute page"),
        |_status, _error, _recursive| panic!("data response should not execute error page"),
        |_remote_id, _event, _state| panic!("data response should not execute remote"),
        |_request| panic!("data response should not fetch"),
    )
    .expect("respond to data request");

    let RuntimeRespondResult::Response(response) = response else {
        panic!("expected direct data response");
    };

    assert_eq!(response.header("x-test"), Some("runtime"));
    assert!(
        response
            .header_values("set-cookie")
            .expect("set-cookie header")
            .iter()
            .any(|header| header.contains("session=abc"))
    );
}

#[test]
fn executes_remote_runtime_requests_before_route_dispatch() {
    let cwd = temp_dir("respond-runtime-request-remote");
    let routes_dir = cwd.join("src").join("routes");
    write_file(&routes_dir.join("+layout.svelte"), "<slot />");
    write_file(
        &routes_dir.join("blog").join("+page.svelte"),
        "<h1>blog</h1>",
    );

    let manifest =
        KitManifest::discover(&ManifestConfig::new(routes_dir, cwd)).expect("discover manifest");
    let options = RuntimeRequestOptions {
        base: "".to_string(),
        app_dir: "_app".to_string(),
        hash_routing: false,
        csrf_check_origin: true,
        csrf_trusted_origins: vec![],
        public_env: Map::new(),
        route_assets: RouteResolutionAssets::default(),
    };

    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/_app/remote/hash/name/arg").expect("valid remote url"),
        headers: header_map([
            ("origin".to_string(), "https://example.com".to_string()),
            ("x-sveltekit-pathname".to_string(), "/blog".to_string()),
        ]),
    };

    let response = respond_runtime_request(
        &manifest,
        &request,
        &options,
        &mut RuntimeRenderState::default(),
        false,
        false,
        |status, message| format!("<h1>{status}:{message}</h1>"),
        |_, _| true,
        |_resolved| None,
        |_resolved, _behavior, _event| panic!("remote request should not execute data"),
        |_resolved, _behavior, _event, _state| panic!("remote request should not execute page"),
        |_status, _error, _recursive| panic!("remote request should not execute error page"),
        |remote_id, event, _state| {
            assert_eq!(remote_id, "hash/name/arg");
            assert!(event.is_remote_request);
            assert_eq!(event.url.path(), "/blog");
            event
                .set_headers(
                    &header_map([("x-test".to_string(), "remote".to_string())]),
                    None,
                )
                .expect("set remote response header");
            event
                .cookies
                .set(
                    "session",
                    "abc",
                    CookieOptions {
                        path: Some("/".to_string()),
                        ..Default::default()
                    },
                )
                .expect("set remote cookie");
            let mut response = ServerResponse::new(200);
            response.set_header("content-type", "application/json; charset=utf-8");
            response.body = Some("{\"type\":\"remote\"}".to_string());
            Ok(response)
        },
        |_request| panic!("remote request should not fall back to fetch"),
    )
    .expect("respond to remote request");

    assert_eq!(
        response,
        RuntimeRespondResult::Response({
            let mut response = ServerResponse::new(200);
            response.set_header("content-type", "application/json; charset=utf-8");
            response.set_header("x-test", "remote");
            response.append_header(
                "set-cookie",
                "session=abc; Path=/; HttpOnly; Secure; SameSite=Lax",
            );
            response.body = Some("{\"type\":\"remote\"}".to_string());
            response
        })
    );
}

#[test]
fn builds_runtime_event_scaffold_like_upstream() {
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/base/blog/hello?x=1").expect("valid request"),
        headers: header_map([("cookie".to_string(), "session=abc".to_string())]),
    };

    let event: RuntimeEvent = build_runtime_event(
        &request,
        Arc::new(app_state()),
        Url::parse("https://example.com/base/blog/hello?x=1").expect("valid rewritten url"),
        Some("/blog/[slug]".to_string()),
        std::collections::BTreeMap::from([("slug".to_string(), "hello".to_string())]),
        true,
        false,
        1,
    );

    assert_eq!(event.route_id.as_deref(), Some("/blog/[slug]"));
    assert_eq!(event.params.get("slug").map(String::as_str), Some("hello"));
    assert_eq!(event.url.path(), "/base/blog/hello");
    assert!(event.is_data_request);
    assert!(event.is_sub_request);
    assert!(!event.is_remote_request);
    assert_eq!(event.cookies.get("session").as_deref(), Some("abc"));
    assert!(event.response_headers.is_empty());
}
