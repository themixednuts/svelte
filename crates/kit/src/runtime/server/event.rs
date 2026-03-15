use std::collections::BTreeMap;
use std::sync::Arc;

use url::Url;

use super::{AppState, RuntimeCookies, RuntimeEvent, RuntimeResponseHeaders, ServerRequest};

pub fn build_runtime_event(
    request: &ServerRequest,
    app_state: Arc<AppState>,
    rewritten_url: Url,
    route_id: Option<String>,
    params: BTreeMap<String, String>,
    is_data_request: bool,
    is_remote_request: bool,
    depth: usize,
) -> RuntimeEvent {
    RuntimeEvent {
        app_state,
        cookies: RuntimeCookies::new(request.header("cookie"), &rewritten_url),
        params,
        request: request.clone(),
        route_id,
        response_headers: RuntimeResponseHeaders::default(),
        url: rewritten_url,
        is_data_request,
        is_sub_request: depth > 0,
        is_remote_request,
    }
}
