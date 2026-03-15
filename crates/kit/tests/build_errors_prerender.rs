use http::Method;
use serde_json::{Map, json};
use svelte_kit::{
    AppState, Error, PrerenderError, RemoteCallExecution, RemoteCallKind, RemoteCallRequest,
    execute_remote_call, parse_remote_id, prerender_entry_generator_mismatch_error,
    prerender_unseen_routes_error, stringify_remote_arg,
};

#[test]
fn formats_unseen_prerender_routes_like_upstream() {
    let error = prerender_unseen_routes_error(&["/[x]"]);
    assert_eq!(
        error.to_string(),
        "The following routes were marked as prerenderable, but were not prerendered because they were not found while crawling your app:\n  - /[x]\n\nSee the `handleUnseenRoutes` option in https://svelte.dev/docs/kit/configuration#prerender for more info."
    );
    assert!(matches!(
        error,
        Error::Prerender(PrerenderError::UnseenRoutes { routes }) if routes == "  - /[x]"
    ));
}

#[test]
fn formats_entry_generator_mismatch_like_upstream() {
    let error = prerender_entry_generator_mismatch_error(
        "/[slug]/[notSpecific]",
        "/whatever/specific",
        "/[slug]/specific",
    );
    assert_eq!(
        error.to_string(),
        "The entries export from /[slug]/[notSpecific] generated entry /whatever/specific, which was matched by /[slug]/specific - see the `handleEntryGeneratorMismatch` option in https://svelte.dev/docs/kit/configuration#prerender for more info.\nTo suppress or handle this error, implement `handleEntryGeneratorMismatch` in https://svelte.dev/docs/kit/configuration#prerender"
    );
    assert!(matches!(
        error,
        Error::Prerender(PrerenderError::EntryGeneratorMismatch {
            generated_from_id,
            entry,
            matched_id
        }) if generated_from_id == "/[slug]/[notSpecific]"
            && entry == "/whatever/specific"
            && matched_id == "/[slug]/specific"
    ));
}

#[test]
fn remote_function_errors_bubble_through_prerender_execution() {
    let app_state = AppState::default();
    let request = RemoteCallRequest {
        id: parse_remote_id("hash/name"),
        kind: RemoteCallKind::Single,
        method: Method::GET,
        content_type: None,
        payload: Some(
            stringify_remote_arg(&app_state, Some(&json!({ "server": true })))
                .expect("encode payload"),
        ),
        payloads: vec![],
        refreshes: vec![],
        form_data: Map::new(),
        form_meta_refreshes: vec![],
        prerendering: true,
    };

    let response = execute_remote_call(
        &request,
        &app_state,
        |_request| {
            Ok(RemoteCallExecution::Error {
                status: 500,
                error: json!({ "message": "remote function blew up" }),
            })
        },
        |_invocation| Ok(None),
    )
    .expect("remote execution should succeed");

    let body = response.body.expect("error response should have body");
    assert!(body.contains("remote function blew up"));
}
