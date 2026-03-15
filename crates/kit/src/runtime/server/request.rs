use std::sync::Arc;

use ::http::Method;
use http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};
use url::Url;

use crate::manifest::{KitManifest, ManifestRoute};
use crate::pathname::{
    has_data_suffix, has_resolution_suffix, strip_data_suffix, strip_resolution_suffix,
};
use crate::url::{decode_params, normalize_path, try_decode_pathname};
use crate::{RequestError, Result};

use super::{
    ActionJsonResult, ActionRequestResult, EndpointModule, ErrorPageRequestResult,
    ExecutedErrorPage, PageActionExecution, PageErrorBoundary, PageLoadResult, PageRequestResult,
    PreparedRuntimeExecution, PreprocessedRuntimeRequest, RenderedPage, RequestEventState,
    RequestKind, ResolvedRuntimeRequest, RuntimeEvent, RuntimeExecutionResult, RuntimeRenderState,
    RuntimeRequestOptions, RuntimeRespondResult, RuntimeRouteBehavior, RuntimeRouteDispatch,
    ServerRequest, ServerRequestEvent, ServerResponse, ShellPageResponse, action_json_response,
    build_runtime_event, check_csrf, check_remote_request_origin, dispatch_special_runtime_request,
    execute_page_request_with_action_and_stateful_load, finalize_route_response, get_remote_action,
    handle_remote_form_post, is_action_json_request, is_action_request, is_endpoint_request,
    maybe_not_modified_response, no_actions_action_json_response, no_actions_action_request_result,
    page_method_response, redirect_data_response, render_endpoint, render_shell_page_response,
    resolve_remote_request_url, resolve_runtime_route_behavior, response_with_vary_accept,
};

fn allow_get_header_map() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("allow"),
        HeaderValue::from_static("GET"),
    );
    headers
}

pub fn plain_text_response(status: u16, body: impl Into<String>) -> ServerResponse {
    ServerResponse::builder(status)
        .header("content-type", "text/plain; charset=utf-8")
        .expect("plain text response content-type is valid")
        .body(body.into())
        .build()
        .expect("plain text response is valid")
}

fn shell_runtime_result() -> RuntimeRespondResult {
    RuntimeRespondResult::Page(PageRequestResult::Shell(render_shell_page_response(
        200, true,
    )))
}

fn runtime_event_url(mut url: Url, state: &RuntimeRenderState) -> Url {
    if state
        .prerendering
        .as_ref()
        .is_some_and(|prerender| !prerender.fallback && !prerender.inside_reroute)
    {
        url.set_query(None);
    }

    url
}

pub fn apply_runtime_response_effects(
    response: &mut ServerResponse,
    effects: &super::RuntimeResponseEffects,
) {
    for (name, value) in &effects.headers {
        let Ok(value) = value.to_str() else {
            continue;
        };
        response.set_header(name.as_str(), value);
    }

    for header in &effects.set_cookie_headers {
        response.append_header("set-cookie", header.clone());
    }
}

fn should_ignore_early_response_for_fallback(
    response: &ServerResponse,
    state: &RuntimeRenderState,
) -> bool {
    state
        .prerendering
        .as_ref()
        .is_some_and(|prerender| prerender.fallback)
        && response.has_header("x-sveltekit-normalize")
}

fn attach_page_effects(
    mut result: PageRequestResult,
    effects: super::RuntimeResponseEffects,
) -> PageRequestResult {
    match &mut result {
        PageRequestResult::Rendered(rendered) => rendered.effects = effects,
        PageRequestResult::Shell(shell) => shell.effects = effects,
        PageRequestResult::ErrorBoundary(boundary) => boundary.effects = effects,
        PageRequestResult::Early(response) | PageRequestResult::Redirect(response) => {
            apply_runtime_response_effects(response, &effects);
        }
        PageRequestResult::Fatal {
            effects: fatal_effects,
            ..
        } => *fatal_effects = effects,
    }

    result
}

pub fn prepare_request_url(request_url: &Url) -> Result<super::PreparedRequestUrl> {
    let mut url = request_url.clone();
    let mut invalidated_data_nodes = None;
    let kind = if has_resolution_suffix(url.path()) {
        let pathname = strip_resolution_suffix(url.path());
        url.set_path(&pathname);
        RequestKind::RouteResolution
    } else if has_data_suffix(url.path()) {
        let stripped = strip_data_suffix(url.path());
        let trailing_slash = url
            .query_pairs()
            .any(|(key, value)| key == super::TRAILING_SLASH_PARAM && value == "1");
        let pathname = if trailing_slash {
            format!("{stripped}/")
        } else if stripped.is_empty() {
            "/".to_string()
        } else {
            stripped
        };
        url.set_path(&pathname);

        invalidated_data_nodes = url
            .query_pairs()
            .find(|(key, _)| key == super::INVALIDATED_PARAM)
            .map(|(_, value)| value.chars().map(|node| node == '1').collect());

        let retained = url
            .query_pairs()
            .filter(|(key, _)| {
                key != super::INVALIDATED_PARAM && key != super::TRAILING_SLASH_PARAM
            })
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();

        {
            let mut query = url.query_pairs_mut();
            query.clear();
            for (key, value) in retained {
                query.append_pair(&key, &value);
            }
        }

        RequestKind::Data
    } else {
        RequestKind::Page
    };

    Ok(super::PreparedRequestUrl {
        kind,
        url,
        invalidated_data_nodes,
    })
}

pub fn request_normalization_redirect(
    url: &Url,
    trailing_slash: &str,
    is_data_request: bool,
) -> Option<String> {
    if is_data_request {
        return None;
    }

    let normalized = normalize_path(url.path(), trailing_slash);
    if normalized == url.path() {
        return None;
    }

    let mut location = if normalized.starts_with("//") {
        format!("{}{}", url.origin().ascii_serialization(), normalized)
    } else {
        normalized
    };

    if url.query() != Some("")
        && let Some(query) = url.query()
    {
        location.push('?');
        location.push_str(query);
    }

    Some(location)
}

pub fn resolve_runtime_request<'a, F>(
    manifest: &'a KitManifest,
    request_url: &Url,
    base: &str,
    matches: F,
) -> Result<Option<ResolvedRuntimeRequest<'a>>>
where
    F: FnMut(&str, &str) -> bool,
{
    let prepared = prepare_request_url(request_url)?;
    let mut resolved_path = prepared.url.path().to_string();

    if !base.is_empty() {
        if !resolved_path.starts_with(base) {
            return Ok(None);
        }

        resolved_path = resolved_path[base.len()..].to_string();
        if resolved_path.is_empty() {
            resolved_path = "/".to_string();
        }
    }

    let Some(found) = manifest.find_matching_route(&resolved_path, matches) else {
        return Ok(None);
    };

    Ok(Some(ResolvedRuntimeRequest {
        prepared,
        resolved_path,
        route: found.route,
        params: decode_params(found.params)?,
    }))
}

pub fn runtime_normalization_response(
    request_url: &Url,
    prepared: &super::PreparedRequestUrl,
    behavior: &RuntimeRouteBehavior,
) -> Option<ServerResponse> {
    if prepared.kind == RequestKind::Data {
        return None;
    }

    let location = request_normalization_redirect(
        request_url,
        &behavior.trailing_slash,
        prepared.kind == RequestKind::Data,
    )?;

    let mut response = ServerResponse::new(308);
    response.set_header("x-sveltekit-normalize", "1");
    response.set_header("location", location);
    Some(response)
}

pub fn preprocess_runtime_request<'a, F>(
    manifest: &'a KitManifest,
    request: &ServerRequest,
    options: &RuntimeRequestOptions,
    state: &RuntimeRenderState,
    matches: F,
) -> Result<PreprocessedRuntimeRequest<'a>>
where
    F: Clone + FnMut(&str, &str) -> bool,
{
    let remote_id = super::get_remote_id(&request.url, &options.base, &options.app_dir);
    if let Some(response) = check_remote_request_origin(request, remote_id.as_deref()) {
        return Ok(PreprocessedRuntimeRequest {
            remote_id,
            prepared: prepare_request_url(&request.url)?,
            rewritten_url: request.url.clone(),
            resolved: None,
            early_response: Some(response),
        });
    }

    if let Some(response) = check_csrf(
        request,
        options.csrf_check_origin,
        &options.csrf_trusted_origins,
    ) {
        return Ok(PreprocessedRuntimeRequest {
            remote_id,
            prepared: prepare_request_url(&request.url)?,
            rewritten_url: request.url.clone(),
            resolved: None,
            early_response: Some(response),
        });
    }

    let rewritten_url = resolve_remote_request_url(request, &options.base, remote_id.as_deref())?;
    if try_decode_pathname(rewritten_url.path()).is_err() {
        return Ok(PreprocessedRuntimeRequest {
            remote_id,
            prepared: prepare_request_url(&rewritten_url)?,
            rewritten_url,
            resolved: None,
            early_response: Some(plain_text_response(400, "Malformed URI")),
        });
    }

    let special_options = super::SpecialRuntimeRequestOptions {
        app_dir: options.app_dir.clone(),
        base: options.base.clone(),
        hash_routing: options.hash_routing,
        public_env: &options.public_env,
        route_assets: &options.route_assets,
    };

    if !options.base.is_empty()
        && !rewritten_url.path().starts_with(&options.base)
        && !state
            .prerendering
            .as_ref()
            .is_some_and(|prerender| prerender.fallback)
    {
        return Ok(PreprocessedRuntimeRequest {
            remote_id,
            prepared: prepare_request_url(&rewritten_url)?,
            rewritten_url,
            resolved: None,
            early_response: Some(plain_text_response(404, "Not found")),
        });
    }

    if remote_id.is_none() {
        if let Some(response) =
            dispatch_special_runtime_request(manifest, request, &special_options, matches.clone())?
        {
            return Ok(PreprocessedRuntimeRequest {
                remote_id,
                prepared: prepare_request_url(&rewritten_url)?,
                rewritten_url,
                resolved: None,
                early_response: Some(response),
            });
        }
    }

    let prepared = prepare_request_url(&rewritten_url)?;
    let resolved = resolve_runtime_request(manifest, &rewritten_url, &options.base, matches)?;
    if let Some(resolved_request) = resolved.as_ref() {
        let behavior = resolve_runtime_route_behavior(
            manifest,
            resolved_request.route,
            rewritten_url.path(),
            &options.base,
        );
        if let Some(response) = runtime_normalization_response(&rewritten_url, &prepared, &behavior)
        {
            return Ok(PreprocessedRuntimeRequest {
                remote_id,
                prepared,
                rewritten_url,
                resolved: Some(resolved_request.clone()),
                early_response: Some(response),
            });
        }
    }

    Ok(PreprocessedRuntimeRequest {
        remote_id,
        prepared,
        rewritten_url,
        resolved,
        early_response: None,
    })
}

pub fn prepare_runtime_execution<'a, F>(
    manifest: &'a KitManifest,
    request: &ServerRequest,
    options: &RuntimeRequestOptions,
    state: &RuntimeRenderState,
    depth: usize,
    page_has_actions: bool,
    matches: F,
) -> Result<PreparedRuntimeExecution<'a>>
where
    F: Clone + FnMut(&str, &str) -> bool,
{
    let preprocessed = preprocess_runtime_request(manifest, request, options, state, matches)?;
    let Some(resolved) = preprocessed.resolved.as_ref() else {
        return Ok(PreparedRuntimeExecution {
            preprocessed,
            behavior: None,
            dispatch: None,
            event: None,
        });
    };

    let behavior = resolve_runtime_route_behavior(
        manifest,
        resolved.route,
        preprocessed.rewritten_url.path(),
        &options.base,
    );
    let dispatch = resolve_runtime_route_dispatch(
        resolved.route,
        request,
        preprocessed.prepared.kind == RequestKind::Data,
        page_has_actions,
    )?;
    let event_url = runtime_event_url(preprocessed.rewritten_url.clone(), state);
    let event = build_runtime_event(
        request,
        Arc::clone(&state.app_state),
        event_url,
        Some(resolved.route.id.clone()),
        resolved.params.clone(),
        preprocessed.prepared.kind == RequestKind::Data,
        preprocessed.remote_id.is_some(),
        depth,
    );
    event.cookies.set_trailing_slash(&behavior.trailing_slash)?;

    Ok(PreparedRuntimeExecution {
        preprocessed,
        behavior: Some(behavior),
        dispatch: Some(dispatch),
        event: Some(event),
    })
}

pub fn resolve_runtime_route_dispatch(
    route: &ManifestRoute,
    request: &ServerRequest,
    is_data_request: bool,
    page_has_actions: bool,
) -> Result<RuntimeRouteDispatch> {
    if is_data_request {
        return Ok(RuntimeRouteDispatch::Data);
    }

    if route.endpoint.is_some()
        && (route.page.is_none()
            || is_endpoint_request(
                &request.try_method()?,
                request.header("accept"),
                request.header("x-sveltekit-action"),
            ))
    {
        return Ok(RuntimeRouteDispatch::Endpoint);
    }

    if route.page.is_some() {
        if super::PAGE_METHODS.contains(&request.method.as_str()) {
            return Ok(RuntimeRouteDispatch::Page);
        }

        return Ok(RuntimeRouteDispatch::PageMethodNotAllowed(
            page_method_response(&request.try_method()?, page_has_actions)?,
        ));
    }

    Ok(RuntimeRouteDispatch::Endpoint)
}

pub fn execute_prepared_runtime_request<FD, FP>(
    prepared: &PreparedRuntimeExecution<'_>,
    state: &mut RuntimeRenderState,
    endpoint_module: Option<&EndpointModule>,
    mut execute_data: FD,
    mut execute_page: FP,
) -> Result<RuntimeExecutionResult>
where
    FD: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FP: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<PageRequestResult>,
{
    if let Some(response) = prepared.preprocessed.early_response.clone() {
        return Ok(RuntimeExecutionResult::Response(response));
    }

    let Some(resolved) = prepared.preprocessed.resolved.as_ref() else {
        return Ok(RuntimeExecutionResult::NotFound);
    };
    let behavior = prepared
        .behavior
        .as_ref()
        .ok_or(RequestError::MissingRuntimeRouteBehavior)?;
    let dispatch = prepared
        .dispatch
        .as_ref()
        .ok_or(RequestError::MissingRuntimeRouteDispatch)?;
    let event = prepared
        .event
        .as_ref()
        .ok_or(RequestError::MissingRuntimeEvent)?;

    match dispatch {
        RuntimeRouteDispatch::PageMethodNotAllowed(response) => {
            Ok(RuntimeExecutionResult::Response(response.clone()))
        }
        RuntimeRouteDispatch::Data => {
            let mut response = execute_data(resolved, behavior, event)?
                .ok_or(RequestError::InvalidDataRouteExecution)?;
            event.apply_response_effects(&mut response);
            Ok(RuntimeExecutionResult::Response(response))
        }
        RuntimeRouteDispatch::Endpoint => {
            let module = endpoint_module.ok_or(RequestError::MissingEndpointModule)?;
            let mut event_state = RequestEventState::default();
            let endpoint_event = ServerRequestEvent {
                request: event.request.clone(),
                route_id: Some(resolved.route.id.clone()),
            };
            let mut response = render_endpoint(&endpoint_event, &mut event_state, module, state)?;
            event.apply_response_effects(&mut response);
            Ok(RuntimeExecutionResult::Response(finalize_route_response(
                &event.request,
                resolved.route,
                &response,
            )))
        }
        RuntimeRouteDispatch::Page => {
            execute_page(resolved, behavior, event, state).map(RuntimeExecutionResult::Page)
        }
    }
}

pub fn execute_prepared_runtime_request_with_page_stage<FD, FJ, FA, FR, FL>(
    manifest: &KitManifest,
    prepared: &PreparedRuntimeExecution<'_>,
    state: &mut RuntimeRenderState,
    endpoint_module: Option<&EndpointModule>,
    page_has_actions: bool,
    dev: bool,
    mut execute_data: FD,
    mut execute_action_json: FJ,
    mut execute_action: FA,
    mut execute_remote_action: FR,
    mut load_page: FL,
) -> Result<RuntimeExecutionResult>
where
    FD: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FJ: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<ActionJsonResult>,
    FA: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<ActionRequestResult>,
    FR: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<super::RemoteFormExecutionResult>,
    FL: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
{
    if let Some(response) = prepared.preprocessed.early_response.clone() {
        return Ok(RuntimeExecutionResult::Response(response));
    }

    let Some(resolved) = prepared.preprocessed.resolved.as_ref() else {
        return Ok(RuntimeExecutionResult::NotFound);
    };
    let behavior = prepared
        .behavior
        .as_ref()
        .ok_or(RequestError::MissingRuntimeRouteBehavior)?;
    let dispatch = prepared
        .dispatch
        .as_ref()
        .ok_or(RequestError::MissingRuntimeRouteDispatch)?;
    let event = prepared
        .event
        .as_ref()
        .ok_or(RequestError::MissingRuntimeEvent)?;

    match dispatch {
        RuntimeRouteDispatch::PageMethodNotAllowed(response) => {
            Ok(RuntimeExecutionResult::Response(response.clone()))
        }
        RuntimeRouteDispatch::Data => {
            let mut response = execute_data(resolved, behavior, event)?
                .ok_or(RequestError::InvalidDataRouteExecution)?;
            event.apply_response_effects(&mut response);
            Ok(RuntimeExecutionResult::Response(response))
        }
        RuntimeRouteDispatch::Endpoint => {
            let module = endpoint_module.ok_or(RequestError::MissingEndpointModule)?;
            let mut event_state = RequestEventState::default();
            let endpoint_event = ServerRequestEvent {
                request: event.request.clone(),
                route_id: Some(resolved.route.id.clone()),
            };
            let mut response = render_endpoint(&endpoint_event, &mut event_state, module, state)?;
            event.apply_response_effects(&mut response);
            Ok(RuntimeExecutionResult::Response(finalize_route_response(
                &event.request,
                resolved.route,
                &response,
            )))
        }
        RuntimeRouteDispatch::Page => {
            if is_action_json_request(&event.request) {
                let mut response = if page_has_actions {
                    action_json_response(
                        &execute_action_json(resolved, behavior, event, state)?,
                        event.app_state.as_ref(),
                    )?
                } else {
                    no_actions_action_json_response(Some(resolved.route.id.as_str()), dev)
                };
                event.apply_response_effects(&mut response);
                return Ok(RuntimeExecutionResult::Response(response));
            }

            let action = if is_action_request(&event.request) {
                if let Some(remote_id) = get_remote_action(&event.request.url) {
                    if page_has_actions {
                        let remote_result =
                            execute_remote_action(resolved, behavior, event, state, &remote_id)?;
                        let post = handle_remote_form_post(
                            Some(resolved.route.id.as_str()),
                            dev,
                            true,
                            || remote_result,
                        );
                        Some(PageActionExecution {
                            headers: post.headers,
                            result: post.result,
                        })
                    } else {
                        Some(PageActionExecution {
                            headers: allow_get_header_map(),
                            result: no_actions_action_request_result(
                                Some(resolved.route.id.as_str()),
                                dev,
                            ),
                        })
                    }
                } else if page_has_actions {
                    Some(PageActionExecution {
                        headers: HeaderMap::new(),
                        result: execute_action(resolved, behavior, event, state)?,
                    })
                } else {
                    Some(PageActionExecution {
                        headers: allow_get_header_map(),
                        result: no_actions_action_request_result(
                            Some(resolved.route.id.as_str()),
                            dev,
                        ),
                    })
                }
            } else {
                None
            };

            let page = execute_page_request_with_action_and_stateful_load(
                manifest,
                resolved.route,
                event.request.url.path(),
                page_has_actions,
                state,
                action,
                200,
                |state, node_index, server_parent, parent| {
                    load_page(
                        resolved,
                        behavior,
                        event,
                        state,
                        node_index,
                        server_parent,
                        parent,
                    )
                },
            )?;
            Ok(RuntimeExecutionResult::Page(attach_page_effects(
                page,
                event.capture_response_effects(),
            )))
        }
    }
}

pub fn execute_prepared_runtime_request_with_named_page_stage<FD, FJ, FA, FR, FL>(
    manifest: &KitManifest,
    prepared: &PreparedRuntimeExecution<'_>,
    state: &mut RuntimeRenderState,
    endpoint_module: Option<&EndpointModule>,
    has_default_action: bool,
    named_action_count: usize,
    mut execute_data: FD,
    mut execute_action_json: FJ,
    mut execute_action: FA,
    mut execute_remote_action: FR,
    mut load_page: FL,
) -> Result<RuntimeExecutionResult>
where
    FD: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FJ: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<Option<ActionJsonResult>>,
    FA: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<Option<ActionRequestResult>>,
    FR: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<super::RemoteFormExecutionResult>,
    FL: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
{
    if let Some(response) = prepared.preprocessed.early_response.clone() {
        return Ok(RuntimeExecutionResult::Response(response));
    }

    let Some(resolved) = prepared.preprocessed.resolved.as_ref() else {
        return Ok(RuntimeExecutionResult::NotFound);
    };
    let behavior = prepared
        .behavior
        .as_ref()
        .ok_or(RequestError::MissingRuntimeRouteBehavior)?;
    let dispatch = prepared
        .dispatch
        .as_ref()
        .ok_or(RequestError::MissingRuntimeRouteDispatch)?;
    let event = prepared
        .event
        .as_ref()
        .ok_or(RequestError::MissingRuntimeEvent)?;

    match dispatch {
        RuntimeRouteDispatch::PageMethodNotAllowed(response) => {
            Ok(RuntimeExecutionResult::Response(response.clone()))
        }
        RuntimeRouteDispatch::Data => {
            let mut response = execute_data(resolved, behavior, event)?
                .ok_or(RequestError::InvalidDataRouteExecution)?;
            event.apply_response_effects(&mut response);
            Ok(RuntimeExecutionResult::Response(response))
        }
        RuntimeRouteDispatch::Endpoint => {
            let module = endpoint_module.ok_or(RequestError::MissingEndpointModule)?;
            let mut event_state = RequestEventState::default();
            let endpoint_event = ServerRequestEvent {
                request: event.request.clone(),
                route_id: Some(resolved.route.id.clone()),
            };
            let mut response = render_endpoint(&endpoint_event, &mut event_state, module, state)?;
            event.apply_response_effects(&mut response);
            Ok(RuntimeExecutionResult::Response(finalize_route_response(
                &event.request,
                resolved.route,
                &response,
            )))
        }
        RuntimeRouteDispatch::Page => {
            if state.depth > super::MAX_REQUEST_DEPTH {
                return Ok(RuntimeExecutionResult::Response(
                    super::plain_text_response(
                        404,
                        format!("Not found: {}", event.request.url.path()),
                    ),
                ));
            }

            if let Some(mut response) = super::execute_named_page_action_json_request_result(
                &event.request,
                event.app_state.as_ref(),
                Some(resolved.route.id.as_str()),
                false,
                has_default_action,
                named_action_count,
                |name| execute_action_json(resolved, behavior, event, state, name),
            )? {
                event.apply_response_effects(&mut response);
                return Ok(RuntimeExecutionResult::Response(response));
            }

            let action = if get_remote_action(&event.request.url).is_some() {
                super::resolve_page_action_request_result(
                    &event.request,
                    Some(resolved.route.id.as_str()),
                    false,
                    has_default_action || named_action_count > 0,
                    || -> Result<ActionRequestResult> {
                        unreachable!("remote action path bypasses local named action closure")
                    },
                    |remote_id| execute_remote_action(resolved, behavior, event, state, remote_id),
                )?
            } else {
                super::resolve_named_page_action_request_result(
                    &event.request,
                    has_default_action,
                    named_action_count,
                    |name| execute_action(resolved, behavior, event, state, name),
                )?
            };

            let page = execute_page_request_with_action_and_stateful_load(
                manifest,
                resolved.route,
                event.request.url.path(),
                has_default_action || named_action_count > 0,
                state,
                action,
                200,
                |state, node_index, server_parent, parent| {
                    load_page(
                        resolved,
                        behavior,
                        event,
                        state,
                        node_index,
                        server_parent,
                        parent,
                    )
                },
            )?;
            Ok(RuntimeExecutionResult::Page(attach_page_effects(
                page,
                event.capture_response_effects(),
            )))
        }
    }
}

pub fn respond_runtime_request<FMatch, FEndpoint, FData, FPage, FError, FFetch, FRemote, R>(
    manifest: &KitManifest,
    request: &ServerRequest,
    options: &RuntimeRequestOptions,
    state: &mut RuntimeRenderState,
    page_has_actions: bool,
    dev: bool,
    render_error: R,
    matches: FMatch,
    endpoint_module: FEndpoint,
    execute_data: FData,
    execute_page: FPage,
    mut execute_error_page: FError,
    mut execute_remote: FRemote,
    mut fetch_runtime_response: FFetch,
) -> Result<RuntimeRespondResult>
where
    FMatch: Clone + FnMut(&str, &str) -> bool,
    FEndpoint: FnOnce(Option<&ResolvedRuntimeRequest<'_>>) -> Option<EndpointModule>,
    FData: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FPage: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<PageRequestResult>,
    FError: FnMut(u16, Value, bool) -> Result<super::ErrorPageRequestResult>,
    FFetch: FnMut(&ServerRequest) -> Result<ServerResponse>,
    FRemote: FnMut(&str, &RuntimeEvent, &mut RuntimeRenderState) -> Result<ServerResponse>,
    R: FnOnce(u16, &str) -> String + Clone,
{
    let prepared = prepare_runtime_execution(
        manifest,
        request,
        options,
        state,
        state.depth,
        page_has_actions,
        matches,
    )?;
    let resolved = prepared.preprocessed.resolved.as_ref();
    let owned_endpoint_module = endpoint_module(resolved);

    if let Some(response) = prepared.preprocessed.early_response.clone()
        && !should_ignore_early_response_for_fallback(&response, state)
    {
        return Ok(RuntimeRespondResult::Response(response));
    }

    if options.hash_routing
        || state
            .prerendering
            .as_ref()
            .is_some_and(|prerender| prerender.fallback)
    {
        return Ok(shell_runtime_result());
    }

    if let Some(remote_id) = prepared.preprocessed.remote_id.as_deref() {
        let event = build_runtime_event(
            request,
            Arc::clone(&state.app_state),
            runtime_event_url(prepared.preprocessed.rewritten_url.clone(), state),
            resolved.map(|resolved| resolved.route.id.clone()),
            resolved
                .map(|resolved| resolved.params.clone())
                .unwrap_or_default(),
            prepared.preprocessed.prepared.kind == RequestKind::Data,
            true,
            state.depth,
        );
        if let Some(behavior) = prepared.behavior.as_ref() {
            event.cookies.set_trailing_slash(&behavior.trailing_slash)?;
        }
        return execute_remote(remote_id, &event, state).map(|mut response| {
            event.apply_response_effects(&mut response);
            RuntimeRespondResult::Response(response)
        });
    }

    match execute_prepared_runtime_request(
        &prepared,
        state,
        owned_endpoint_module.as_ref(),
        execute_data,
        execute_page,
    )? {
        RuntimeExecutionResult::Response(response) => {
            Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                request,
                prepared.preprocessed.prepared.kind,
                resolved,
                state,
                response,
            )))
        }
        RuntimeExecutionResult::Page(page) => Ok(RuntimeRespondResult::Page(page)),
        RuntimeExecutionResult::NotFound => {
            if state.error && state.depth > 0 {
                return fetch_runtime_response(&delegated_error_request(request)).map(|response| {
                    RuntimeRespondResult::Response(finalize_runtime_response(
                        request,
                        prepared.preprocessed.prepared.kind,
                        resolved,
                        state,
                        response,
                    ))
                });
            }

            if state.error {
                return Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    plain_text_response(500, "Internal Server Error"),
                )));
            }

            if state.prerendering.is_some() {
                return Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    plain_text_response(404, "not found"),
                )));
            }

            if state.depth == 0 {
                let _ = render_error;
                let _ = dev;
                let error = serde_json::json!({
                    "message": format!("Not found: {}", request.url.path()),
                });
                return execute_error_page(404, error, false).map(RuntimeRespondResult::ErrorPage);
            }

            fetch_runtime_response(request).map(|response| {
                RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    response,
                ))
            })
        }
    }
}

pub fn respond_runtime_request_with_page_stage<
    FMatch,
    FEndpoint,
    FData,
    FActionJson,
    FAction,
    FRemoteAction,
    FLoadPage,
    FError,
    FFetch,
    FRemote,
    R,
>(
    manifest: &KitManifest,
    request: &ServerRequest,
    options: &RuntimeRequestOptions,
    state: &mut RuntimeRenderState,
    page_has_actions: bool,
    dev: bool,
    render_error: R,
    matches: FMatch,
    endpoint_module: FEndpoint,
    execute_data: FData,
    execute_action_json: FActionJson,
    execute_action: FAction,
    execute_remote_action: FRemoteAction,
    load_page: FLoadPage,
    mut execute_error_page: FError,
    mut execute_remote: FRemote,
    mut fetch_runtime_response: FFetch,
) -> Result<RuntimeRespondResult>
where
    FMatch: Clone + FnMut(&str, &str) -> bool,
    FEndpoint: FnOnce(Option<&ResolvedRuntimeRequest<'_>>) -> Option<EndpointModule>,
    FData: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FActionJson: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<ActionJsonResult>,
    FAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<ActionRequestResult>,
    FRemoteAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<super::RemoteFormExecutionResult>,
    FLoadPage: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
    FError: FnMut(u16, Value, bool) -> Result<super::ErrorPageRequestResult>,
    FFetch: FnMut(&ServerRequest) -> Result<ServerResponse>,
    FRemote: FnMut(&str, &RuntimeEvent, &mut RuntimeRenderState) -> Result<ServerResponse>,
    R: FnOnce(u16, &str) -> String + Clone,
{
    let prepared = prepare_runtime_execution(
        manifest,
        request,
        options,
        state,
        state.depth,
        page_has_actions,
        matches,
    )?;
    let resolved = prepared.preprocessed.resolved.as_ref();
    let owned_endpoint_module = endpoint_module(resolved);

    if let Some(response) = prepared.preprocessed.early_response.clone()
        && !should_ignore_early_response_for_fallback(&response, state)
    {
        return Ok(RuntimeRespondResult::Response(response));
    }

    if options.hash_routing
        || state
            .prerendering
            .as_ref()
            .is_some_and(|prerender| prerender.fallback)
    {
        return Ok(shell_runtime_result());
    }

    if let Some(remote_id) = prepared.preprocessed.remote_id.as_deref() {
        let event = build_runtime_event(
            request,
            Arc::clone(&state.app_state),
            runtime_event_url(prepared.preprocessed.rewritten_url.clone(), state),
            resolved.map(|resolved| resolved.route.id.clone()),
            resolved
                .map(|resolved| resolved.params.clone())
                .unwrap_or_default(),
            prepared.preprocessed.prepared.kind == RequestKind::Data,
            true,
            state.depth,
        );
        if let Some(behavior) = prepared.behavior.as_ref() {
            event.cookies.set_trailing_slash(&behavior.trailing_slash)?;
        }
        return execute_remote(remote_id, &event, state).map(|mut response| {
            event.apply_response_effects(&mut response);
            RuntimeRespondResult::Response(response)
        });
    }

    match execute_prepared_runtime_request_with_page_stage(
        manifest,
        &prepared,
        state,
        owned_endpoint_module.as_ref(),
        page_has_actions,
        dev,
        execute_data,
        execute_action_json,
        execute_action,
        execute_remote_action,
        load_page,
    )? {
        RuntimeExecutionResult::Response(response) => {
            Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                request,
                prepared.preprocessed.prepared.kind,
                resolved,
                state,
                response,
            )))
        }
        RuntimeExecutionResult::Page(page) => Ok(RuntimeRespondResult::Page(page)),
        RuntimeExecutionResult::NotFound => {
            if state.error && state.depth > 0 {
                return fetch_runtime_response(&delegated_error_request(request)).map(|response| {
                    RuntimeRespondResult::Response(finalize_runtime_response(
                        request,
                        prepared.preprocessed.prepared.kind,
                        resolved,
                        state,
                        response,
                    ))
                });
            }

            if state.error {
                return Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    plain_text_response(500, "Internal Server Error"),
                )));
            }

            if state.prerendering.is_some() {
                return Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    plain_text_response(404, "not found"),
                )));
            }

            if state.depth == 0 {
                let _ = render_error;
                let _ = dev;
                let error = serde_json::json!({
                    "message": format!("Not found: {}", request.url.path()),
                });
                return execute_error_page(404, error, false).map(RuntimeRespondResult::ErrorPage);
            }

            fetch_runtime_response(request).map(|response| {
                RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    response,
                ))
            })
        }
    }
}

pub fn respond_runtime_request_with_named_page_stage<
    FMatch,
    FEndpoint,
    FData,
    FActionJson,
    FAction,
    FRemoteAction,
    FLoadPage,
    FError,
    FFetch,
    FRemote,
    R,
>(
    manifest: &KitManifest,
    request: &ServerRequest,
    options: &RuntimeRequestOptions,
    state: &mut RuntimeRenderState,
    has_default_action: bool,
    named_action_count: usize,
    render_error: R,
    matches: FMatch,
    endpoint_module: FEndpoint,
    execute_data: FData,
    execute_action_json: FActionJson,
    execute_action: FAction,
    execute_remote_action: FRemoteAction,
    load_page: FLoadPage,
    mut execute_error_page: FError,
    mut execute_remote: FRemote,
    mut fetch_runtime_response: FFetch,
) -> Result<RuntimeRespondResult>
where
    FMatch: Clone + FnMut(&str, &str) -> bool,
    FEndpoint: FnOnce(Option<&ResolvedRuntimeRequest<'_>>) -> Option<EndpointModule>,
    FData: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FActionJson: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<Option<ActionJsonResult>>,
    FAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<Option<ActionRequestResult>>,
    FRemoteAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<super::RemoteFormExecutionResult>,
    FLoadPage: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
    FError: FnMut(u16, Value, bool) -> Result<super::ErrorPageRequestResult>,
    FFetch: FnMut(&ServerRequest) -> Result<ServerResponse>,
    FRemote: FnMut(&str, &RuntimeEvent, &mut RuntimeRenderState) -> Result<ServerResponse>,
    R: FnOnce(u16, &str) -> String + Clone,
{
    let prepared = prepare_runtime_execution(
        manifest,
        request,
        options,
        state,
        state.depth,
        has_default_action || named_action_count > 0,
        matches,
    )?;
    let resolved = prepared.preprocessed.resolved.as_ref();
    let owned_endpoint_module = endpoint_module(resolved);

    if let Some(response) = prepared.preprocessed.early_response.clone()
        && !should_ignore_early_response_for_fallback(&response, state)
    {
        return Ok(RuntimeRespondResult::Response(response));
    }

    if options.hash_routing
        || state
            .prerendering
            .as_ref()
            .is_some_and(|prerender| prerender.fallback)
    {
        return Ok(shell_runtime_result());
    }

    if let Some(remote_id) = prepared.preprocessed.remote_id.as_deref() {
        let event = build_runtime_event(
            request,
            Arc::clone(&state.app_state),
            runtime_event_url(prepared.preprocessed.rewritten_url.clone(), state),
            resolved.map(|resolved| resolved.route.id.clone()),
            resolved
                .map(|resolved| resolved.params.clone())
                .unwrap_or_default(),
            prepared.preprocessed.prepared.kind == RequestKind::Data,
            true,
            state.depth,
        );
        if let Some(behavior) = prepared.behavior.as_ref() {
            event.cookies.set_trailing_slash(&behavior.trailing_slash)?;
        }
        return execute_remote(remote_id, &event, state).map(|mut response| {
            event.apply_response_effects(&mut response);
            RuntimeRespondResult::Response(response)
        });
    }

    match execute_prepared_runtime_request_with_named_page_stage(
        manifest,
        &prepared,
        state,
        owned_endpoint_module.as_ref(),
        has_default_action,
        named_action_count,
        execute_data,
        execute_action_json,
        execute_action,
        execute_remote_action,
        load_page,
    )? {
        RuntimeExecutionResult::Response(response) => {
            Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                request,
                prepared.preprocessed.prepared.kind,
                resolved,
                state,
                response,
            )))
        }
        RuntimeExecutionResult::Page(page) => Ok(RuntimeRespondResult::Page(page)),
        RuntimeExecutionResult::NotFound => {
            if state.error && state.depth > 0 {
                return fetch_runtime_response(&delegated_error_request(request)).map(|response| {
                    RuntimeRespondResult::Response(finalize_runtime_response(
                        request,
                        prepared.preprocessed.prepared.kind,
                        resolved,
                        state,
                        response,
                    ))
                });
            }

            if state.error {
                return Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    plain_text_response(500, "Internal Server Error"),
                )));
            }

            if state.prerendering.is_some() {
                return Ok(RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    plain_text_response(404, "not found"),
                )));
            }

            if state.depth == 0 {
                let _ = render_error;
                let error = serde_json::json!({
                    "message": format!("Not found: {}", request.url.path()),
                });
                return execute_error_page(404, error, false).map(RuntimeRespondResult::ErrorPage);
            }

            fetch_runtime_response(request).map(|response| {
                RuntimeRespondResult::Response(finalize_runtime_response(
                    request,
                    prepared.preprocessed.prepared.kind,
                    resolved,
                    state,
                    response,
                ))
            })
        }
    }
}

fn delegated_error_request(request: &ServerRequest) -> ServerRequest {
    let mut delegated = request.clone();
    delegated.set_header("x-sveltekit-error", "true");
    delegated
}

fn finalize_runtime_response(
    request: &ServerRequest,
    request_kind: RequestKind,
    resolved: Option<&ResolvedRuntimeRequest<'_>>,
    state: &RuntimeRenderState,
    response: ServerResponse,
) -> ServerResponse {
    let mut response = response;

    if request_kind == RequestKind::Data
        && (300..=308).contains(&response.status.as_u16())
        && let Some(location) = response.header("location")
    {
        return redirect_data_response(location);
    }

    if request.method_is(&Method::GET)
        && let Some(resolved) = resolved
        && resolved.route.page.is_some()
        && resolved.route.endpoint.is_some()
    {
        response = response_with_vary_accept(&response);
    }

    if state.prerendering.is_some()
        && let Some(resolved) = resolved
    {
        response.set_header("x-sveltekit-routeid", resolved.route.id.clone());
    }

    if request.method_is(&Method::HEAD) {
        response.body = None;
    }

    maybe_not_modified_response(request, &response).unwrap_or(response)
}

fn apply_action_headers(response: &mut ServerResponse, action: Option<&PageActionExecution>) {
    let Some(action) = action else {
        return;
    };

    for (name, value) in &action.headers {
        let Ok(value) = value.to_str() else {
            continue;
        };
        response.set_header(name.as_str(), value);
    }
}

pub fn materialize_error_page_request_result<F>(
    result: ErrorPageRequestResult,
    render_error_page: F,
) -> Result<ServerResponse>
where
    F: FnOnce(&ExecutedErrorPage) -> Result<ServerResponse>,
{
    match result {
        ErrorPageRequestResult::Rendered(rendered) => render_error_page(&rendered),
        ErrorPageRequestResult::Redirect(response) | ErrorPageRequestResult::Static(response) => {
            Ok(response)
        }
    }
}

pub fn materialize_page_request_result<FShell, FPage, FBoundary, FFatal, FErrorPage>(
    result: PageRequestResult,
    execute_error_page: FFatal,
    render_shell: FShell,
    render_page: FPage,
    render_boundary: FBoundary,
    render_error_page: FErrorPage,
) -> Result<ServerResponse>
where
    FShell: FnOnce(&ShellPageResponse) -> Result<ServerResponse>,
    FPage: FnOnce(&RenderedPage) -> Result<ServerResponse>,
    FBoundary: FnOnce(&PageErrorBoundary) -> Result<ServerResponse>,
    FFatal: FnOnce(u16, Value, bool) -> Result<ErrorPageRequestResult>,
    FErrorPage: FnOnce(&ExecutedErrorPage) -> Result<ServerResponse>,
{
    match result {
        PageRequestResult::Rendered(rendered) => {
            let mut response = render_page(&rendered)?;
            apply_action_headers(&mut response, rendered.action.as_ref());
            apply_runtime_response_effects(&mut response, &rendered.effects);
            Ok(response)
        }
        PageRequestResult::Shell(shell) => {
            let mut response = render_shell(&shell)?;
            apply_action_headers(&mut response, shell.action.as_ref());
            apply_runtime_response_effects(&mut response, &shell.effects);
            Ok(response)
        }
        PageRequestResult::Early(response) | PageRequestResult::Redirect(response) => Ok(response),
        PageRequestResult::ErrorBoundary(boundary) => {
            let mut response = render_boundary(&boundary)?;
            apply_action_headers(&mut response, boundary.action.as_ref());
            apply_runtime_response_effects(&mut response, &boundary.effects);
            Ok(response)
        }
        PageRequestResult::Fatal {
            status,
            error,
            effects,
        } => execute_error_page(status, error, false).and_then(|result| {
            let mut response = materialize_error_page_request_result(result, render_error_page)?;
            apply_runtime_response_effects(&mut response, &effects);
            Ok(response)
        }),
    }
}

pub fn respond_runtime_request_materialized_with_page_stage<
    FMatch,
    FEndpoint,
    FData,
    FActionJson,
    FAction,
    FRemoteAction,
    FLoadPage,
    FError,
    FFetch,
    FRemote,
    R,
    FShell,
    FPage,
    FBoundary,
    FErrorPage,
>(
    manifest: &KitManifest,
    request: &ServerRequest,
    options: &RuntimeRequestOptions,
    state: &mut RuntimeRenderState,
    page_has_actions: bool,
    dev: bool,
    render_error: R,
    matches: FMatch,
    endpoint_module: FEndpoint,
    execute_data: FData,
    execute_action_json: FActionJson,
    execute_action: FAction,
    execute_remote_action: FRemoteAction,
    load_page: FLoadPage,
    mut execute_error_page: FError,
    execute_remote: FRemote,
    fetch_runtime_response: FFetch,
    render_shell: FShell,
    render_page: FPage,
    render_boundary: FBoundary,
    render_error_page: FErrorPage,
) -> Result<ServerResponse>
where
    FMatch: Clone + FnMut(&str, &str) -> bool,
    FEndpoint: FnOnce(Option<&ResolvedRuntimeRequest<'_>>) -> Option<EndpointModule>,
    FData: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FActionJson: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<ActionJsonResult>,
    FAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
    ) -> Result<ActionRequestResult>,
    FRemoteAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<super::RemoteFormExecutionResult>,
    FLoadPage: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
    FError: FnMut(u16, Value, bool) -> Result<super::ErrorPageRequestResult>,
    FFetch: FnMut(&ServerRequest) -> Result<ServerResponse>,
    FRemote: FnMut(&str, &RuntimeEvent, &mut RuntimeRenderState) -> Result<ServerResponse>,
    R: FnOnce(u16, &str) -> String + Clone,
    FShell: FnOnce(&ShellPageResponse) -> Result<ServerResponse>,
    FPage: FnOnce(&RenderedPage) -> Result<ServerResponse>,
    FBoundary: FnOnce(&PageErrorBoundary) -> Result<ServerResponse>,
    FErrorPage: FnOnce(&ExecutedErrorPage) -> Result<ServerResponse>,
{
    let finalization_kind = prepare_request_url(&request.url)?.kind;
    let finalization_resolved =
        resolve_runtime_request(manifest, &request.url, &options.base, matches.clone())?;

    match respond_runtime_request_with_page_stage(
        manifest,
        request,
        options,
        state,
        page_has_actions,
        dev,
        render_error,
        matches,
        endpoint_module,
        execute_data,
        execute_action_json,
        execute_action,
        execute_remote_action,
        load_page,
        &mut execute_error_page,
        execute_remote,
        fetch_runtime_response,
    )? {
        RuntimeRespondResult::Response(response) => Ok(finalize_runtime_response(
            request,
            finalization_kind,
            finalization_resolved.as_ref(),
            state,
            response,
        )),
        RuntimeRespondResult::Page(page) => materialize_page_request_result(
            page,
            execute_error_page,
            render_shell,
            render_page,
            render_boundary,
            render_error_page,
        )
        .map(|response| {
            finalize_runtime_response(
                request,
                finalization_kind,
                finalization_resolved.as_ref(),
                state,
                response,
            )
        }),
        RuntimeRespondResult::ErrorPage(error_page) => {
            materialize_error_page_request_result(error_page, render_error_page).map(|response| {
                finalize_runtime_response(
                    request,
                    finalization_kind,
                    finalization_resolved.as_ref(),
                    state,
                    response,
                )
            })
        }
    }
}

pub fn respond_runtime_request_materialized_with_named_page_stage<
    FMatch,
    FEndpoint,
    FData,
    FActionJson,
    FAction,
    FRemoteAction,
    FLoadPage,
    FError,
    FFetch,
    FRemote,
    R,
    FShell,
    FPage,
    FBoundary,
    FErrorPage,
>(
    manifest: &KitManifest,
    request: &ServerRequest,
    options: &RuntimeRequestOptions,
    state: &mut RuntimeRenderState,
    has_default_action: bool,
    named_action_count: usize,
    render_error: R,
    matches: FMatch,
    endpoint_module: FEndpoint,
    execute_data: FData,
    execute_action_json: FActionJson,
    execute_action: FAction,
    execute_remote_action: FRemoteAction,
    load_page: FLoadPage,
    mut execute_error_page: FError,
    execute_remote: FRemote,
    fetch_runtime_response: FFetch,
    render_shell: FShell,
    render_page: FPage,
    render_boundary: FBoundary,
    render_error_page: FErrorPage,
) -> Result<ServerResponse>
where
    FMatch: Clone + FnMut(&str, &str) -> bool,
    FEndpoint: FnOnce(Option<&ResolvedRuntimeRequest<'_>>) -> Option<EndpointModule>,
    FData: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
    ) -> Result<Option<ServerResponse>>,
    FActionJson: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<Option<ActionJsonResult>>,
    FAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<Option<ActionRequestResult>>,
    FRemoteAction: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        &str,
    ) -> Result<super::RemoteFormExecutionResult>,
    FLoadPage: FnMut(
        &ResolvedRuntimeRequest<'_>,
        &RuntimeRouteBehavior,
        &RuntimeEvent,
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
    FError: FnMut(u16, Value, bool) -> Result<super::ErrorPageRequestResult>,
    FFetch: FnMut(&ServerRequest) -> Result<ServerResponse>,
    FRemote: FnMut(&str, &RuntimeEvent, &mut RuntimeRenderState) -> Result<ServerResponse>,
    R: FnOnce(u16, &str) -> String + Clone,
    FShell: FnOnce(&ShellPageResponse) -> Result<ServerResponse>,
    FPage: FnOnce(&RenderedPage) -> Result<ServerResponse>,
    FBoundary: FnOnce(&PageErrorBoundary) -> Result<ServerResponse>,
    FErrorPage: FnOnce(&ExecutedErrorPage) -> Result<ServerResponse>,
{
    let finalization_kind = prepare_request_url(&request.url)?.kind;
    let finalization_resolved =
        resolve_runtime_request(manifest, &request.url, &options.base, matches.clone())?;

    match respond_runtime_request_with_named_page_stage(
        manifest,
        request,
        options,
        state,
        has_default_action,
        named_action_count,
        render_error,
        matches,
        endpoint_module,
        execute_data,
        execute_action_json,
        execute_action,
        execute_remote_action,
        load_page,
        &mut execute_error_page,
        execute_remote,
        fetch_runtime_response,
    )? {
        RuntimeRespondResult::Response(response) => Ok(finalize_runtime_response(
            request,
            finalization_kind,
            finalization_resolved.as_ref(),
            state,
            response,
        )),
        RuntimeRespondResult::Page(page) => materialize_page_request_result(
            page,
            execute_error_page,
            render_shell,
            render_page,
            render_boundary,
            render_error_page,
        )
        .map(|response| {
            finalize_runtime_response(
                request,
                finalization_kind,
                finalization_resolved.as_ref(),
                state,
                response,
            )
        }),
        RuntimeRespondResult::ErrorPage(error_page) => {
            materialize_error_page_request_result(error_page, render_error_page).map(|response| {
                finalize_runtime_response(
                    request,
                    finalization_kind,
                    finalization_resolved.as_ref(),
                    state,
                    response,
                )
            })
        }
    }
}
