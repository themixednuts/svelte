use http::{HeaderMap, Method};
use serde_json::{Map, json};
use std::{collections::BTreeMap, sync::Arc};
use svelte_kit::{
    ActionRequestResult, AppState, PreparedRemoteInvocation, RemoteCallExecution, RemoteCallKind,
    RemoteCallRequest, RemoteFormExecutionResult, RemoteFunctionResponse, ServerRequest,
    ServerTransportDecoder, ServerTransportEncoder, execute_remote_call,
    handle_remote_form_action_request, handle_remote_form_post_result, parse_remote_arg,
    parse_remote_id, remote_json_response, stringify_remote_arg,
};
use url::Url;

fn decode_date(value: &serde_json::Value) -> svelte_kit::Result<serde_json::Value> {
    Ok(json!({ "$type": "date", "value": value }))
}

fn default_app_state() -> AppState {
    AppState::default()
}

fn date_roundtrip_app_state() -> AppState {
    AppState {
        decoders: BTreeMap::from([(
            "date".to_string(),
            Arc::new(decode_date) as ServerTransportDecoder,
        )]),
        encoders: BTreeMap::from([(
            "date".to_string(),
            Arc::new(|value: &serde_json::Value| {
                if value.get("$type") == Some(&json!("date")) {
                    Ok(Some(
                        value
                            .get("value")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    ))
                } else {
                    Ok(None)
                }
            }) as ServerTransportEncoder,
        )]),
    }
}

fn date_encode_app_state() -> AppState {
    AppState {
        decoders: BTreeMap::new(),
        encoders: BTreeMap::from([(
            "date".to_string(),
            Arc::new(|value: &serde_json::Value| {
                if value.get("$type") == Some(&json!("date")) {
                    Ok(Some(
                        value
                            .get("value")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    ))
                } else {
                    Ok(None)
                }
            }) as ServerTransportEncoder,
        )]),
    }
}

#[test]
fn parses_remote_ids_like_upstream() {
    let parsed = parse_remote_id("hash/name/arg");
    assert_eq!(parsed.hash, "hash");
    assert_eq!(parsed.name, "name");
    assert_eq!(parsed.argument.as_deref(), Some("arg"));

    let parsed = parse_remote_id("hash/name");
    assert_eq!(parsed.hash, "hash");
    assert_eq!(parsed.name, "name");
    assert_eq!(parsed.argument, None);
}

#[test]
fn renders_remote_json_responses() {
    let result = remote_json_response(&RemoteFunctionResponse::Result {
        result: "{\"ok\":true}".to_string(),
        refreshes: Some("{\"a\":1}".to_string()),
    });
    assert_eq!(result.status, 200);
    assert_eq!(
        result.body.as_deref(),
        Some(
            "{\"refreshes\":\"{\\\"a\\\":1}\",\"result\":\"{\\\"ok\\\":true}\",\"type\":\"result\"}"
        )
    );

    let redirect = remote_json_response(&RemoteFunctionResponse::Redirect {
        location: "/login".to_string(),
        refreshes: None,
    });
    assert_eq!(redirect.status, 200);
    assert_eq!(
        redirect.body.as_deref(),
        Some("{\"location\":\"/login\",\"type\":\"redirect\"}")
    );

    let error = remote_json_response(&RemoteFunctionResponse::Error {
        error: json!({ "message": "broken" }),
        status: 422,
    });
    assert_eq!(error.status, 422);
    assert_eq!(
        error.body.as_deref(),
        Some("{\"error\":{\"message\":\"broken\"},\"status\":422,\"type\":\"error\"}")
    );
}

#[test]
fn round_trips_remote_args_with_transport_hooks() {
    let app_state = date_roundtrip_app_state();

    let payload = stringify_remote_arg(
        &app_state,
        Some(&json!({
            "published": { "$type": "date", "value": "2026-03-12" }
        })),
    )
    .expect("encode remote arg");
    let decoded = parse_remote_arg(&app_state, &payload)
        .expect("parse remote arg")
        .expect("decoded remote arg");

    assert_eq!(
        decoded,
        json!({
            "published": { "$type": "date", "value": "2026-03-12" }
        })
    );
}

#[test]
fn serializes_remote_results_with_transport_hooks() {
    let app_state = date_encode_app_state();

    let request = RemoteCallRequest {
        id: parse_remote_id("hash/name"),
        kind: RemoteCallKind::Single,
        method: Method::GET,
        content_type: None,
        payload: Some(
            stringify_remote_arg(&app_state, Some(&json!({ "ok": true }))).expect("encode payload"),
        ),
        payloads: vec![],
        refreshes: vec![],
        form_data: Map::new(),
        form_meta_refreshes: vec![],
        prerendering: false,
    };

    let response = execute_remote_call(
        &request,
        &app_state,
        |invocation| match invocation {
            PreparedRemoteInvocation::Single { payload } => {
                assert_eq!(payload, Some(json!({ "ok": true })));
                Ok(RemoteCallExecution::Result {
                    result: json!({
                        "published": { "$type": "date", "value": "2026-03-12" }
                    }),
                    issues: false,
                })
            }
            other => panic!("unexpected invocation: {other:?}"),
        },
        |_key| Ok(None),
    )
    .expect("remote response");

    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"result\":\"{\\\"published\\\":{\\\"kind\\\":\\\"date\\\",\\\"type\\\":\\\"Transport\\\",\\\"value\\\":\\\"2026-03-12\\\"}}\",\"type\":\"result\"}"
        )
    );
}

#[test]
fn handles_remote_form_post_results() {
    let missing = handle_remote_form_post_result(Some("/blog"), true, false, || {
        panic!("missing remote form should not execute")
    });
    assert_eq!(
        missing
            .headers
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("GET")
    );
    match missing.result {
        ActionRequestResult::Error { error } => {
            assert!(error.to_string().contains("No form actions exist"));
        }
        other => panic!("expected missing-form error, got {other:?}"),
    }

    let success = handle_remote_form_post_result(Some("/blog"), false, true, || {
        RemoteFormExecutionResult::Success
    });
    assert_eq!(
        success.result,
        ActionRequestResult::Success {
            status: 200,
            data: None
        }
    );

    let redirect = handle_remote_form_post_result(Some("/blog"), false, true, || {
        RemoteFormExecutionResult::Redirect {
            status: 303,
            location: "/next".to_string(),
        }
    });
    assert_eq!(
        redirect.result,
        ActionRequestResult::Redirect {
            status: 303,
            location: "/next".to_string()
        }
    );

    let action_failure = handle_remote_form_post_result(Some("/blog"), false, true, || {
        RemoteFormExecutionResult::Error {
            error: "original".to_string(),
            is_action_failure: true,
        }
    });
    assert_eq!(
        action_failure.result,
        ActionRequestResult::Error {
            error: json!("Cannot \"throw fail()\". Use \"return fail()\"")
        }
    );
}

#[test]
fn executes_query_batch_remote_calls_and_rejects_wrong_method() {
    let app_state = default_app_state();
    let payload_a = stringify_remote_arg(&app_state, Some(&json!("a"))).expect("encode payload a");
    let payload_b = stringify_remote_arg(&app_state, Some(&json!("b"))).expect("encode payload b");
    let request = RemoteCallRequest {
        id: parse_remote_id("hash/name"),
        kind: RemoteCallKind::QueryBatch,
        method: Method::GET,
        content_type: None,
        payload: None,
        payloads: vec![payload_a.clone(), payload_b.clone()],
        refreshes: vec![],
        form_data: Map::new(),
        form_meta_refreshes: vec![],
        prerendering: false,
    };

    let rejected = execute_remote_call(
        &request,
        &app_state,
        |_invocation| panic!("invalid method should not execute"),
        |_key| panic!("invalid method should not resolve refreshes"),
    )
    .expect("remote rejection response");
    assert_eq!(rejected.status, 200);
    assert_eq!(
        rejected.body.as_deref(),
        Some(
            "{\"error\":{\"message\":\"`query.batch` functions must be invoked via POST request, not GET\"},\"status\":405,\"type\":\"error\"}"
        )
    );

    let request = RemoteCallRequest {
        method: Method::POST,
        ..request
    };
    let response = execute_remote_call(
        &request,
        &app_state,
        |invocation| match invocation {
            PreparedRemoteInvocation::QueryBatch { payloads } => {
                assert_eq!(payloads, vec![json!("a"), json!("b")]);
                Ok(RemoteCallExecution::Result {
                    result: json!([1, 2]),
                    issues: false,
                })
            }
            other => panic!("unexpected invocation: {other:?}"),
        },
        |_key| Ok(None),
    )
    .expect("query batch response");
    assert_eq!(
        response.body.as_deref(),
        Some("{\"result\":\"[1,2]\",\"type\":\"result\"}")
    );
}

#[test]
fn executes_form_remote_calls_with_id_injection_and_refreshes() {
    let app_state = default_app_state();
    let request = RemoteCallRequest {
        id: parse_remote_id("hash/name/%22abc%22"),
        kind: RemoteCallKind::Form,
        method: Method::POST,
        content_type: Some("application/x-www-form-urlencoded".to_string()),
        payload: None,
        payloads: vec![],
        refreshes: vec![],
        form_data: Map::from_iter([("title".to_string(), json!("hello"))]),
        form_meta_refreshes: vec!["refresh/a".to_string(), "refresh/b".to_string()],
        prerendering: false,
    };

    let response = execute_remote_call(
        &request,
        &app_state,
        |invocation| match invocation {
            PreparedRemoteInvocation::Form { data } => {
                assert_eq!(data.get("title"), Some(&json!("hello")));
                assert_eq!(data.get("id"), Some(&json!("abc")));
                Ok(RemoteCallExecution::Result {
                    result: json!({ "ok": true }),
                    issues: false,
                })
            }
            other => panic!("unexpected invocation: {other:?}"),
        },
        |key| Ok(Some(json!(format!("value:{key}")))),
    )
    .expect("form response");

    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"refreshes\":\"{\\\"refresh/a\\\":\\\"value:refresh/a\\\",\\\"refresh/b\\\":\\\"value:refresh/b\\\"}\",\"result\":\"{\\\"ok\\\":true}\",\"type\":\"result\"}"
        )
    );
}

#[test]
fn rejects_form_remote_calls_with_wrong_content_type() {
    let app_state = default_app_state();
    let request = RemoteCallRequest {
        id: parse_remote_id("hash/name"),
        kind: RemoteCallKind::Form,
        method: Method::POST,
        content_type: Some("application/json".to_string()),
        payload: None,
        payloads: vec![],
        refreshes: vec![],
        form_data: Map::new(),
        form_meta_refreshes: vec![],
        prerendering: true,
    };

    let response = execute_remote_call(
        &request,
        &app_state,
        |_invocation| panic!("invalid content type should not execute"),
        |_key| Ok(None),
    )
    .expect("form rejection response");
    assert_eq!(response.status, 415);
    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"error\":{\"message\":\"`form` functions expect form-encoded data — received application/json\"},\"status\":415,\"type\":\"error\"}"
        )
    );
}

#[test]
fn executes_command_remote_calls_with_refreshes_and_missing_refresh_is_bad_request() {
    let app_state = default_app_state();
    let payload = stringify_remote_arg(&app_state, Some(&json!({ "answer": 42 })))
        .expect("encode command payload");
    let request = RemoteCallRequest {
        id: parse_remote_id("hash/name"),
        kind: RemoteCallKind::Command,
        method: Method::POST,
        content_type: Some("application/json".to_string()),
        payload: Some(payload),
        payloads: vec![],
        refreshes: vec!["refresh/a".to_string(), "refresh/b".to_string()],
        form_data: Map::new(),
        form_meta_refreshes: vec![],
        prerendering: false,
    };

    let response = execute_remote_call(
        &request,
        &app_state,
        |invocation| match invocation {
            PreparedRemoteInvocation::Command { payload } => {
                assert_eq!(payload, Some(json!({ "answer": 42 })));
                Ok(RemoteCallExecution::Result {
                    result: json!({ "ok": true }),
                    issues: false,
                })
            }
            other => panic!("unexpected invocation: {other:?}"),
        },
        |key| Ok(Some(json!(format!("refresh:{key}")))),
    )
    .expect("command response");
    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"refreshes\":\"{\\\"refresh/a\\\":\\\"refresh:refresh/a\\\",\\\"refresh/b\\\":\\\"refresh:refresh/b\\\"}\",\"result\":\"{\\\"ok\\\":true}\",\"type\":\"result\"}"
        )
    );

    let missing_refresh = execute_remote_call(
        &request,
        &app_state,
        |_invocation| {
            Ok(RemoteCallExecution::Result {
                result: json!({ "ok": true }),
                issues: false,
            })
        },
        |_key| Ok(None),
    )
    .expect("bad request response");
    assert_eq!(missing_refresh.status, 400);
    assert_eq!(
        missing_refresh.body.as_deref(),
        Some("{\"error\":{\"message\":\"Bad Request\"},\"status\":400,\"type\":\"error\"}")
    );
}

#[test]
fn executes_single_and_prerender_remote_calls_with_upstream_payload_rules() {
    let app_state = default_app_state();
    let client_payload = stringify_remote_arg(&app_state, Some(&json!({ "client": true })))
        .expect("encode single payload");
    let single = RemoteCallRequest {
        id: parse_remote_id("hash/name"),
        kind: RemoteCallKind::Single,
        method: Method::GET,
        content_type: None,
        payload: Some(client_payload),
        payloads: vec![],
        refreshes: vec![],
        form_data: Map::new(),
        form_meta_refreshes: vec![],
        prerendering: false,
    };

    let single_response = execute_remote_call(
        &single,
        &app_state,
        |invocation| match invocation {
            PreparedRemoteInvocation::Single { payload } => {
                assert_eq!(payload, Some(json!({ "client": true })));
                Ok(RemoteCallExecution::Result {
                    result: json!({ "mode": "single" }),
                    issues: false,
                })
            }
            other => panic!("unexpected invocation: {other:?}"),
        },
        |_key| Ok(None),
    )
    .expect("single response");
    assert_eq!(
        single_response.body.as_deref(),
        Some("{\"result\":\"{\\\"mode\\\":\\\"single\\\"}\",\"type\":\"result\"}")
    );

    let prerender_payload = stringify_remote_arg(&app_state, Some(&json!({ "server": true })))
        .expect("encode prerender payload");
    let prerender = RemoteCallRequest {
        id: parse_remote_id(&format!("hash/name/{prerender_payload}")),
        kind: RemoteCallKind::Prerender,
        method: Method::GET,
        content_type: None,
        payload: Some("{\"ignored\":true}".to_string()),
        payloads: vec![],
        refreshes: vec![],
        form_data: Map::new(),
        form_meta_refreshes: vec![],
        prerendering: true,
    };

    let prerender_response = execute_remote_call(
        &prerender,
        &app_state,
        |invocation| match invocation {
            PreparedRemoteInvocation::Single { payload } => {
                assert_eq!(payload, Some(json!({ "server": true })));
                Ok(RemoteCallExecution::Error {
                    status: 418,
                    error: json!({ "message": "teapot" }),
                })
            }
            other => panic!("unexpected invocation: {other:?}"),
        },
        |_key| Ok(None),
    )
    .expect("prerender response");
    assert_eq!(prerender_response.status, 418);
    assert_eq!(
        prerender_response.body.as_deref(),
        Some("{\"error\":{\"message\":\"teapot\"},\"status\":418,\"type\":\"error\"}")
    );
}

#[test]
fn includes_refreshes_on_form_redirects() {
    let app_state = default_app_state();
    let request = RemoteCallRequest {
        id: parse_remote_id("hash/name/%22abc%22"),
        kind: RemoteCallKind::Form,
        method: Method::POST,
        content_type: Some("multipart/form-data; boundary=kit".to_string()),
        payload: None,
        payloads: vec![],
        refreshes: vec![],
        form_data: Map::new(),
        form_meta_refreshes: vec!["refresh/a".to_string()],
        prerendering: false,
    };

    let response = execute_remote_call(
        &request,
        &app_state,
        |invocation| match invocation {
            PreparedRemoteInvocation::Form { data } => {
                assert_eq!(data.get("id"), Some(&json!("abc")));
                Ok(RemoteCallExecution::Redirect {
                    status: 303,
                    location: "/next".to_string(),
                })
            }
            other => panic!("unexpected invocation: {other:?}"),
        },
        |key| Ok(Some(json!(format!("refresh:{key}")))),
    )
    .expect("form redirect response");
    assert_eq!(
        response.body.as_deref(),
        Some(
            "{\"location\":\"/next\",\"refreshes\":\"{\\\"refresh/a\\\":\\\"refresh:refresh/a\\\"}\",\"type\":\"redirect\"}"
        )
    );
}

#[test]
fn handles_remote_form_action_requests_from_page_posts() {
    let get_request = ServerRequest {
        method: Method::GET,
        url: Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22")
            .expect("valid get url"),
        headers: HeaderMap::new(),
    };
    assert!(
        handle_remote_form_action_request(&get_request, Some("/blog"), false, true, |_id| {
            panic!("GET should not execute remote action")
        })
        .expect("remote action resolution succeeds")
        .is_none()
    );

    let post_request = ServerRequest {
        method: Method::POST,
        url: Url::parse("https://example.com/blog?/remote=hash/name/%22abc%22")
            .expect("valid post url"),
        headers: HeaderMap::new(),
    };
    let result =
        handle_remote_form_action_request(&post_request, Some("/blog"), false, true, |id| {
            assert_eq!(id, "hash/name/\"abc\"");
            Ok(RemoteFormExecutionResult::Success)
        })
        .expect("remote action resolution succeeds")
        .expect("remote page action result");
    assert_eq!(
        result.result,
        ActionRequestResult::Success {
            status: 200,
            data: None
        }
    );

    let missing =
        handle_remote_form_action_request(&post_request, Some("/blog"), true, false, |_id| {
            panic!("missing form actions should not execute callback")
        })
        .expect("remote action resolution succeeds")
        .expect("missing remote form result");
    assert_eq!(
        missing
            .headers
            .get("allow")
            .and_then(|value| value.to_str().ok()),
        Some("GET")
    );
}
