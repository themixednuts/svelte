use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use ::http::Method;

use crate::http::negotiate;
use crate::manifest::ManifestRoute;
use crate::{Error, Result, RuntimeEndpointError};

use super::{
    ALLOWED_PAGE_METHODS, ActionJsonResult, ActionRequestResult, AppState, ENDPOINT_METHODS,
    PAGE_METHODS, PrerenderDependency, RequestEventState, RuntimeRenderState, ServerRequest,
    ServerRequestEvent, ServerResponse, data_json_response, encode_transport_value,
    redirect_response, response_with_vary_accept,
};

#[derive(Clone)]
pub struct EndpointModule {
    pub handlers: HashMap<Method, EndpointHandler>,
    pub fallback: Option<EndpointHandler>,
    pub prerender: Option<bool>,
}

impl EndpointModule {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            fallback: None,
            prerender: None,
        }
    }

    pub fn with_handler(
        mut self,
        method: Method,
        handler: impl Fn(&ServerRequestEvent) -> EndpointResult + Send + Sync + 'static,
    ) -> Self {
        self.handlers.insert(method, Arc::new(handler));
        self
    }

    pub fn try_with_handler(
        self,
        method: &str,
        handler: impl Fn(&ServerRequestEvent) -> EndpointResult + Send + Sync + 'static,
    ) -> Result<Self> {
        let method = method.parse::<Method>().map_err(|error| {
            RuntimeEndpointError::InvalidHandlerMethod {
                method: method.to_string(),
                message: error.to_string(),
            }
        })?;
        Ok(self.with_handler(method, handler))
    }

    pub fn with_fallback(
        mut self,
        handler: impl Fn(&ServerRequestEvent) -> EndpointResult + Send + Sync + 'static,
    ) -> Self {
        self.fallback = Some(Arc::new(handler));
        self
    }
}

pub type EndpointHandler = Arc<dyn Fn(&ServerRequestEvent) -> EndpointResult + Send + Sync>;

#[derive(Debug)]
pub enum EndpointError {
    Error(Error),
    Redirect { status: u16, location: String },
}

pub type EndpointResult = std::result::Result<ServerResponse, EndpointError>;

impl From<Error> for EndpointError {
    fn from(value: Error) -> Self {
        Self::Error(value)
    }
}

pub fn finalize_route_response(
    request: &ServerRequest,
    route: &ManifestRoute,
    response: &ServerResponse,
) -> ServerResponse {
    if request.method_is(&Method::GET) && route.page.is_some() && route.endpoint.is_some() {
        response_with_vary_accept(response)
    } else {
        response.clone()
    }
}

pub fn is_action_json_request(request: &ServerRequest) -> bool {
    if !request.method_is(&Method::POST) {
        return false;
    }

    negotiate(
        request.header("accept").unwrap_or("*/*"),
        &["application/json", "text/html"],
    ) == Some("application/json")
}

pub fn is_action_request(request: &ServerRequest) -> bool {
    request.method_is(&Method::POST)
}

pub fn check_incorrect_fail_use(error: &str, is_action_failure: bool) -> String {
    if is_action_failure {
        "Cannot \"throw fail()\". Use \"return fail()\"".to_string()
    } else {
        error.to_string()
    }
}

pub fn action_json_response(
    result: &ActionJsonResult,
    app_state: &AppState,
) -> Result<ServerResponse> {
    match result {
        ActionJsonResult::Success { status, data } => Ok(data_json_response(
            serde_json::json!({
                "type": "success",
                "status": status,
                "data": data
                    .as_ref()
                    .map(|data| encode_transport_value(app_state, data))
                    .transpose()
                    ?,
            }),
            *status,
        )),
        ActionJsonResult::Failure { status, data } => Ok(data_json_response(
            serde_json::json!({
                "type": "failure",
                "status": status,
                "data": encode_transport_value(app_state, data)?,
            }),
            200,
        )),
        ActionJsonResult::Redirect { status, location } => Ok(data_json_response(
            serde_json::json!({
                "type": "redirect",
                "status": status,
                "location": location,
            }),
            200,
        )),
        ActionJsonResult::Error { status, error } => Ok(data_json_response(
            serde_json::json!({
                "type": "error",
                "error": encode_transport_value(app_state, error)?,
            }),
            *status,
        )),
    }
}

pub fn no_actions_action_json_response(route_id: Option<&str>, dev: bool) -> ServerResponse {
    let message = if dev {
        format!(
            "POST method not allowed. No form actions exist for the page at {}",
            route_id.unwrap_or("this page")
        )
    } else {
        "POST method not allowed. No form actions exist for this page".to_string()
    };

    let mut response = data_json_response(
        serde_json::json!({
            "type": "error",
            "error": { "message": message },
        }),
        405,
    );
    response.set_header("allow", "GET");
    response
}

pub fn no_actions_action_request_result(route_id: Option<&str>, dev: bool) -> ActionRequestResult {
    let message = if dev {
        format!(
            "POST method not allowed. No form actions exist for the page at {}",
            route_id.unwrap_or("this page")
        )
    } else {
        "POST method not allowed. No form actions exist for this page".to_string()
    };

    ActionRequestResult::Error {
        error: serde_json::json!({
            "status": 405,
            "message": message,
            "allow": "GET",
        }),
    }
}

pub fn page_method_response(method: &Method, has_actions: bool) -> Result<ServerResponse> {
    let mut allowed = ALLOWED_PAGE_METHODS
        .iter()
        .map(|method| (*method).to_string())
        .collect::<Vec<_>>();
    if has_actions {
        allowed.push("POST".to_string());
    }

    if *method == Method::OPTIONS {
        let mut response = ServerResponse::new(204);
        response.set_header("allow", allowed.join(", "));
        return Ok(response);
    }

    let mut response = ServerResponse::new(405);
    response.set_header("allow", allowed.join(", "));
    response.set_header("content-type", "text/plain; charset=utf-8");
    response.body = Some(format!("{} method not allowed", method.as_str()));
    Ok(response)
}

pub fn allowed_methods(methods: &BTreeSet<String>) -> Vec<String> {
    let mut allowed = ENDPOINT_METHODS
        .iter()
        .filter(|method| methods.contains(**method))
        .map(|method| (*method).to_string())
        .collect::<Vec<_>>();

    if methods.contains("GET") && !methods.contains("HEAD") {
        allowed.push("HEAD".to_string());
    }

    allowed
}

pub fn method_not_allowed_message(method: &Method) -> String {
    format!("{} method not allowed", method.as_str())
}

pub fn method_not_allowed_response(method: &Method, methods: &BTreeSet<String>) -> ServerResponse {
    let mut response = ServerResponse::new(405);
    response.body = Some(method_not_allowed_message(method));
    response.set_header("allow", allowed_methods(methods).join(", "));
    response.set_header("content-type", "text/plain; charset=utf-8");
    response
}

pub fn is_endpoint_request(
    method: &Method,
    accept: Option<&str>,
    x_sveltekit_action: Option<&str>,
) -> bool {
    let method = method.as_str();
    if ENDPOINT_METHODS.contains(&method) && !PAGE_METHODS.contains(&method) {
        return true;
    }

    if method == "POST" && x_sveltekit_action == Some("true") {
        return false;
    }

    let accept = accept.unwrap_or("*/*");
    negotiate(accept, &["*", "text/html"]) != Some("text/html")
}

pub fn render_endpoint(
    event: &ServerRequestEvent,
    event_state: &mut RequestEventState,
    module: &EndpointModule,
    state: &mut RuntimeRenderState,
) -> Result<ServerResponse> {
    let method = event.request.try_method()?;

    let mut handler = module
        .handlers
        .get(&method)
        .cloned()
        .or_else(|| module.fallback.clone());

    if method == Method::HEAD && !module.handlers.contains_key(&Method::HEAD) {
        handler = module
            .handlers
            .get(&Method::GET)
            .cloned()
            .or_else(|| module.fallback.clone());
    }

    let Some(handler) = handler else {
        return Ok(method_not_allowed_response(
            &method,
            &module
                .handlers
                .keys()
                .map(|method| method.as_str().to_string())
                .collect(),
        ));
    };

    let prerender = module.prerender.unwrap_or(state.prerender_default);
    if prerender
        && [Method::POST, Method::PATCH, Method::PUT, Method::DELETE]
            .iter()
            .any(|method| module.handlers.contains_key(method))
    {
        return Err(RuntimeEndpointError::PrerenderMutativeEndpoint.into());
    }

    if let Some(prerendering) = &state.prerendering {
        if !prerendering.inside_reroute && !prerender {
            if state.depth > 0 {
                let route_id = event
                    .route_id
                    .as_deref()
                    .unwrap_or(event.request.url.path());
                return Err(RuntimeEndpointError::NotPrerenderable {
                    route_id: route_id.to_string(),
                }
                .into());
            }

            return Ok(ServerResponse::new(204));
        }
    }

    event_state.allows_commands = true;
    let response = match handler(event) {
        Ok(response) => response,
        Err(EndpointError::Redirect { status, location }) => {
            return Ok(redirect_response(status, &location));
        }
        Err(EndpointError::Error(error)) => return Err(error),
    };

    if let Some(prerendering) = &mut state.prerendering {
        if !prerendering.inside_reroute || prerender {
            let mut cloned = response.clone();
            cloned.set_header("x-sveltekit-prerender", prerender.to_string());

            if prerendering.inside_reroute && prerender {
                if let Some(route_id) = &event.route_id {
                    cloned.set_header("x-sveltekit-routeid", route_id);
                }
                prerendering.dependencies.insert(
                    event.request.url.path().to_string(),
                    PrerenderDependency {
                        response: cloned.clone(),
                        body: None,
                    },
                );
            } else {
                return Ok(cloned);
            }
        }
    }

    Ok(response)
}
