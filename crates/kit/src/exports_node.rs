use std::{fs::File, path::Path};

use http::Request as HttpRequest;

use crate::{ExportsNodeError, Result, ServerRequest, ServerResponse};

#[derive(Debug, Clone)]
pub struct NodeRequest {
    pub request: ServerRequest,
    pub body: Option<Vec<u8>>,
}

pub fn get_node_request(
    request: HttpRequest<Option<Vec<u8>>>,
    body_size_limit: Option<usize>,
) -> Result<NodeRequest> {
    let (parts, body) = request.into_parts();
    let request = ServerRequest::try_from(HttpRequest::from_parts(parts, ()))?;
    let body = match request.method.as_str() {
        "GET" | "HEAD" => None,
        _ => body.filter(|body| !body.is_empty()),
    };

    if let (Some(limit), Some(body)) = (body_size_limit, body.as_ref()) {
        if body.len() > limit {
            return Err(ExportsNodeError::BodyLimitExceeded {
                length: body.len(),
                limit,
            }
            .into());
        }
    }

    Ok(NodeRequest { request, body })
}

pub fn set_node_response(response: &ServerResponse) -> Result<http::Response<Option<String>>> {
    response.to_http_response()
}

pub fn create_readable_stream(path: &Path) -> Result<File> {
    File::open(path).map_err(Into::into)
}
