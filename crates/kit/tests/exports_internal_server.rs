use std::collections::BTreeMap;
use std::sync::Arc;

use http::Method;
use svelte_kit::{
    AppState, Error, RequestStore, RequestStoreError, ServerRequestBuilder, TracingState,
    build_runtime_event, get_request_event, get_request_store, merge_tracing,
    try_get_request_store, with_request_store,
};
use url::Url;

fn event(url: &str) -> svelte_kit::RuntimeEvent {
    let request = ServerRequestBuilder::default()
        .method(Method::GET)
        .url(Url::parse(url).expect("url should parse"))
        .build()
        .expect("request should build");

    build_runtime_event(
        &request,
        Arc::new(AppState::default()),
        Url::parse(url).expect("url should parse"),
        Some("/test".to_string()),
        BTreeMap::new(),
        false,
        false,
        0,
    )
}

#[test]
fn request_store_is_available_inside_scope() {
    let store = RequestStore::new(event("https://example.com/test"));

    let route_id = with_request_store(Some(store.clone()), || {
        assert!(try_get_request_store().is_some());
        let event = get_request_event().expect("event should be available");
        event.route_id.clone()
    });

    assert_eq!(route_id.as_deref(), Some("/test"));
    assert!(try_get_request_store().is_none());
}

#[test]
fn get_request_store_errors_outside_scope() {
    let error = get_request_store().expect_err("store should be unavailable");
    assert!(matches!(
        error,
        Error::RequestStore(RequestStoreError::MissingRequestStore)
    ));
    assert!(
        error
            .to_string()
            .contains("Could not get the request store")
    );
}

#[test]
fn merge_tracing_replaces_current_span_only() {
    let tracing = TracingState {
        enabled: true,
        root: "root",
        current: "old",
    };

    let merged = merge_tracing(&tracing, "new");
    assert!(merged.enabled);
    assert_eq!(merged.root, "root");
    assert_eq!(merged.current, "new");
}
