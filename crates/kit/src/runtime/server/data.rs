use serde_json::{Map, Value};

use crate::Result;
use crate::manifest::ManifestRoute;
use crate::runtime::shared::validate_load_response;
use crate::url::normalize_path;

use super::{
    AppState, DataRequestNode, PreparedDataRequest, ResolvedRuntimeRequest, RuntimeRouteBehavior,
    ServerDataNode, ServerResponse, encode_transport_value,
};

pub fn route_data_node_indexes(route: &ManifestRoute) -> Option<Vec<Option<usize>>> {
    route.page.as_ref().map(|page| {
        page.layouts
            .iter()
            .copied()
            .chain(std::iter::once(Some(page.leaf)))
            .collect()
    })
}

pub fn invalidated_data_node_flags(
    node_count: usize,
    invalidated_data_nodes: Option<&[bool]>,
) -> Vec<bool> {
    match invalidated_data_nodes {
        None => vec![true; node_count],
        Some(flags) => (0..node_count)
            .map(|index| flags.get(index).copied().unwrap_or(false))
            .collect(),
    }
}

pub fn data_json_response(body: impl Into<Value>, status: u16) -> ServerResponse {
    let body = match body.into() {
        Value::String(string) => string,
        value => serde_json::to_string(&value).expect("serialize data json response"),
    };

    ServerResponse::builder(status)
        .header("content-type", "application/json")
        .expect("data json content-type is valid")
        .header("cache-control", "private, no-store")
        .expect("data json cache-control is valid")
        .body(body)
        .build()
        .expect("data json response is valid")
}

pub fn data_request_not_found_response() -> ServerResponse {
    ServerResponse::new(404)
}

pub fn redirect_data_response(location: &str) -> ServerResponse {
    data_json_response(
        serde_json::json!({
            "type": "redirect",
            "location": location,
        }),
        200,
    )
}

pub fn prepare_data_request(
    resolved: &ResolvedRuntimeRequest<'_>,
    behavior: &RuntimeRouteBehavior,
) -> Option<PreparedDataRequest> {
    let node_indexes = route_data_node_indexes(resolved.route)?;
    let invalidated = invalidated_data_node_flags(
        node_indexes.len(),
        resolved.prepared.invalidated_data_nodes.as_deref(),
    );

    Some(PreparedDataRequest {
        normalized_pathname: normalize_path(
            &resolved.prepared.url.path().to_string(),
            &behavior.trailing_slash,
        ),
        node_indexes,
        invalidated,
    })
}

pub fn render_data_request<F>(
    prepared: &PreparedDataRequest,
    app_state: &AppState,
    mut load: F,
) -> Result<ServerResponse>
where
    F: FnMut(usize, &Map<String, Value>) -> Result<DataRequestNode>,
{
    let mut parent_data = Map::new();
    let mut aborted = false;
    let mut nodes = Vec::with_capacity(prepared.node_indexes.len());

    for (i, node_index) in prepared.node_indexes.iter().enumerate() {
        if !prepared.invalidated.get(i).copied().unwrap_or(false) || aborted {
            nodes.push(serialize_data_request_node(
                &DataRequestNode::Skip,
                app_state,
            )?);
            continue;
        }

        let Some(node_index) = node_index else {
            nodes.push(Value::Null);
            continue;
        };

        let node = load(*node_index, &parent_data)?;
        match node {
            DataRequestNode::Data { data, uses, slash } => {
                validate_load_response(
                    &data,
                    Some(&format!("while rendering data request node {node_index}")),
                )?;
                if let Value::Object(entries) = &data {
                    for (key, value) in entries {
                        parent_data.insert(key.clone(), value.clone());
                    }
                }
                nodes.push(serialize_data_request_node(
                    &DataRequestNode::Data { data, uses, slash },
                    app_state,
                )?);
            }
            DataRequestNode::Redirect { location } => return Ok(redirect_data_response(&location)),
            DataRequestNode::Error { status, error } => {
                aborted = true;
                nodes.push(serialize_data_request_node(
                    &DataRequestNode::Error { status, error },
                    app_state,
                )?);
            }
            DataRequestNode::Skip => nodes.push(serialize_data_request_node(
                &DataRequestNode::Skip,
                app_state,
            )?),
        }
    }

    Ok(data_json_response(
        serde_json::json!({
            "type": "data",
            "nodes": nodes,
        }),
        200,
    ))
}

pub fn execute_data_request<F>(
    resolved: &ResolvedRuntimeRequest<'_>,
    behavior: &RuntimeRouteBehavior,
    app_state: &AppState,
    load: F,
) -> Result<Option<ServerResponse>>
where
    F: FnMut(usize, &Map<String, Value>) -> Result<DataRequestNode>,
{
    let Some(prepared) = prepare_data_request(resolved, behavior) else {
        return Ok(None);
    };

    render_data_request(&prepared, app_state, load).map(Some)
}

fn serialize_data_request_node(node: &DataRequestNode, app_state: &AppState) -> Result<Value> {
    match node {
        DataRequestNode::Data { data, uses, slash } => {
            let mut object = Map::new();
            object.insert("type".to_string(), Value::String("data".to_string()));
            object.insert("data".to_string(), encode_transport_value(app_state, data)?);
            object.insert(
                "uses".to_string(),
                Value::Object(super::serialize_uses(&ServerDataNode {
                    uses: uses.clone(),
                })),
            );
            if let Some(slash) = slash {
                object.insert("slash".to_string(), Value::String(slash.clone()));
            }
            Ok(Value::Object(object))
        }
        DataRequestNode::Skip => Ok(serde_json::json!({ "type": "skip" })),
        DataRequestNode::Error { status, error } => {
            let mut object = Map::new();
            object.insert("type".to_string(), Value::String("error".to_string()));
            if let Some(status) = status {
                object.insert("status".to_string(), Value::from(*status));
            }
            object.insert(
                "error".to_string(),
                encode_transport_value(app_state, error)?,
            );
            Ok(Value::Object(object))
        }
        DataRequestNode::Redirect { location } => Ok(serde_json::json!({
            "type": "redirect",
            "location": location,
        })),
    }
}
