use ::http::{HeaderMap, HeaderName, HeaderValue, Method};
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value};
use url::Url;

use crate::Result;
use crate::http::is_form_content_type;

use super::{
    ActionRequestResult, AppState, ServerRequest, ServerResponse, check_incorrect_fail_use,
    is_action_request, no_actions_action_request_result, parse_remote_arg,
    stringify_transport_payload,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRemoteId {
    pub hash: String,
    pub name: String,
    pub argument: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCallKind {
    QueryBatch,
    Form,
    Command,
    Prerender,
    Single,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCallRequest {
    pub id: ParsedRemoteId,
    pub kind: RemoteCallKind,
    pub method: Method,
    pub content_type: Option<String>,
    pub payload: Option<String>,
    pub payloads: Vec<String>,
    pub refreshes: Vec<String>,
    pub form_data: Map<String, Value>,
    pub form_meta_refreshes: Vec<String>,
    pub prerendering: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedRemoteInvocation {
    QueryBatch { payloads: Vec<Value> },
    Form { data: Map<String, Value> },
    Command { payload: Option<Value> },
    Single { payload: Option<Value> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCallExecution {
    Result { result: Value, issues: bool },
    Redirect { status: u16, location: String },
    Error { status: u16, error: Value },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFunctionResponse {
    Result {
        result: String,
        refreshes: Option<String>,
    },
    Redirect {
        location: String,
        refreshes: Option<String>,
    },
    Error {
        error: Value,
        status: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFormExecutionResult {
    Success,
    Redirect {
        status: u16,
        location: String,
    },
    Error {
        error: String,
        is_action_failure: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFormPostResult {
    pub headers: HeaderMap,
    pub result: ActionRequestResult,
}

pub fn parse_remote_id(id: &str) -> ParsedRemoteId {
    let mut parts = id.splitn(3, '/');
    ParsedRemoteId {
        hash: parts.next().unwrap_or_default().to_string(),
        name: parts.next().unwrap_or_default().to_string(),
        argument: parts.next().map(str::to_string),
    }
}

pub fn get_remote_id(url: &Url, base: &str, app_dir: &str) -> Option<String> {
    let prefix = format!(
        "{}/{}/remote/",
        base.trim_end_matches('/'),
        app_dir.trim_start_matches('/')
    );
    url.path()
        .strip_prefix(&prefix)
        .map(|remote| remote.to_string())
}

pub fn get_remote_action(url: &Url) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == "/remote")
        .map(|(_, value)| value.into_owned())
}

pub fn resolve_remote_request_url(
    request: &ServerRequest,
    base: &str,
    remote_id: Option<&str>,
) -> Result<Url> {
    let mut url = request.url.clone();

    if remote_id.is_some() {
        url.set_path(request.header("x-sveltekit-pathname").unwrap_or(base));
        url.set_query(
            request
                .header("x-sveltekit-search")
                .map(|search| search.trim_start_matches('?')),
        );
    }

    Ok(url)
}

pub fn check_remote_request_origin(
    request: &ServerRequest,
    remote_id: Option<&str>,
) -> Option<ServerResponse> {
    if remote_id.is_none() || request.method_is(&Method::GET) {
        return None;
    }

    let same_origin = request.url.origin().ascii_serialization();
    if request.header("origin") == Some(same_origin.as_str()) {
        return None;
    }

    let mut response = ServerResponse::new(403);
    response.set_header("content-type", "application/json; charset=utf-8");
    response.body = Some(
        serde_json::to_string(&serde_json::json!({
            "message": "Cross-site remote requests are forbidden"
        }))
        .expect("serialize remote-origin json response"),
    );
    Some(response)
}

pub fn remote_json_response(payload: &RemoteFunctionResponse) -> ServerResponse {
    remote_json_response_with_status(payload, None)
}

pub fn remote_json_response_with_status(
    payload: &RemoteFunctionResponse,
    status_override: Option<u16>,
) -> ServerResponse {
    let status = status_override.unwrap_or_else(|| match payload {
        RemoteFunctionResponse::Error { status, .. } => *status,
        _ => 200,
    });

    let body = match payload {
        RemoteFunctionResponse::Result { result, refreshes } => {
            let mut object = Map::from_iter([
                ("type".to_string(), Value::String("result".to_string())),
                ("result".to_string(), Value::String(result.clone())),
            ]);
            if let Some(refreshes) = refreshes {
                object.insert("refreshes".to_string(), Value::String(refreshes.clone()));
            }
            Value::Object(object)
        }
        RemoteFunctionResponse::Redirect {
            location,
            refreshes,
        } => {
            let mut object = Map::from_iter([
                ("type".to_string(), Value::String("redirect".to_string())),
                ("location".to_string(), Value::String(location.clone())),
            ]);
            if let Some(refreshes) = refreshes {
                object.insert("refreshes".to_string(), Value::String(refreshes.clone()));
            }
            Value::Object(object)
        }
        RemoteFunctionResponse::Error { error, status } => Value::Object(Map::from_iter([
            ("type".to_string(), Value::String("error".to_string())),
            ("error".to_string(), error.clone()),
            ("status".to_string(), Value::from(*status)),
        ])),
    };

    ServerResponse::builder(status)
        .header("content-type", "application/json; charset=utf-8")
        .expect("remote json content-type is valid")
        .header("cache-control", "private, no-store")
        .expect("remote json cache-control is valid")
        .body(serde_json::to_string(&body).expect("serialize remote json response"))
        .build()
        .expect("remote json response is valid")
}

pub fn execute_remote_call(
    request: &RemoteCallRequest,
    app_state: &AppState,
    mut execute: impl FnMut(PreparedRemoteInvocation) -> Result<RemoteCallExecution>,
    mut resolve_refresh: impl FnMut(&str) -> Result<Option<Value>>,
) -> Result<ServerResponse> {
    let invocation = match request.kind {
        RemoteCallKind::QueryBatch => {
            if request.method != Method::POST {
                return Ok(remote_json_response_with_status(
                    &RemoteFunctionResponse::Error {
                        error: serde_json::json!({
                            "message": format!(
                                "`query.batch` functions must be invoked via POST request, not {}",
                                request.method
                            )
                        }),
                        status: 405,
                    },
                    Some(200),
                ));
            }

            PreparedRemoteInvocation::QueryBatch {
                payloads: request
                    .payloads
                    .iter()
                    .map(|payload| parse_remote_arg(app_state, payload))
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .map(|payload| payload.unwrap_or(Value::Null))
                    .collect(),
            }
        }
        RemoteCallKind::Form => {
            if request.method != Method::POST {
                return Ok(remote_json_response_with_status(
                    &RemoteFunctionResponse::Error {
                        error: serde_json::json!({
                            "message": format!(
                                "`form` functions must be invoked via POST request, not {}",
                                request.method
                            )
                        }),
                        status: 405,
                    },
                    Some(if request.prerendering { 405 } else { 200 }),
                ));
            }

            if !is_form_content_type(request.content_type.as_deref()) {
                let content_type = request.content_type.as_deref().unwrap_or_default();
                return Ok(remote_json_response_with_status(
                    &RemoteFunctionResponse::Error {
                        error: serde_json::json!({
                            "message": format!(
                                "`form` functions expect form-encoded data — received {content_type}"
                            )
                        }),
                        status: 415,
                    },
                    Some(if request.prerendering { 415 } else { 200 }),
                ));
            }

            let mut data = request.form_data.clone();
            if let Some(argument) = request.id.argument.as_deref()
                && !data.contains_key("id")
            {
                data.insert("id".to_string(), decode_remote_argument(argument)?);
            }

            PreparedRemoteInvocation::Form { data }
        }
        RemoteCallKind::Command => PreparedRemoteInvocation::Command {
            payload: request
                .payload
                .as_deref()
                .map(|payload| parse_remote_arg(app_state, payload))
                .transpose()?
                .flatten(),
        },
        RemoteCallKind::Prerender | RemoteCallKind::Single => PreparedRemoteInvocation::Single {
            payload: request
                .id
                .argument
                .clone()
                .or_else(|| request.payload.clone())
                .as_deref()
                .map(|payload| parse_remote_arg(app_state, payload))
                .transpose()?
                .flatten(),
        },
    };

    let response = match execute(invocation)? {
        RemoteCallExecution::Result { result, issues } => {
            let refreshes = match request.kind {
                RemoteCallKind::Form if !issues => match serialize_remote_refreshes(
                    &request.form_meta_refreshes,
                    app_state,
                    &mut resolve_refresh,
                )? {
                    Some(refreshes) => Some(refreshes),
                    None if request.form_meta_refreshes.is_empty() => None,
                    None => return Ok(bad_request_remote_response()),
                },
                RemoteCallKind::Command => {
                    match serialize_remote_refreshes(
                        &request.refreshes,
                        app_state,
                        &mut resolve_refresh,
                    )? {
                        Some(refreshes) => Some(refreshes),
                        None if request.refreshes.is_empty() => None,
                        None => return Ok(bad_request_remote_response()),
                    }
                }
                _ => None,
            };

            remote_json_response(&RemoteFunctionResponse::Result {
                result: stringify_transport_payload(app_state, &result)?,
                refreshes,
            })
        }
        RemoteCallExecution::Redirect {
            status: _,
            location,
        } => {
            let refreshes = if matches!(request.kind, RemoteCallKind::Form) {
                match serialize_remote_refreshes(
                    &request.form_meta_refreshes,
                    app_state,
                    &mut resolve_refresh,
                )? {
                    Some(refreshes) => Some(refreshes),
                    None if request.form_meta_refreshes.is_empty() => None,
                    None => return Ok(bad_request_remote_response()),
                }
            } else {
                None
            };
            remote_json_response(&RemoteFunctionResponse::Redirect {
                location,
                refreshes,
            })
        }
        RemoteCallExecution::Error { status, error } => remote_json_response_with_status(
            &RemoteFunctionResponse::Error { error, status },
            Some(if request.prerendering { status } else { 200 }),
        ),
    };

    Ok(response)
}

pub fn handle_remote_call(
    request: &RemoteCallRequest,
    app_state: &AppState,
    execute: impl FnMut(PreparedRemoteInvocation) -> Result<RemoteCallExecution>,
    resolve_refresh: impl FnMut(&str) -> Result<Option<Value>>,
) -> Result<ServerResponse> {
    execute_remote_call(request, app_state, execute, resolve_refresh)
}

pub fn handle_remote_form_post_result(
    route_id: Option<&str>,
    dev: bool,
    has_form: bool,
    execute: impl FnOnce() -> RemoteFormExecutionResult,
) -> RemoteFormPostResult {
    if !has_form {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("allow"),
            HeaderValue::from_static("GET"),
        );
        return RemoteFormPostResult {
            headers,
            result: no_actions_action_request_result(route_id, dev),
        };
    }

    let result = match execute() {
        RemoteFormExecutionResult::Success => ActionRequestResult::Success {
            status: 200,
            data: None,
        },
        RemoteFormExecutionResult::Redirect { status, location } => {
            ActionRequestResult::Redirect { status, location }
        }
        RemoteFormExecutionResult::Error {
            error,
            is_action_failure,
        } => ActionRequestResult::Error {
            error: Value::String(check_incorrect_fail_use(&error, is_action_failure)),
        },
    };

    RemoteFormPostResult {
        headers: HeaderMap::new(),
        result,
    }
}

pub fn handle_remote_form_post(
    route_id: Option<&str>,
    dev: bool,
    has_form: bool,
    execute: impl FnOnce() -> RemoteFormExecutionResult,
) -> RemoteFormPostResult {
    handle_remote_form_post_result(route_id, dev, has_form, execute)
}

pub fn handle_remote_form_action_request(
    request: &ServerRequest,
    route_id: Option<&str>,
    dev: bool,
    has_form: bool,
    execute: impl FnOnce(&str) -> Result<RemoteFormExecutionResult>,
) -> Result<Option<RemoteFormPostResult>> {
    handle_remote_form_action_request_result(request, route_id, dev, has_form, execute)
}

pub(crate) fn handle_remote_form_action_request_result(
    request: &ServerRequest,
    route_id: Option<&str>,
    dev: bool,
    has_form: bool,
    execute: impl FnOnce(&str) -> Result<RemoteFormExecutionResult>,
) -> Result<Option<RemoteFormPostResult>> {
    if !is_action_request(request) {
        return Ok(None);
    }

    if !has_form {
        return Ok(Some(handle_remote_form_post(
            route_id,
            dev,
            has_form,
            || unreachable!("missing form actions should not execute remote callback"),
        )));
    }

    let Some(remote_id) = get_remote_action(&request.url) else {
        return Ok(None);
    };
    let result = execute(&remote_id)?;
    Ok(Some(handle_remote_form_post(
        route_id,
        dev,
        has_form,
        || result,
    )))
}

fn serialize_remote_refreshes(
    keys: &[String],
    app_state: &AppState,
    resolve_refresh: &mut impl FnMut(&str) -> Result<Option<Value>>,
) -> Result<Option<String>> {
    let mut refreshes = Map::new();
    for key in keys {
        if let Some(value) = resolve_refresh(key)? {
            refreshes.insert(key.clone(), value);
        }
    }

    if refreshes.is_empty() {
        Ok(None)
    } else {
        stringify_transport_payload(app_state, &Value::Object(refreshes)).map(Some)
    }
}

fn bad_request_remote_response() -> ServerResponse {
    remote_json_response_with_status(
        &RemoteFunctionResponse::Error {
            error: serde_json::json!({
                "message": "Bad Request"
            }),
            status: 400,
        },
        Some(400),
    )
}

fn decode_remote_argument(argument: &str) -> Result<Value> {
    let decoded = percent_decode_str(argument).decode_utf8_lossy();
    serde_json::from_str(decoded.as_ref()).map_err(|error| {
        crate::RuntimeRemoteError::InvalidArgument {
            message: error.to_string(),
        }
        .into()
    })
}
