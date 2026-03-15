use http::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};

use crate::http::is_form_content_type;
use crate::manifest::{KitManifest, ManifestRoute};
use crate::pathname::add_data_suffix;
use crate::runtime::shared::validate_load_response;
use crate::{Result, RuntimePageError};

use super::{
    ActionJsonResult, ActionRequestResult, ErrorPageRenderPlan, ErrorPageRequestResult,
    ExecutedErrorPage, PageActionExecution, PageErrorBoundary, PageExecutionResult, PageLoadResult,
    PageLoadedNode, PageRenderPlan, PageRequestResult, PageRuntimeDecision, RenderedPage,
    RuntimeEvent, RuntimePageNodes, RuntimeRenderState, ServerRequest, ServerResponse,
    action_json_response, get_remote_action, handle_remote_form_action_request_result,
    is_action_json_request, is_action_request, no_actions_action_json_response,
    no_actions_action_request_result, redirect_response, static_error_page,
};

pub(crate) const MAX_REQUEST_DEPTH: usize = 10;

pub fn prepare_page_render_plan(
    manifest: &KitManifest,
    route: &ManifestRoute,
    request_pathname: &str,
) -> Option<PageRenderPlan> {
    let page = route.page.as_ref()?;
    let nodes = RuntimePageNodes::from_route(page, manifest);

    Some(PageRenderPlan {
        ssr: nodes.ssr(),
        csr: nodes.csr(),
        prerender: nodes.prerender(),
        should_prerender_data: nodes.should_prerender_data(),
        data_pathname: add_data_suffix(request_pathname),
    })
}

pub fn page_request_requires_shell_only(plan: &PageRenderPlan, prerendering: bool) -> bool {
    !plan.ssr && !(prerendering && plan.should_prerender_data)
}

pub fn prepare_error_page_render(
    manifest: &KitManifest,
    status: u16,
    error: Value,
) -> Option<ErrorPageRenderPlan> {
    let layout = manifest.nodes.first()?;
    let _error_node = manifest.nodes.get(1)?;
    let nodes = RuntimePageNodes {
        nodes: vec![Some(layout)],
    };

    Some(ErrorPageRenderPlan {
        status,
        error,
        ssr: nodes.ssr(),
        csr: nodes.csr(),
        branch_node_indexes: vec![0, 1],
    })
}

pub fn apply_page_prerender_policy(
    route_id: Option<&str>,
    plan: &PageRenderPlan,
    has_actions: bool,
    state: &mut RuntimeRenderState,
) -> Result<Option<ServerResponse>> {
    if plan.prerender && has_actions {
        return Err(RuntimePageError::PrerenderActions.into());
    }

    if !plan.prerender
        && let Some(prerendering) = &state.prerendering
        && !prerendering.inside_reroute
    {
        if state.depth > 0 {
            return Err(RuntimePageError::NotPrerenderable {
                route_id: route_id.unwrap_or_default().to_string(),
            }
            .into());
        }

        return Ok(Some(ServerResponse::new(204)));
    }

    state.prerender_default = plan.prerender;
    Ok(None)
}

pub fn render_shell_page_response(status: u16, csr: bool) -> super::ShellPageResponse {
    super::ShellPageResponse {
        status,
        ssr: false,
        csr,
        action: None,
        effects: Default::default(),
    }
}

pub fn resolve_page_runtime_decision(
    route_id: Option<&str>,
    plan: PageRenderPlan,
    has_actions: bool,
    state: &mut RuntimeRenderState,
    status: u16,
) -> Result<PageRuntimeDecision> {
    if let Some(response) = apply_page_prerender_policy(route_id, &plan, has_actions, state)? {
        return Ok(PageRuntimeDecision::Early(response));
    }

    if page_request_requires_shell_only(&plan, state.prerendering.is_some()) {
        return Ok(PageRuntimeDecision::Shell(render_shell_page_response(
            status, plan.csr,
        )));
    }

    Ok(PageRuntimeDecision::Render(plan))
}

fn execute_page_load_internal<F>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    action_error: Option<(u16, Value)>,
    mut load: F,
) -> Result<PageExecutionResult>
where
    F: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    let Some(page) = route.page.as_ref() else {
        return Err(RuntimePageError::NonPageLoadRoute.into());
    };

    let mut branch = Vec::with_capacity(page.layouts.len() + 1);
    let mut parent_server_data = Map::new();
    let mut parent_data = Map::new();

    for (position, node_index) in page
        .layouts
        .iter()
        .copied()
        .chain(std::iter::once(Some(page.leaf)))
        .enumerate()
    {
        let Some(node_index) = node_index else {
            branch.push(None);
            continue;
        };

        let result = if position == page.layouts.len() {
            if let Some((status, error)) = action_error.as_ref() {
                PageLoadResult::Error {
                    status: *status,
                    error: error.clone(),
                }
            } else {
                load(node_index, &parent_server_data, &parent_data)?
            }
        } else {
            load(node_index, &parent_server_data, &parent_data)?
        };

        match result {
            PageLoadResult::Loaded { server_data, data } => {
                validate_page_load_payload(route.id.as_str(), node_index, server_data.as_ref())?;
                validate_page_load_payload(route.id.as_str(), node_index, data.as_ref())?;
                merge_parent_object(&mut parent_server_data, server_data.as_ref());
                merge_parent_object(&mut parent_data, data.as_ref());
                branch.push(Some(PageLoadedNode {
                    node_index,
                    server_data,
                    data,
                }));
            }
            PageLoadResult::Redirect { status, location } => {
                return Ok(PageExecutionResult::Redirect(redirect_response(
                    status, &location,
                )));
            }
            PageLoadResult::Error { status, error } => {
                let mut boundary_position = position;
                while boundary_position > 0 {
                    boundary_position -= 1;
                    if let Some(error_node_index) = page.errors[boundary_position] {
                        let boundary_branch = compact_loaded_branch(&branch[..=boundary_position]);
                        let layout_indexes = boundary_branch
                            .iter()
                            .map(|node| manifest.nodes.get(node.node_index))
                            .collect::<Vec<_>>();
                        let nodes = RuntimePageNodes {
                            nodes: layout_indexes,
                        };

                        return Ok(PageExecutionResult::ErrorBoundary(PageErrorBoundary {
                            status,
                            error,
                            branch: boundary_branch,
                            error_node_index,
                            ssr: nodes.ssr(),
                            csr: nodes.csr(),
                            action: None,
                            effects: Default::default(),
                        }));
                    }
                }

                return Ok(PageExecutionResult::Fatal { status, error });
            }
        }
    }

    Ok(PageExecutionResult::Rendered {
        branch: compact_loaded_branch(&branch),
    })
}

fn execute_page_load_internal_with_state<F>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    action_error: Option<(u16, Value)>,
    state: &mut RuntimeRenderState,
    mut load: F,
) -> Result<PageExecutionResult>
where
    F: FnMut(
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
{
    let Some(page) = route.page.as_ref() else {
        return Err(RuntimePageError::NonPageLoadRoute.into());
    };

    let mut branch = Vec::with_capacity(page.layouts.len() + 1);
    let mut parent_server_data = Map::new();
    let mut parent_data = Map::new();

    for (position, node_index) in page
        .layouts
        .iter()
        .copied()
        .chain(std::iter::once(Some(page.leaf)))
        .enumerate()
    {
        let Some(node_index) = node_index else {
            branch.push(None);
            continue;
        };

        let result = if position == page.layouts.len() {
            if let Some((status, error)) = action_error.as_ref() {
                PageLoadResult::Error {
                    status: *status,
                    error: error.clone(),
                }
            } else {
                load(state, node_index, &parent_server_data, &parent_data)?
            }
        } else {
            load(state, node_index, &parent_server_data, &parent_data)?
        };

        match result {
            PageLoadResult::Loaded { server_data, data } => {
                validate_page_load_payload(route.id.as_str(), node_index, server_data.as_ref())?;
                validate_page_load_payload(route.id.as_str(), node_index, data.as_ref())?;
                merge_parent_object(&mut parent_server_data, server_data.as_ref());
                merge_parent_object(&mut parent_data, data.as_ref());
                branch.push(Some(PageLoadedNode {
                    node_index,
                    server_data,
                    data,
                }));
            }
            PageLoadResult::Redirect { status, location } => {
                return Ok(PageExecutionResult::Redirect(redirect_response(
                    status, &location,
                )));
            }
            PageLoadResult::Error { status, error } => {
                let mut boundary_position = position;
                while boundary_position > 0 {
                    boundary_position -= 1;
                    if let Some(error_node_index) = page.errors[boundary_position] {
                        let boundary_branch = compact_loaded_branch(&branch[..=boundary_position]);
                        let layout_indexes = boundary_branch
                            .iter()
                            .map(|node| manifest.nodes.get(node.node_index))
                            .collect::<Vec<_>>();
                        let nodes = RuntimePageNodes {
                            nodes: layout_indexes,
                        };

                        return Ok(PageExecutionResult::ErrorBoundary(PageErrorBoundary {
                            status,
                            error,
                            branch: boundary_branch,
                            error_node_index,
                            ssr: nodes.ssr(),
                            csr: nodes.csr(),
                            action: None,
                            effects: Default::default(),
                        }));
                    }
                }

                return Ok(PageExecutionResult::Fatal { status, error });
            }
        }
    }

    Ok(PageExecutionResult::Rendered {
        branch: compact_loaded_branch(&branch),
    })
}

pub fn execute_page_load<F>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    load: F,
) -> Result<PageExecutionResult>
where
    F: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    execute_page_load_internal(manifest, route, None, load)
}

pub fn execute_page_request<F>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    request_pathname: &str,
    has_actions: bool,
    state: &mut RuntimeRenderState,
    status: u16,
    load: F,
) -> Result<PageRequestResult>
where
    F: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    execute_page_request_with_action(
        manifest,
        route,
        request_pathname,
        has_actions,
        state,
        None,
        status,
        load,
    )
}

pub fn execute_page_request_with_action<F>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    request_pathname: &str,
    has_actions: bool,
    state: &mut RuntimeRenderState,
    action: Option<PageActionExecution>,
    status: u16,
    load: F,
) -> Result<PageRequestResult>
where
    F: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    let Some(plan) = prepare_page_render_plan(manifest, route, request_pathname) else {
        return Err(RuntimePageError::NonPageRequestRoute.into());
    };

    if let Some(response) = action
        .as_ref()
        .and_then(PageActionExecution::redirect_response)
    {
        return Ok(PageRequestResult::Redirect(response));
    }

    let status = action
        .as_ref()
        .map(PageActionExecution::status)
        .unwrap_or(status);

    match resolve_page_runtime_decision(Some(route.id.as_str()), plan, has_actions, state, status)?
    {
        PageRuntimeDecision::Shell(mut shell) => {
            shell.action = action;
            Ok(PageRequestResult::Shell(shell))
        }
        PageRuntimeDecision::Early(response) => Ok(PageRequestResult::Early(response)),
        PageRuntimeDecision::Render(plan) => match execute_page_load_internal(
            manifest,
            route,
            action.as_ref().and_then(|action| match &action.result {
                ActionRequestResult::Error { error } => Some((action.status(), error.clone())),
                _ => None,
            }),
            load,
        )? {
            PageExecutionResult::Rendered { branch } => {
                Ok(PageRequestResult::Rendered(RenderedPage {
                    plan,
                    branch,
                    action,
                    effects: Default::default(),
                }))
            }
            PageExecutionResult::Redirect(response) => Ok(PageRequestResult::Redirect(response)),
            PageExecutionResult::ErrorBoundary(mut boundary) => {
                boundary.action = action;
                Ok(PageRequestResult::ErrorBoundary(boundary))
            }
            PageExecutionResult::Fatal { status, error } => Ok(PageRequestResult::Fatal {
                status,
                error,
                effects: Default::default(),
            }),
        },
    }
}

pub fn execute_page_request_from_request<FL, FR, FLoad>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    request: &ServerRequest,
    has_actions: bool,
    dev: bool,
    state: &mut RuntimeRenderState,
    status: u16,
    execute_local_action: FL,
    execute_remote_action: FR,
    load: FLoad,
) -> Result<PageRequestResult>
where
    FL: FnOnce() -> Result<ActionRequestResult>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
    FLoad: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    let action = resolve_page_action_request(
        request,
        Some(route.id.as_str()),
        dev,
        has_actions,
        execute_local_action,
        execute_remote_action,
    )?;

    execute_page_request_with_action(
        manifest,
        route,
        request.url.path(),
        has_actions,
        state,
        action,
        status,
        load,
    )
}

pub fn execute_named_page_request_from_request<FL, FR, FLoad>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    request: &ServerRequest,
    has_default_action: bool,
    named_action_count: usize,
    state: &mut RuntimeRenderState,
    status: u16,
    execute_local_action: FL,
    execute_remote_action: FR,
    load: FLoad,
) -> Result<PageRequestResult>
where
    FL: FnOnce(&str) -> Result<Option<ActionRequestResult>>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
    FLoad: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    let action = if get_remote_action(&request.url).is_some() {
        resolve_page_action_request_result(
            request,
            Some(route.id.as_str()),
            false,
            has_default_action || named_action_count > 0,
            || -> Result<ActionRequestResult> {
                unreachable!("remote action path bypasses local named action closure")
            },
            execute_remote_action,
        )?
    } else {
        resolve_named_page_action_request_result(
            request,
            has_default_action,
            named_action_count,
            execute_local_action,
        )?
    };

    execute_page_request_with_action(
        manifest,
        route,
        request.url.path(),
        has_default_action || named_action_count > 0,
        state,
        action,
        status,
        load,
    )
}

pub fn execute_runtime_page_request<FL, FR, FLoad>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    event: &RuntimeEvent,
    has_actions: bool,
    dev: bool,
    state: &mut RuntimeRenderState,
    status: u16,
    execute_local_action: FL,
    execute_remote_action: FR,
    load: FLoad,
) -> Result<PageRequestResult>
where
    FL: FnOnce() -> Result<ActionRequestResult>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
    FLoad: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    execute_page_request_from_request(
        manifest,
        route,
        &event.request,
        has_actions,
        dev,
        state,
        status,
        execute_local_action,
        execute_remote_action,
        load,
    )
}

pub fn execute_named_runtime_page_request<FL, FR, FLoad>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    event: &RuntimeEvent,
    has_default_action: bool,
    named_action_count: usize,
    state: &mut RuntimeRenderState,
    status: u16,
    execute_local_action: FL,
    execute_remote_action: FR,
    load: FLoad,
) -> Result<PageRequestResult>
where
    FL: FnOnce(&str) -> Result<Option<ActionRequestResult>>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
    FLoad: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    execute_named_page_request_from_request(
        manifest,
        route,
        &event.request,
        has_default_action,
        named_action_count,
        state,
        status,
        execute_local_action,
        execute_remote_action,
        load,
    )
}

pub fn execute_page_request_with_action_and_stateful_load<F>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    request_pathname: &str,
    has_actions: bool,
    state: &mut RuntimeRenderState,
    action: Option<PageActionExecution>,
    status: u16,
    load: F,
) -> Result<PageRequestResult>
where
    F: FnMut(
        &mut RuntimeRenderState,
        usize,
        &Map<String, Value>,
        &Map<String, Value>,
    ) -> Result<PageLoadResult>,
{
    let Some(plan) = prepare_page_render_plan(manifest, route, request_pathname) else {
        return Err(RuntimePageError::NonPageRequestRoute.into());
    };

    if let Some(response) = action
        .as_ref()
        .and_then(PageActionExecution::redirect_response)
    {
        return Ok(PageRequestResult::Redirect(response));
    }

    let status = action
        .as_ref()
        .map(PageActionExecution::status)
        .unwrap_or(status);

    match resolve_page_runtime_decision(Some(route.id.as_str()), plan, has_actions, state, status)?
    {
        PageRuntimeDecision::Shell(mut shell) => {
            shell.action = action;
            Ok(PageRequestResult::Shell(shell))
        }
        PageRuntimeDecision::Early(response) => Ok(PageRequestResult::Early(response)),
        PageRuntimeDecision::Render(plan) => match execute_page_load_internal_with_state(
            manifest,
            route,
            action.as_ref().and_then(|action| match &action.result {
                ActionRequestResult::Error { error } => Some((action.status(), error.clone())),
                _ => None,
            }),
            state,
            load,
        )? {
            PageExecutionResult::Rendered { branch } => {
                Ok(PageRequestResult::Rendered(RenderedPage {
                    plan,
                    branch,
                    action,
                    effects: Default::default(),
                }))
            }
            PageExecutionResult::Redirect(response) => Ok(PageRequestResult::Redirect(response)),
            PageExecutionResult::ErrorBoundary(mut boundary) => {
                boundary.action = action;
                Ok(PageRequestResult::ErrorBoundary(boundary))
            }
            PageExecutionResult::Fatal { status, error } => Ok(PageRequestResult::Fatal {
                status,
                error,
                effects: Default::default(),
            }),
        },
    }
}

pub fn execute_runtime_page_stage<FJ, FL, FR, FLoad>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    event: &RuntimeEvent,
    has_actions: bool,
    dev: bool,
    state: &mut RuntimeRenderState,
    status: u16,
    execute_action_json: FJ,
    execute_local_action: FL,
    execute_remote_action: FR,
    load: FLoad,
) -> Result<super::RuntimeExecutionResult>
where
    FJ: FnOnce() -> ActionJsonResult,
    FL: FnOnce() -> Result<ActionRequestResult>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
    FLoad: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    if state.depth > MAX_REQUEST_DEPTH {
        return Ok(super::RuntimeExecutionResult::Response(
            super::plain_text_response(404, format!("Not found: {}", event.request.url.path())),
        ));
    }

    if let Some(response) = execute_page_action_json_request(
        &event.request,
        event.app_state.as_ref(),
        Some(route.id.as_str()),
        dev,
        has_actions,
        execute_action_json,
    ) {
        return Ok(super::RuntimeExecutionResult::Response(response));
    }

    execute_runtime_page_request(
        manifest,
        route,
        event,
        has_actions,
        dev,
        state,
        status,
        execute_local_action,
        execute_remote_action,
        load,
    )
    .map(super::RuntimeExecutionResult::Page)
}

pub fn execute_named_runtime_page_stage<FJ, FL, FR, FLoad>(
    manifest: &KitManifest,
    route: &ManifestRoute,
    event: &RuntimeEvent,
    has_default_action: bool,
    named_action_count: usize,
    state: &mut RuntimeRenderState,
    status: u16,
    execute_action_json: FJ,
    execute_local_action: FL,
    execute_remote_action: FR,
    load: FLoad,
) -> Result<super::RuntimeExecutionResult>
where
    FJ: FnOnce(&str) -> Result<Option<ActionJsonResult>>,
    FL: FnOnce(&str) -> Result<Option<ActionRequestResult>>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
    FLoad: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    if state.depth > MAX_REQUEST_DEPTH {
        return Ok(super::RuntimeExecutionResult::Response(
            super::plain_text_response(404, format!("Not found: {}", event.request.url.path())),
        ));
    }

    if let Some(response) = execute_named_page_action_json_request_result(
        &event.request,
        event.app_state.as_ref(),
        Some(route.id.as_str()),
        false,
        has_default_action,
        named_action_count,
        execute_action_json,
    )? {
        return Ok(super::RuntimeExecutionResult::Response(response));
    }

    execute_named_runtime_page_request(
        manifest,
        route,
        event,
        has_default_action,
        named_action_count,
        state,
        status,
        execute_local_action,
        execute_remote_action,
        load,
    )
    .map(super::RuntimeExecutionResult::Page)
}

pub fn execute_page_action_json_request<F>(
    request: &ServerRequest,
    app_state: &super::AppState,
    route_id: Option<&str>,
    dev: bool,
    has_actions: bool,
    execute: F,
) -> Option<ServerResponse>
where
    F: FnOnce() -> ActionJsonResult,
{
    if !is_action_json_request(request) {
        return None;
    }

    if !has_actions {
        return Some(no_actions_action_json_response(route_id, dev));
    }

    let result = execute();
    Some(match validate_action_result_shape(route_id, &result) {
        Ok(()) => action_json_response(&result, app_state).ok()?,
        Err(error) => action_json_response(
            &ActionJsonResult::Error {
                status: 500,
                error: serde_json::json!({ "message": error.to_string() }),
            },
            app_state,
        )
        .ok()?,
    })
}

pub fn execute_named_page_action_json_request<F>(
    request: &ServerRequest,
    app_state: &super::AppState,
    route_id: Option<&str>,
    dev: bool,
    has_default_action: bool,
    named_action_count: usize,
    execute_named: F,
) -> Result<Option<ServerResponse>>
where
    F: FnOnce(&str) -> Result<Option<ActionJsonResult>>,
{
    execute_named_page_action_json_request_result(
        request,
        app_state,
        route_id,
        dev,
        has_default_action,
        named_action_count,
        execute_named,
    )
}

pub(crate) fn execute_named_page_action_json_request_result<F>(
    request: &ServerRequest,
    app_state: &super::AppState,
    route_id: Option<&str>,
    dev: bool,
    has_default_action: bool,
    named_action_count: usize,
    execute_named: F,
) -> Result<Option<ServerResponse>>
where
    F: FnOnce(&str) -> Result<Option<ActionJsonResult>>,
{
    if !is_action_json_request(request) {
        return Ok(None);
    }

    if !has_default_action && named_action_count == 0 {
        return Ok(Some(no_actions_action_json_response(route_id, dev)));
    }

    if has_default_action && named_action_count > 0 {
        return Ok(Some(action_json_response(
            &ActionJsonResult::Error {
                status: 500,
                error: serde_json::json!({
                    "message": "When using named actions, the default action cannot be used. See the docs for more info: https://svelte.dev/docs/kit/form-actions#named-actions"
                }),
            },
            app_state,
        )?));
    }

    let action_name = match selected_action_name(&request.url) {
        Ok(name) => name,
        Err(error) => {
            return Ok(Some(action_json_response(
                &ActionJsonResult::Error { status: 500, error },
                app_state,
            )?));
        }
    };

    if !is_form_content_type(request.header("content-type")) {
        let content_type = request.header("content-type").unwrap_or("null");
        return Ok(Some(action_json_response(
            &ActionJsonResult::Error {
                status: 415,
                error: serde_json::json!({
                    "status": 415,
                    "message": format!("Form actions expect form-encoded data — received {content_type}")
                }),
            },
            app_state,
        )?));
    }

    let Some(result) = execute_named(&action_name)? else {
        return Ok(Some(action_json_response(
            &ActionJsonResult::Error {
                status: 404,
                error: serde_json::json!({
                    "status": 404,
                    "message": format!("No action with name '{action_name}' found")
                }),
            },
            app_state,
        )?));
    };

    Ok(Some(
        match validate_action_result_shape(route_id, &result) {
            Ok(()) => action_json_response(&result, app_state)?,
            Err(error) => action_json_response(
                &ActionJsonResult::Error {
                    status: 500,
                    error: serde_json::json!({ "message": error.to_string() }),
                },
                app_state,
            )?,
        },
    ))
}

pub fn resolve_page_action_request<FL, FR>(
    request: &ServerRequest,
    route_id: Option<&str>,
    dev: bool,
    has_actions: bool,
    execute_local: FL,
    execute_remote: FR,
) -> Result<Option<PageActionExecution>>
where
    FL: FnOnce() -> Result<ActionRequestResult>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
{
    resolve_page_action_request_result(
        request,
        route_id,
        dev,
        has_actions,
        execute_local,
        execute_remote,
    )
}

pub(crate) fn resolve_page_action_request_result<FL, FR>(
    request: &ServerRequest,
    route_id: Option<&str>,
    dev: bool,
    has_actions: bool,
    execute_local: FL,
    execute_remote: FR,
) -> Result<Option<PageActionExecution>>
where
    FL: FnOnce() -> Result<ActionRequestResult>,
    FR: FnOnce(&str) -> Result<super::RemoteFormExecutionResult>,
{
    if !is_action_request(request) {
        return Ok(None);
    }

    if get_remote_action(&request.url).is_some() {
        let remote = handle_remote_form_action_request_result(
            request,
            route_id,
            dev,
            has_actions,
            execute_remote,
        )?;
        return Ok(remote.map(|remote| PageActionExecution {
            headers: remote.headers,
            result: remote.result,
        }));
    }

    if !has_actions {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("allow"),
            HeaderValue::from_static("GET"),
        );
        return Ok(Some(PageActionExecution {
            headers,
            result: no_actions_action_request_result(route_id, dev),
        }));
    }

    let result = execute_local()?;
    Ok(Some(PageActionExecution {
        headers: HeaderMap::new(),
        result: validate_action_request_result_shape(route_id, result),
    }))
}

pub fn resolve_named_page_action_request<F>(
    request: &ServerRequest,
    has_default_action: bool,
    named_action_count: usize,
    execute_named: F,
) -> Result<Option<PageActionExecution>>
where
    F: FnOnce(&str) -> Result<Option<ActionRequestResult>>,
{
    resolve_named_page_action_request_result(
        request,
        has_default_action,
        named_action_count,
        execute_named,
    )
}

pub(crate) fn resolve_named_page_action_request_result<F>(
    request: &ServerRequest,
    has_default_action: bool,
    named_action_count: usize,
    execute_named: F,
) -> Result<Option<PageActionExecution>>
where
    F: FnOnce(&str) -> Result<Option<ActionRequestResult>>,
{
    if !is_action_request(request) {
        return Ok(None);
    }

    if has_default_action && named_action_count > 0 {
        return Ok(Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Error {
                error: serde_json::json!({
                    "message": "When using named actions, the default action cannot be used. See the docs for more info: https://svelte.dev/docs/kit/form-actions#named-actions"
                }),
            },
        }));
    }

    let action_name = match selected_action_name(&request.url) {
        Ok(name) => name,
        Err(error) => {
            return Ok(Some(PageActionExecution {
                headers: HeaderMap::new(),
                result: ActionRequestResult::Error { error },
            }));
        }
    };

    if !is_form_content_type(request.header("content-type")) {
        let content_type = request.header("content-type").unwrap_or("null");
        return Ok(Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Error {
                error: serde_json::json!({
                    "status": 415,
                    "message": format!("Form actions expect form-encoded data — received {content_type}")
                }),
            },
        }));
    }

    let Some(result) = execute_named(&action_name)? else {
        return Ok(Some(PageActionExecution {
            headers: HeaderMap::new(),
            result: ActionRequestResult::Error {
                error: serde_json::json!({
                    "status": 404,
                    "message": format!("No action with name '{action_name}' found")
                }),
            },
        }));
    };

    Ok(Some(PageActionExecution {
        headers: HeaderMap::new(),
        result: validate_action_request_result_shape(None, result),
    }))
}

pub fn execute_error_page_load<F>(
    manifest: &KitManifest,
    status: u16,
    error: Value,
    mut load: F,
) -> Result<ExecutedErrorPage>
where
    F: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
{
    let Some(plan) = prepare_error_page_render(manifest, status, error) else {
        return Err(RuntimePageError::MissingErrorPageFallbackNodes.into());
    };

    let mut branch = Vec::with_capacity(plan.branch_node_indexes.len());
    let mut parent_server_data = Map::new();
    let mut parent_data = Map::new();
    let last = plan.branch_node_indexes.len().saturating_sub(1);

    for (position, node_index) in plan.branch_node_indexes.iter().copied().enumerate() {
        if position == last {
            branch.push(PageLoadedNode {
                node_index,
                server_data: None,
                data: None,
            });
            continue;
        }

        match load(node_index, &parent_server_data, &parent_data)? {
            PageLoadResult::Loaded { server_data, data } => {
                merge_parent_object(&mut parent_server_data, server_data.as_ref());
                merge_parent_object(&mut parent_data, data.as_ref());
                branch.push(PageLoadedNode {
                    node_index,
                    server_data,
                    data,
                });
            }
            PageLoadResult::Redirect { status, location } => {
                return Err(RuntimePageError::ErrorPageLoadRedirect { status, location }.into());
            }
            PageLoadResult::Error { status, error } => {
                return Err(RuntimePageError::ErrorPageLoadFailed {
                    status,
                    error: error.to_string(),
                }
                .into());
            }
        }
    }

    Ok(ExecutedErrorPage { plan, branch })
}

pub fn execute_error_page_request<F, R>(
    manifest: &KitManifest,
    status: u16,
    error: Value,
    request_already_handling_error: bool,
    dev: bool,
    render_error: R,
    mut load: F,
) -> Result<ErrorPageRequestResult>
where
    F: FnMut(usize, &Map<String, Value>, &Map<String, Value>) -> Result<PageLoadResult>,
    R: FnOnce(u16, &str) -> String + Clone,
{
    if request_already_handling_error {
        return Ok(ErrorPageRequestResult::Static(static_error_page(
            status,
            &runtime_error_message(&error),
            render_error,
            dev,
        )));
    }

    let Some(plan) = prepare_error_page_render(manifest, status, error.clone()) else {
        return Err(RuntimePageError::MissingErrorPageFallbackNodes.into());
    };

    let mut branch = Vec::with_capacity(plan.branch_node_indexes.len());
    let mut parent_server_data = Map::new();
    let mut parent_data = Map::new();
    let last = plan.branch_node_indexes.len().saturating_sub(1);

    for (position, node_index) in plan.branch_node_indexes.iter().copied().enumerate() {
        if position == last {
            branch.push(PageLoadedNode {
                node_index,
                server_data: None,
                data: None,
            });
            continue;
        }

        match load(node_index, &parent_server_data, &parent_data)? {
            PageLoadResult::Loaded { server_data, data } => {
                merge_parent_object(&mut parent_server_data, server_data.as_ref());
                merge_parent_object(&mut parent_data, data.as_ref());
                branch.push(PageLoadedNode {
                    node_index,
                    server_data,
                    data,
                });
            }
            PageLoadResult::Redirect { status, location } => {
                return Ok(ErrorPageRequestResult::Redirect(redirect_response(
                    status, &location,
                )));
            }
            PageLoadResult::Error { status, error } => {
                return Ok(ErrorPageRequestResult::Static(static_error_page(
                    status,
                    &runtime_error_message(&error),
                    render_error,
                    dev,
                )));
            }
        }
    }

    Ok(ErrorPageRequestResult::Rendered(ExecutedErrorPage {
        plan,
        branch,
    }))
}

fn merge_parent_object(into: &mut Map<String, Value>, value: Option<&Value>) {
    let Some(Value::Object(entries)) = value else {
        return;
    };

    for (key, value) in entries {
        into.insert(key.clone(), value.clone());
    }
}

fn compact_loaded_branch(branch: &[Option<PageLoadedNode>]) -> Vec<PageLoadedNode> {
    branch.iter().flatten().cloned().collect()
}

fn validate_page_load_payload(
    route_id: &str,
    node_index: usize,
    value: Option<&Value>,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };

    validate_load_response(
        value,
        Some(&format!("while rendering {route_id} (node {node_index})")),
    )
}

fn validate_action_result_shape(route_id: Option<&str>, result: &ActionJsonResult) -> Result<()> {
    match result {
        ActionJsonResult::Success {
            data: Some(data), ..
        } => validate_action_payload(route_id, data),
        ActionJsonResult::Failure { data, .. } => validate_action_payload(route_id, data),
        _ => Ok(()),
    }
}

fn validate_action_request_result_shape(
    route_id: Option<&str>,
    result: ActionRequestResult,
) -> ActionRequestResult {
    let invalid = match &result {
        ActionRequestResult::Success {
            data: Some(data), ..
        } => validate_action_payload(route_id, data).err(),
        ActionRequestResult::Failure { data, .. } => validate_action_payload(route_id, data).err(),
        _ => None,
    };

    if let Some(error) = invalid {
        ActionRequestResult::Error {
            error: serde_json::json!({ "message": error.to_string() }),
        }
    } else {
        result
    }
}

fn validate_action_payload(route_id: Option<&str>, value: &Value) -> Result<()> {
    if value.is_null() || value.is_object() {
        return Ok(());
    }

    Err(RuntimePageError::InvalidActionPayload {
        route_id: route_id.unwrap_or("this page").to_string(),
    }
    .into())
}

fn runtime_error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Internal Error")
        .to_string()
}

fn selected_action_name(url: &url::Url) -> std::result::Result<String, Value> {
    for (name, _) in url.query_pairs() {
        if let Some(name) = name.strip_prefix('/') {
            if name == "default" {
                return Err(serde_json::json!({
                    "message": "Cannot use reserved action name \"default\""
                }));
            }

            return Ok(name.to_string());
        }
    }

    Ok("default".to_string())
}
