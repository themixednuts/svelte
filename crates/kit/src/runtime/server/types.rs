use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use ::http::{
    HeaderMap, HeaderName, HeaderValue, Method, Request as HttpRequest, Response as HttpResponse,
    StatusCode, Uri,
};
use serde_json::{Map, Value};
use url::Url;

use crate::cookie::CookieJar;
use crate::manifest::ManifestRoute;
use crate::{CookieOptions, Error, Result, RuntimeHttpError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Page,
    Data,
    RouteResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequestUrl {
    pub kind: RequestKind,
    pub url: Url,
    pub invalidated_data_nodes: Option<Vec<bool>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedRuntimeRequest<'a> {
    pub prepared: PreparedRequestUrl,
    pub resolved_path: String,
    pub route: &'a ManifestRoute,
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RouteResolutionAssets {
    pub base: String,
    pub assets: String,
    pub relative: bool,
    pub start: Option<String>,
    pub nodes: Vec<Option<String>>,
    pub css: BTreeMap<usize, Vec<String>>,
}

#[derive(Debug)]
pub struct SpecialRuntimeRequestOptions<'a> {
    pub app_dir: String,
    pub base: String,
    pub hash_routing: bool,
    pub public_env: &'a Map<String, Value>,
    pub route_assets: &'a RouteResolutionAssets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequestOptions {
    pub base: String,
    pub app_dir: String,
    pub hash_routing: bool,
    pub csrf_check_origin: bool,
    pub csrf_trusted_origins: Vec<String>,
    pub public_env: Map<String, Value>,
    pub route_assets: RouteResolutionAssets,
}

#[derive(Debug, Clone)]
pub struct PreprocessedRuntimeRequest<'a> {
    pub remote_id: Option<String>,
    pub prepared: PreparedRequestUrl,
    pub rewritten_url: Url,
    pub resolved: Option<ResolvedRuntimeRequest<'a>>,
    pub early_response: Option<ServerResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeFatalError {
    Http {
        status: u16,
        body: Map<String, Value>,
    },
    Kit {
        status: u16,
        text: String,
    },
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDevalueError {
    pub message: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
}

impl ServerRequest {
    pub fn builder() -> ServerRequestBuilder {
        ServerRequestBuilder::default()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn try_method(&self) -> Result<Method> {
        Ok(self.method.clone())
    }

    pub fn method_is(&self, method: &Method) -> bool {
        self.try_method()
            .map(|candidate| candidate == *method)
            .unwrap_or(false)
    }

    pub fn try_header_map(&self) -> Result<HeaderMap> {
        Ok(self.headers.clone())
    }

    pub fn to_http_request(&self) -> Result<HttpRequest<()>> {
        let mut builder = HttpRequest::builder()
            .method(self.method.clone())
            .uri(parse_absolute_uri(&self.url)?);

        let headers = builder
            .headers_mut()
            .expect("request builder headers available before body");
        *headers = self.try_header_map()?;

        builder.body(()).map_err(|error| {
            RuntimeHttpError::BuildHttpRequest {
                message: error.to_string(),
            }
            .into()
        })
    }

    pub fn set_header(&mut self, name: &str, value: impl AsRef<str>) {
        let header_name =
            parse_header_name(name).expect("server request header name must be valid");
        let header_value = parse_header_value(name, value.as_ref())
            .expect("server request header value must be valid");
        self.headers.insert(header_name, header_value);
    }
}

impl TryFrom<&ServerRequest> for HttpRequest<()> {
    type Error = Error;

    fn try_from(value: &ServerRequest) -> Result<Self> {
        value.to_http_request()
    }
}

impl TryFrom<HttpRequest<()>> for ServerRequest {
    type Error = Error;

    fn try_from(request: HttpRequest<()>) -> Result<Self> {
        let (parts, _) = request.into_parts();
        let url = Url::parse(&parts.uri.to_string()).map_err(|error| {
            RuntimeHttpError::InvalidAbsoluteRequestUri {
                uri: parts.uri.to_string(),
                message: error.to_string(),
            }
        })?;
        Ok(Self {
            method: parts.method,
            url,
            headers: parts.headers,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ServerRequestBuilder {
    method: Option<Method>,
    url: Option<Url>,
    headers: HeaderMap,
}

impl ServerRequestBuilder {
    pub fn method(mut self, method: Method) -> Self {
        self.method = Some(method);
        self
    }

    pub fn try_method(mut self, method: &str) -> Result<Self> {
        self.method =
            Some(
                method
                    .parse::<Method>()
                    .map_err(|error| RuntimeHttpError::BuildHttpRequest {
                        message: error.to_string(),
                    })?,
            );
        Ok(self)
    }

    pub fn url(mut self, url: Url) -> Self {
        self.url = Some(url);
        self
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Result<Self> {
        let header_name = parse_header_name(name)?;
        let value = value.into();
        let header_value = parse_header_value(name, &value)?;
        self.headers.insert(header_name, header_value);
        Ok(self)
    }

    pub fn build(self) -> Result<ServerRequest> {
        let request = ServerRequest {
            method: self.method.unwrap_or(Method::GET),
            url: self.url.ok_or(RuntimeHttpError::MissingRequestUrl)?,
            headers: self.headers,
        };
        request.to_http_request()?;
        Ok(request)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RequestEventState {
    pub allows_commands: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRequestEvent {
    pub request: ServerRequest,
    pub route_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrerenderDependency {
    pub response: ServerResponse,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrerenderState {
    pub fallback: bool,
    pub inside_reroute: bool,
    pub cache: Option<String>,
    pub dependencies: BTreeMap<String, PrerenderDependency>,
}

#[derive(Debug, Clone)]
pub struct RuntimeRenderState {
    pub app_state: Arc<super::AppState>,
    pub error: bool,
    pub prerender_default: bool,
    pub prerendering: Option<PrerenderState>,
    pub depth: usize,
}

impl Default for RuntimeRenderState {
    fn default() -> Self {
        Self {
            app_state: Arc::new(super::AppState::default()),
            error: false,
            prerender_default: false,
            prerendering: None,
            depth: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedRuntimeExecution<'a> {
    pub preprocessed: PreprocessedRuntimeRequest<'a>,
    pub behavior: Option<RuntimeRouteBehavior>,
    pub dispatch: Option<RuntimeRouteDispatch>,
    pub event: Option<RuntimeEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerDataUses {
    pub dependencies: BTreeSet<String>,
    pub search_params: BTreeSet<String>,
    pub params: BTreeSet<String>,
    pub parent: bool,
    pub route: bool,
    pub url: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerDataNode {
    pub uses: Option<ServerDataUses>,
}

#[derive(Debug, Clone)]
pub struct ServerResponseBuilder {
    status: StatusCode,
    headers: HeaderMap,
    body: Option<String>,
}

impl ServerResponseBuilder {
    pub fn new(status: u16) -> Self {
        Self {
            status: StatusCode::from_u16(status)
                .expect("server response builder status must be valid"),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status =
            StatusCode::from_u16(status).expect("server response builder status must be valid");
        self
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Result<Self> {
        let normalized = parse_header_name(name)?;
        let value = value.into();
        let header_value = parse_header_value(normalized.as_str(), &value)?;
        self.headers.insert(normalized, header_value);
        Ok(self)
    }

    pub fn append_header(mut self, name: &str, value: impl Into<String>) -> Result<Self> {
        let normalized = parse_header_name(name)?;
        let value = value.into();
        let header_value = parse_header_value(normalized.as_str(), &value)?;
        self.headers.append(normalized, header_value);
        Ok(self)
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn build(self) -> Result<ServerResponse> {
        let response = ServerResponse {
            status: self.status,
            headers: self.headers,
            body: self.body,
        };
        response.to_http_response()?;
        Ok(response)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeCookies(Arc<Mutex<CookieJar>>);

impl RuntimeCookies {
    pub fn new(cookie_header: Option<&str>, url: &Url) -> Self {
        Self(Arc::new(Mutex::new(CookieJar::new(cookie_header, url))))
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(name)
    }

    pub fn set(&self, name: &str, value: &str, options: CookieOptions) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .set(name, value, options)
    }

    pub fn delete(&self, name: &str, options: CookieOptions) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .delete(name, options)
    }

    pub fn set_trailing_slash(&self, trailing_slash: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .set_trailing_slash(trailing_slash)
    }

    pub fn set_cookie_headers(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .set_cookie_headers()
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeResponseHeaders(Arc<Mutex<HeaderMap>>);

impl RuntimeResponseHeaders {
    pub fn is_empty(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
    }

    pub fn snapshot(&self) -> HeaderMap {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeEvent {
    pub app_state: Arc<super::AppState>,
    pub cookies: RuntimeCookies,
    pub params: BTreeMap<String, String>,
    pub request: ServerRequest,
    pub route_id: Option<String>,
    pub response_headers: RuntimeResponseHeaders,
    pub url: Url,
    pub is_data_request: bool,
    pub is_sub_request: bool,
    pub is_remote_request: bool,
}

impl RuntimeEvent {
    pub fn set_headers(
        &self,
        headers: &HeaderMap,
        prerendering: Option<&mut PrerenderState>,
    ) -> Result<()> {
        let mut header_map = self
            .response_headers
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut prerendering = prerendering;
        for (name, value) in headers {
            let Ok(value) = value.to_str() else {
                continue;
            };
            let prerendering_ref = match prerendering.as_mut() {
                Some(prerendering) => Some(&mut **prerendering),
                None => None,
            };
            super::set_response_header(&mut header_map, name.as_str(), value, prerendering_ref)?;
        }
        Ok(())
    }

    pub fn apply_response_effects(&self, response: &mut ServerResponse) {
        super::apply_runtime_response_effects(response, &self.capture_response_effects());
    }

    pub fn capture_response_effects(&self) -> super::RuntimeResponseEffects {
        super::RuntimeResponseEffects {
            headers: self.response_headers.snapshot(),
            set_cookie_headers: self.cookies.set_cookie_headers(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRouteBehavior {
    pub trailing_slash: String,
    pub prerender: bool,
    pub config: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeRouteDispatch {
    Data,
    Endpoint,
    Page,
    PageMethodNotAllowed(ServerResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataRequestNode {
    Data {
        data: Value,
        uses: Option<ServerDataUses>,
        slash: Option<String>,
    },
    Skip,
    Error {
        status: Option<u16>,
        error: Value,
    },
    Redirect {
        location: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDataRequest {
    pub normalized_pathname: String,
    pub node_indexes: Vec<Option<usize>>,
    pub invalidated: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRequestEnvelope {
    pub nodes: Vec<Value>,
}

impl ServerResponse {
    pub fn builder(status: u16) -> ServerResponseBuilder {
        ServerResponseBuilder::new(status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn header_values(&self, name: &str) -> Option<Vec<&str>> {
        let values = self
            .headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>();
        (!values.is_empty()).then_some(values)
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers.contains_key(name)
    }

    pub fn try_status_code(&self) -> Result<StatusCode> {
        Ok(self.status)
    }

    pub fn try_header_map(&self) -> Result<HeaderMap> {
        Ok(self.headers.clone())
    }

    pub fn to_http_response(&self) -> Result<HttpResponse<Option<String>>> {
        let mut builder = HttpResponse::builder().status(self.try_status_code()?);
        let headers = builder
            .headers_mut()
            .expect("response builder headers available before body");
        *headers = self.try_header_map()?;

        builder.body(self.body.clone()).map_err(|error| {
            RuntimeHttpError::BuildHttpResponse {
                message: error.to_string(),
            }
            .into()
        })
    }
}

impl TryFrom<&ServerResponse> for HttpResponse<Option<String>> {
    type Error = Error;

    fn try_from(value: &ServerResponse) -> Result<Self> {
        value.to_http_response()
    }
}

impl TryFrom<HttpResponse<Option<String>>> for ServerResponse {
    type Error = Error;

    fn try_from(response: HttpResponse<Option<String>>) -> Result<Self> {
        let (parts, body) = response.into_parts();
        Ok(Self {
            status: parts.status,
            headers: parts.headers,
            body,
        })
    }
}

fn parse_header_name(name: &str) -> Result<HeaderName> {
    HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
        RuntimeHttpError::InvalidHeaderName {
            name: name.to_string(),
            message: error.to_string(),
        }
        .into()
    })
}

fn parse_header_value(name: &str, value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value).map_err(|error| {
        RuntimeHttpError::InvalidHeaderValue {
            name: name.to_string(),
            message: error.to_string(),
        }
        .into()
    })
}

fn parse_absolute_uri(url: &Url) -> Result<Uri> {
    url.as_str().parse::<Uri>().map_err(|error| {
        RuntimeHttpError::InvalidAbsoluteRequestUri {
            uri: url.to_string(),
            message: error.to_string(),
        }
        .into()
    })
}
