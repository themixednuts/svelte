use camino::Utf8PathBuf;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use serde_json::json;
use std::{
    fs,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use svelte_kit::{
    ActionJsonResult, ActionRequestResult, AppState, KitManifest, ManifestConfig,
    PageActionExecution, PageLoadResult, PageRequestResult, RuntimeExecutionResult,
    RuntimeRenderState, ServerRequest, build_runtime_event, execute_named_page_action_json_request,
    execute_named_page_request_from_request, execute_named_runtime_page_stage,
    execute_page_action_json_request, execute_page_request_from_request,
    execute_page_request_with_action, resolve_named_page_action_request,
    resolve_page_action_request,
};
use url::Url;

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

fn app_state() -> AppState {
    AppState::default()
}

#[test]
fn ignores_non_action_requests() {
    let request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: HeaderMap::new(),
    };

    assert!(
        resolve_page_action_request(
            &request,
            Some("/blog"),
            false,
            true,
            || panic!("GET should not execute local action"),
            |_id| panic!("GET should not execute remote action"),
        )
        .expect("action resolution succeeds")
        .is_none()
    );
}

#[test]
fn prefers_remote_page_actions_before_local_actions() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22").expect("valid url"),
        headers: HeaderMap::new(),
    };

    let result = resolve_page_action_request(
        &request,
        Some("/blog"),
        false,
        true,
        || panic!("remote page action should bypass local action"),
        |id| {
            assert_eq!(id, "hash/name/\"abc\"");
            Ok(svelte_kit::RemoteFormExecutionResult::Success)
        },
    )
    .expect("remote action resolution succeeds")
    .expect("remote action result");

    assert_eq!(
        result,
        PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Success {
                status: 200,
                data: None,
            },
        }
    );
    assert_eq!(result.status(), 200);
}

#[test]
fn returns_no_actions_page_result_with_allow_header() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: HeaderMap::new(),
    };

    let result = resolve_page_action_request(
        &request,
        Some("/blog"),
        true,
        false,
        || panic!("missing page actions should not execute local action"),
        |_id| panic!("non-remote page action should not execute remote action"),
    )
    .expect("page action resolution succeeds")
    .expect("page action result");

    assert_eq!(
        result
            .headers
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("GET")
    );
    assert_eq!(
        result.result,
        ActionRequestResult::Error {
            error: serde_json::json!({
                "status": 405,
                "message": "POST method not allowed. No form actions exist for the page at /blog",
                "allow": "GET",
            }),
        }
    );
    assert_eq!(result.status(), 405);
}

#[test]
fn executes_local_page_actions_when_not_remote() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: HeaderMap::new(),
    };

    let result = resolve_page_action_request(
        &request,
        Some("/blog"),
        false,
        true,
        || {
            Ok(ActionRequestResult::Redirect {
                status: 303,
                location: "/next".to_string(),
            })
        },
        |_id| panic!("local page action should not execute remote action"),
    )
    .expect("page action resolution succeeds")
    .expect("page action result");

    assert_eq!(
        result.result,
        ActionRequestResult::Redirect {
            status: 303,
            location: "/next".to_string(),
        }
    );
    let redirect = result.redirect_response().expect("redirect response");
    assert_eq!(redirect.status, 303);
    assert_eq!(redirect.header("location"), Some("/next"));
}

#[test]
fn resolves_named_local_page_actions_from_query_params() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid url"),
        headers: header_map([(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )]),
    };

    let result = resolve_named_page_action_request(&request, false, 1, |name| {
        assert_eq!(name, "publish");
        Ok(Some(ActionRequestResult::Success {
            status: 200,
            data: Some(json!({ "ok": true })),
        }))
    })
    .expect("named action resolution succeeds")
    .expect("named action result");

    assert_eq!(
        result.result,
        ActionRequestResult::Success {
            status: 200,
            data: Some(json!({ "ok": true })),
        }
    );
}

#[test]
fn rejects_reserved_default_named_action_and_missing_named_actions() {
    let reserved = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/default").expect("valid url"),
        headers: header_map([(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )]),
    };

    let reserved = resolve_named_page_action_request(&reserved, false, 1, |_| {
        panic!("reserved action name should not dispatch")
    })
    .expect("reserved action resolution succeeds")
    .expect("reserved action result");
    assert_eq!(
        reserved.result,
        ActionRequestResult::Error {
            error: json!({
                "message": "Cannot use reserved action name \"default\""
            }),
        }
    );

    let missing = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid url"),
        headers: header_map([(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )]),
    };

    let missing = resolve_named_page_action_request(&missing, false, 1, |_| Ok(None))
        .expect("missing named action resolution succeeds")
        .expect("missing named action result");
    assert_eq!(
        missing.result,
        ActionRequestResult::Error {
            error: json!({
                "status": 404,
                "message": "No action with name 'publish' found"
            }),
        }
    );
}

#[test]
fn validates_named_local_action_config_and_form_content_type() {
    let mixed = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: header_map([(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )]),
    };

    let mixed = resolve_named_page_action_request(&mixed, true, 1, |_| {
        panic!("mixed default/named config should not dispatch")
    })
    .expect("mixed action resolution succeeds")
    .expect("mixed action result");
    assert_eq!(
        mixed.result,
        ActionRequestResult::Error {
            error: json!({
                "message": "When using named actions, the default action cannot be used. See the docs for more info: https://svelte.dev/docs/kit/form-actions#named-actions"
            }),
        }
    );

    let invalid_content_type = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid url"),
        headers: header_map([("content-type".to_string(), "application/json".to_string())]),
    };

    let invalid_content_type =
        resolve_named_page_action_request(&invalid_content_type, false, 1, |_| {
            panic!("invalid content-type should not dispatch")
        })
        .expect("content-type validation succeeds")
        .expect("content-type validation result");
    assert_eq!(
        invalid_content_type.result,
        ActionRequestResult::Error {
            error: json!({
                "status": 415,
                "message": "Form actions expect form-encoded data — received application/json"
            }),
        }
    );
}

#[test]
fn rejects_non_object_action_json_payloads() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid action json url"),
        headers: header_map([
            ("accept".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ]),
    };

    let response = execute_page_action_json_request(
        &request,
        &app_state(),
        Some("/blog"),
        false,
        true,
        || ActionJsonResult::Success {
            status: 200,
            data: Some(json!(["bad"])),
        },
    )
    .expect("action json response");

    assert_eq!(response.status, 500);
    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"error\":{\"message\":\"Data returned from action inside /blog is not serializable. Form actions need to return plain objects or fail(). E.g. return { success: true } or return fail(400, { message: \\\"invalid\\\" });\"},\"type\":\"error\"}"
        )
    );
}

#[test]
fn rejects_non_object_page_action_payloads() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid page action url"),
        headers: HeaderMap::new(),
    };

    let result = resolve_page_action_request(
        &request,
        Some("/blog"),
        false,
        true,
        || {
            Ok(ActionRequestResult::Success {
                status: 200,
                data: Some(json!(["bad"])),
            })
        },
        |_id| panic!("local page action should not execute remote action"),
    )
    .expect("page action resolution succeeds")
    .expect("page action result");

    assert_eq!(
        result.result,
        ActionRequestResult::Error {
            error: json!({
                "message": "Data returned from action inside /blog is not serializable. Form actions need to return plain objects or fail(). E.g. return { success: true } or return fail(400, { message: \"invalid\" });"
            }),
        }
    );
    assert_eq!(result.status(), 500);
}

#[test]
fn handles_named_action_json_requests() {
    let request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid url"),
        headers: header_map([
            ("accept".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ]),
    };

    let success = execute_named_page_action_json_request(
        &request,
        &app_state(),
        Some("/blog"),
        false,
        false,
        1,
        |name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionJsonResult::Success {
                status: 200,
                data: Some(json!({ "ok": true })),
            }))
        },
    )
    .expect("named action json execution succeeds")
    .expect("named action json response");
    assert_eq!(success.status, 200);
    assert_eq!(
        success.body.as_deref(),
        Some("{\"data\":{\"ok\":true},\"status\":200,\"type\":\"success\"}")
    );

    let missing = execute_named_page_action_json_request(
        &request,
        &app_state(),
        Some("/blog"),
        false,
        false,
        1,
        |_| Ok(None),
    )
    .expect("named action json execution succeeds")
    .expect("missing named action json response");
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.body.as_deref(),
        Some(
            "{\"error\":{\"message\":\"No action with name 'publish' found\",\"status\":404},\"type\":\"error\"}"
        )
    );
}

#[test]
fn validates_named_action_json_configuration_and_content_type() {
    let mixed = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid url"),
        headers: header_map([
            ("accept".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ]),
    };

    let mixed = execute_named_page_action_json_request(
        &mixed,
        &app_state(),
        Some("/blog"),
        false,
        true,
        1,
        |_| panic!("mixed default/named config should not dispatch"),
    )
    .expect("mixed named action json execution succeeds")
    .expect("mixed named action json response");
    assert_eq!(mixed.status, 500);
    assert_eq!(
        mixed.body.as_deref(),
        Some(
            "{\"error\":{\"message\":\"When using named actions, the default action cannot be used. See the docs for more info: https://svelte.dev/docs/kit/form-actions#named-actions\"},\"type\":\"error\"}"
        )
    );

    let invalid_content_type = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid url"),
        headers: header_map([
            ("accept".to_string(), "application/json".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]),
    };

    let invalid_content_type = execute_named_page_action_json_request(
        &invalid_content_type,
        &app_state(),
        Some("/blog"),
        false,
        false,
        1,
        |_| panic!("invalid content-type should not dispatch"),
    )
    .expect("invalid content-type action json execution succeeds")
    .expect("invalid content-type action json response");
    assert_eq!(invalid_content_type.status, 415);
    assert_eq!(
        invalid_content_type.body.as_deref(),
        Some(
            "{\"error\":{\"message\":\"Form actions expect form-encoded data — received application/json\",\"status\":415},\"type\":\"error\"}"
        )
    );
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
        .join(format!("svelte-kit-page-actions-{label}-{unique}"));
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
fn applies_action_results_to_page_execution() {
    let cwd = temp_dir("page-execution");
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
    let redirect = execute_page_request_with_action(
        &manifest,
        route,
        "/blog",
        true,
        &mut RuntimeRenderState::default(),
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Redirect {
                status: 303,
                location: "/next".to_string(),
            },
        }),
        200,
        |_node_index, _, _| panic!("redirect action should bypass page load"),
    )
    .expect("page redirect");
    let PageRequestResult::Redirect(redirect) = redirect else {
        panic!("expected page redirect");
    };
    assert_eq!(redirect.status, 303);

    let failure = execute_page_request_with_action(
        &manifest,
        route,
        "/blog",
        true,
        &mut RuntimeRenderState::default(),
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "bad" }),
            },
        }),
        200,
        |_node_index, _, _| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
    )
    .expect("page failure render");
    let PageRequestResult::Rendered(rendered) = failure else {
        panic!("expected rendered page");
    };
    assert_eq!(rendered.plan.data_pathname, "/blog/__data.json");
    assert_eq!(
        rendered.action,
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "bad" }),
            },
        })
    );

    let error = execute_page_request_with_action(
        &manifest,
        route,
        "/blog",
        true,
        &mut RuntimeRenderState::default(),
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Error {
                error: json!({ "status": 409, "message": "conflict" }),
            },
        }),
        200,
        |_node_index, _, _| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "layout": true })),
            })
        },
    )
    .expect("page error render");
    let PageRequestResult::ErrorBoundary(boundary) = error else {
        panic!("expected error boundary");
    };
    assert_eq!(boundary.status, 409);
    assert_eq!(
        boundary.error,
        json!({ "status": 409, "message": "conflict" })
    );
    assert_eq!(
        boundary.action,
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Error {
                error: json!({ "status": 409, "message": "conflict" }),
            },
        })
    );
}

#[test]
fn composes_page_action_resolution_with_page_execution() {
    let cwd = temp_dir("page-request-from-request");
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

    let remote_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22")
            .expect("valid remote page action url"),
        headers: HeaderMap::new(),
    };
    let remote = execute_page_request_from_request(
        &manifest,
        route,
        &remote_request,
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
    .expect("remote page request");
    let PageRequestResult::Redirect(redirect) = remote else {
        panic!("expected redirect page result");
    };
    assert_eq!(redirect.status, 303);

    let local_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid local page action url"),
        headers: HeaderMap::new(),
    };
    let local = execute_page_request_from_request(
        &manifest,
        route,
        &local_request,
        true,
        false,
        &mut RuntimeRenderState::default(),
        200,
        || {
            Ok(ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "bad" }),
            })
        },
        |_id| panic!("local page action should not execute remote handler"),
        |_node_index, _, _| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
    )
    .expect("local page request");
    let PageRequestResult::Rendered(rendered) = local else {
        panic!("expected rendered page result");
    };
    assert_eq!(
        rendered.action,
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "bad" }),
            },
        })
    );
}

#[test]
fn composes_named_page_action_resolution_with_page_execution() {
    let cwd = temp_dir("named-page-request-from-request");
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

    let named_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid named page action url"),
        headers: header_map([(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )]),
    };
    let named = execute_named_page_request_from_request(
        &manifest,
        route,
        &named_request,
        false,
        1,
        &mut RuntimeRenderState::default(),
        200,
        |name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "bad" }),
            }))
        },
        |_id| panic!("named local page action should not execute remote handler"),
        |_node_index, _, _| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
    )
    .expect("named page request");
    let PageRequestResult::Rendered(rendered) = named else {
        panic!("expected rendered page result");
    };
    assert_eq!(
        rendered.action,
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Failure {
                status: 422,
                data: json!({ "field": "bad" }),
            },
        })
    );
}

#[test]
fn handles_page_action_json_requests_before_page_rendering() {
    let get_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog").expect("valid get url"),
        headers: HeaderMap::new(),
    };
    assert!(
        execute_page_action_json_request(
            &get_request,
            &app_state(),
            Some("/blog"),
            false,
            true,
            || panic!("GET should not execute action-json handler"),
        )
        .is_none()
    );

    let post_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog").expect("valid action json url"),
        headers: header_map([("accept".to_string(), "application/json".to_string())]),
    };

    let success = execute_page_action_json_request(
        &post_request,
        &app_state(),
        Some("/blog"),
        false,
        true,
        || ActionJsonResult::Success {
            status: 200,
            data: Some(json!({ "ok": true })),
        },
    )
    .expect("action json response");
    assert_eq!(success.status, 200);
    assert_eq!(
        success.body.as_deref(),
        Some("{\"data\":{\"ok\":true},\"status\":200,\"type\":\"success\"}")
    );

    let missing = execute_page_action_json_request(
        &post_request,
        &app_state(),
        Some("/blog"),
        true,
        false,
        || panic!("missing actions should not execute action-json handler"),
    )
    .expect("missing action json response");
    assert_eq!(missing.status, 405);
    assert_eq!(missing.header("allow"), Some("GET"));
}

#[test]
fn handles_named_runtime_page_stage_requests() {
    let cwd = temp_dir("named-runtime-page-stage");
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

    let json_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid named action json url"),
        headers: header_map([
            ("accept".to_string(), "application/json".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ]),
    };
    let json_event = build_runtime_event(
        &json_request,
        Arc::new(app_state()),
        json_request.url.clone(),
        Some(route.id.clone()),
        Default::default(),
        false,
        false,
        0,
    );

    let json_result = execute_named_runtime_page_stage(
        &manifest,
        route,
        &json_event,
        false,
        1,
        &mut RuntimeRenderState::default(),
        200,
        |name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionJsonResult::Success {
                status: 200,
                data: Some(json!({ "ok": true })),
            }))
        },
        |_name| panic!("action-json branch should not execute local page action"),
        |_id| panic!("action-json branch should not execute remote action"),
        |_node_index, _, _| panic!("action-json branch should not execute page load"),
    )
    .expect("named runtime page stage");
    let RuntimeExecutionResult::Response(response) = json_result else {
        panic!("expected action-json response");
    };
    assert_eq!(
        response.body.as_deref(),
        Some("{\"data\":{\"ok\":true},\"status\":200,\"type\":\"success\"}")
    );

    let page_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/publish").expect("valid named page action url"),
        headers: header_map([
            ("accept".to_string(), "text/html".to_string()),
            (
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
        ]),
    };
    let page_event = build_runtime_event(
        &page_request,
        Arc::new(app_state()),
        page_request.url.clone(),
        Some(route.id.clone()),
        Default::default(),
        false,
        false,
        0,
    );

    let page_result = execute_named_runtime_page_stage(
        &manifest,
        route,
        &page_event,
        false,
        1,
        &mut RuntimeRenderState::default(),
        200,
        |_name| panic!("page branch should not execute action-json closure"),
        |name| {
            assert_eq!(name, "publish");
            Ok(Some(ActionRequestResult::Success {
                status: 200,
                data: Some(json!({ "ok": true })),
            }))
        },
        |_id| panic!("named local page action should not execute remote action"),
        |_node_index, _, _| {
            Ok(PageLoadResult::Loaded {
                server_data: None,
                data: Some(json!({ "page": true })),
            })
        },
    )
    .expect("named runtime page stage page result");
    let RuntimeExecutionResult::Page(PageRequestResult::Rendered(rendered)) = page_result else {
        panic!("expected rendered page result");
    };
    assert_eq!(
        rendered.action,
        Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Success {
                status: 200,
                data: Some(json!({ "ok": true })),
            },
        })
    );
}
