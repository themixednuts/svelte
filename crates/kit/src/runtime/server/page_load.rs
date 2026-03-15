use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use ::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value};
use url::Url;

use crate::runtime::shared::{validate_depends, validate_load_response};
use crate::{CookieOptions, Result, RuntimeLoadError, SameSite};

use super::{
    DataRequestNode, FetchedResponse, PrerenderDependency, RuntimeEvent, RuntimeRenderState,
    ServerDataUses, ServerResponse,
};

pub struct UniversalFetchCookieHeader {
    inner: Arc<dyn Fn(&Url, Option<&str>) -> String + Send + Sync>,
}

impl Clone for UniversalFetchCookieHeader {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl UniversalFetchCookieHeader {
    pub fn new<F>(get_cookie_header: F) -> Self
    where
        F: Fn(&Url, Option<&str>) -> String + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(get_cookie_header),
        }
    }

    pub fn call(&self, url: &Url, existing: Option<&str>) -> String {
        (self.inner)(url, existing)
    }
}

pub struct UniversalFetchCookieSetter {
    inner: Arc<dyn Fn(&str, &str, CookieOptions) -> Result<()> + Send + Sync>,
}

impl Clone for UniversalFetchCookieSetter {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl UniversalFetchCookieSetter {
    pub fn new<F>(set_cookie: F) -> Self
    where
        F: Fn(&str, &str, CookieOptions) -> Result<()> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(set_cookie),
        }
    }

    pub fn call(&self, name: &str, value: &str, options: CookieOptions) -> Result<()> {
        (self.inner)(name, value, options)
    }
}

pub struct UniversalFetchHandle {
    inner: Arc<
        dyn for<'a> Fn(UniversalFetchHandleContext<'a>) -> Result<UniversalFetchRawResponse>
            + Send
            + Sync,
    >,
}

impl Clone for UniversalFetchHandle {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl UniversalFetchHandle {
    pub fn new<F>(handle_fetch: F) -> Self
    where
        F: for<'a> Fn(UniversalFetchHandleContext<'a>) -> Result<UniversalFetchRawResponse>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner: Arc::new(handle_fetch),
        }
    }

    pub fn call(
        &self,
        context: UniversalFetchHandleContext<'_>,
    ) -> Result<UniversalFetchRawResponse> {
        (self.inner)(context)
    }
}

pub struct UniversalFetchHandleContext<'a> {
    pub url: &'a Url,
    pub options: &'a UniversalFetchOptions,
    pub fetch: &'a mut dyn FnMut(&str, &UniversalFetchOptions) -> Result<UniversalFetchRawResponse>,
}

#[derive(Clone)]
pub struct UniversalFetchContext {
    pub event_url: Url,
    pub get_cookie_header: Option<UniversalFetchCookieHeader>,
    pub handle_fetch: Option<UniversalFetchHandle>,
    pub set_internal_cookie: Option<UniversalFetchCookieSetter>,
    pub request_headers: HeaderMap,
    pub request_method: Method,
    pub route_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UniversalFetchMode {
    #[default]
    Cors,
    NoCors,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum UniversalFetchCredentials {
    Include,
    Omit,
    #[default]
    SameOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniversalFetchOptions {
    pub credentials: UniversalFetchCredentials,
    pub method: Method,
    pub mode: UniversalFetchMode,
    pub headers: HeaderMap,
    pub body: Option<String>,
}

impl Default for UniversalFetchOptions {
    fn default() -> Self {
        Self {
            credentials: UniversalFetchCredentials::SameOrigin,
            method: Method::GET,
            mode: UniversalFetchMode::Cors,
            headers: HeaderMap::new(),
            body: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UniversalFetchBody {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniversalFetchRawResponse {
    pub status: StatusCode,
    pub status_text: String,
    pub headers: HeaderMap,
    pub body: UniversalFetchBody,
}

impl UniversalFetchRawResponse {
    pub fn text(body: &str) -> Self {
        Self {
            status: StatusCode::OK,
            status_text: String::new(),
            headers: HeaderMap::new(),
            body: UniversalFetchBody::Text(body.to_string()),
        }
    }

    pub fn binary(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: StatusCode::OK,
            status_text: String::new(),
            headers: HeaderMap::new(),
            body: UniversalFetchBody::Binary(body.into()),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.insert(
            HeaderName::from_bytes(name.as_bytes()).expect("valid universal fetch header name"),
            HeaderValue::from_str(value).expect("valid universal fetch header value"),
        );
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniversalFetchResponseHeaders {
    headers: HeaderMap,
    readable: BTreeSet<String>,
    route_id: Option<String>,
}

impl UniversalFetchResponseHeaders {
    pub fn get(&self, name: &str) -> Result<Option<String>> {
        let lowered = name.to_ascii_lowercase();

        let Some(value) = self.headers.get(&lowered) else {
            return Ok(None);
        };

        if lowered.starts_with("x-sveltekit-") || self.readable.contains(&lowered) {
            return Ok(Some(
                value
                    .to_str()
                    .map_err(|error| RuntimeLoadError::InvalidResponseHeaderValue {
                        name: name.to_string(),
                        message: error.to_string(),
                    })?
                    .to_string(),
            ));
        }

        Err(RuntimeLoadError::FilteredResponseHeader {
            name: name.to_string(),
            route_suffix: self
                .route_id
                .as_ref()
                .map(|route_id| format!(" (at {route_id})"))
                .unwrap_or_default(),
        }
        .into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniversalFetchResponse {
    body: UniversalFetchBody,
    body_error: Option<String>,
    headers: UniversalFetchResponseHeaders,
}

impl UniversalFetchResponse {
    pub fn text(&self) -> Result<String> {
        if let Some(error) = &self.body_error {
            return Err(RuntimeLoadError::ResponseBodyUnavailable {
                message: error.clone(),
            }
            .into());
        }

        Ok(match &self.body {
            UniversalFetchBody::Text(body) => body.clone(),
            UniversalFetchBody::Binary(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        })
    }

    pub fn json(&self) -> Result<Value> {
        let body = self.text()?;
        if body.is_empty() {
            return Ok(Value::Null);
        }

        serde_json::from_str(&body).map_err(|error| {
            RuntimeLoadError::JsonResponseParse {
                message: error.to_string(),
            }
            .into()
        })
    }

    pub fn array_buffer(&self) -> Result<Vec<u8>> {
        if let Some(error) = &self.body_error {
            return Err(RuntimeLoadError::ResponseBodyUnavailable {
                message: error.clone(),
            }
            .into());
        }

        Ok(match &self.body {
            UniversalFetchBody::Text(body) => body.as_bytes().to_vec(),
            UniversalFetchBody::Binary(bytes) => bytes.clone(),
        })
    }

    pub fn headers(&self) -> &UniversalFetchResponseHeaders {
        &self.headers
    }
}

pub struct UniversalFetch {
    context: UniversalFetchContext,
    state: RuntimeRenderState,
    filter: Box<dyn Fn(&str, &str) -> bool>,
    fetcher: Box<dyn FnMut(&str, &UniversalFetchOptions) -> UniversalFetchRawResponse>,
    fetched: Vec<FetchedResponse>,
}

pub struct UniversalLoadContext<'a> {
    event: &'a RuntimeEvent,
    route_id: Option<&'a str>,
    server_data: Option<&'a Value>,
    parent: &'a mut dyn FnMut() -> Result<Map<String, Value>>,
    fetch: &'a mut UniversalFetch,
}

impl<'a> UniversalLoadContext<'a> {
    pub fn url(&self) -> &Url {
        &self.event.url
    }

    pub fn param(&self, key: &str) -> Option<&str> {
        self.event.params.get(key).map(String::as_str)
    }

    pub fn route_id(&self) -> Option<&str> {
        self.route_id.or(self.event.route_id.as_deref())
    }

    pub fn data(&self) -> Option<&Value> {
        self.server_data
    }

    pub fn parent(&mut self) -> Result<Map<String, Value>> {
        (self.parent)()
    }

    pub fn fetch(
        &mut self,
        input: &str,
        options: UniversalFetchOptions,
    ) -> Result<UniversalFetchResponse> {
        self.fetch.fetch(input, options)
    }

    pub fn set_headers(&self, headers: &HeaderMap) -> Result<()> {
        self.event.set_headers(headers, None)
    }

    pub fn depends(&self, _dependencies: &[&str]) {}

    pub fn untrack<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        f(self)
    }
}

pub struct ServerLoadContext<'a> {
    event: &'a RuntimeEvent,
    route_id: Option<&'a str>,
    uses: ServerDataUses,
    is_tracking: bool,
    parent: &'a mut dyn FnMut() -> Result<Map<String, Value>>,
}

impl<'a> ServerLoadContext<'a> {
    pub fn depends(&mut self, dependency: &str) -> Result<()> {
        let resolved = self.event.url.join(dependency).map_err(|error| {
            RuntimeLoadError::InvalidDependency {
                dependency: dependency.to_string(),
                message: error.to_string(),
            }
        })?;
        let _ = self
            .route_id
            .and_then(|route_id| validate_depends(route_id, dependency));
        if self.is_tracking {
            self.uses.dependencies.insert(resolved.to_string());
        }
        Ok(())
    }

    pub fn param(&mut self, key: &str) -> Option<String> {
        if self.is_tracking {
            self.uses.params.insert(key.to_string());
        }
        self.event.params.get(key).cloned()
    }

    pub fn route_id(&mut self) -> Option<String> {
        if self.is_tracking {
            self.uses.route = true;
        }
        self.event.route_id.clone()
    }

    pub fn url(&mut self) -> &Url {
        if self.is_tracking {
            self.uses.url = true;
        }
        &self.event.url
    }

    pub fn search_param(&mut self, key: &str) -> Option<String> {
        if self.is_tracking {
            self.uses.search_params.insert(key.to_string());
        }
        self.event
            .url
            .query_pairs()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.into_owned())
    }

    pub fn parent(&mut self) -> Result<Map<String, Value>> {
        if self.is_tracking {
            self.uses.parent = true;
        }
        (self.parent)()
    }

    pub fn untrack<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let was_tracking = self.is_tracking;
        self.is_tracking = false;
        let result = f(self);
        self.is_tracking = was_tracking;
        result
    }

    pub fn uses(&self) -> &ServerDataUses {
        &self.uses
    }

    fn into_uses(self) -> ServerDataUses {
        self.uses
    }
}

pub fn load_server_data<F, P>(
    event: &RuntimeEvent,
    route_id: Option<&str>,
    trailing_slash: Option<&str>,
    mut parent: P,
    load: F,
) -> Result<DataRequestNode>
where
    F: FnOnce(&mut ServerLoadContext<'_>) -> Result<Option<Value>>,
    P: FnMut() -> Result<Map<String, Value>>,
{
    let mut context = ServerLoadContext {
        event,
        route_id,
        uses: ServerDataUses::default(),
        is_tracking: true,
        parent: &mut parent,
    };

    let data = load(&mut context)?.unwrap_or(Value::Null);
    let location = route_id.map(|route_id| format!("in {route_id}"));
    validate_load_response(&data, location.as_deref())?;

    Ok(DataRequestNode::Data {
        data,
        uses: Some(context.into_uses()),
        slash: trailing_slash.map(str::to_string),
    })
}

pub fn load_data<F, P>(
    event: &RuntimeEvent,
    route_id: Option<&str>,
    server_data: Option<Value>,
    mut parent: P,
    fetch: &mut UniversalFetch,
    load: Option<F>,
) -> Result<Option<Value>>
where
    F: FnOnce(&mut UniversalLoadContext<'_>) -> Result<Option<Value>>,
    P: FnMut() -> Result<Map<String, Value>>,
{
    let Some(load) = load else {
        return Ok(server_data);
    };

    let mut context = UniversalLoadContext {
        event,
        route_id,
        server_data: server_data.as_ref(),
        parent: &mut parent,
        fetch,
    };

    let data = load(&mut context)?.unwrap_or(Value::Null);
    let location = route_id.map(|route_id| format!("in {route_id}"));
    validate_load_response(&data, location.as_deref())?;
    Ok(Some(data))
}

pub fn create_universal_fetch<F, Filter>(
    context: UniversalFetchContext,
    state: RuntimeRenderState,
    _csr: bool,
    filter: Filter,
    fetcher: F,
) -> UniversalFetch
where
    F: FnMut(&str, &UniversalFetchOptions) -> UniversalFetchRawResponse + 'static,
    Filter: Fn(&str, &str) -> bool + 'static,
{
    UniversalFetch {
        context,
        state,
        filter: Box::new(filter),
        fetcher: Box::new(fetcher),
        fetched: Vec::new(),
    }
}

impl UniversalFetch {
    pub fn state(&self) -> &RuntimeRenderState {
        &self.state
    }

    pub fn fetch(
        &mut self,
        input: &str,
        mut options: UniversalFetchOptions,
    ) -> Result<UniversalFetchResponse> {
        let resolved = resolve_fetch_url(&self.context.event_url, input)?;
        let same_origin = same_origin(&resolved, &self.context.event_url);
        propagate_request_headers(&self.context, &resolved, &mut options, same_origin);
        normalize_origin_header(&self.context.event_url, &resolved, &mut options);
        let mut fetch =
            |url: &str, options: &UniversalFetchOptions| Ok((self.fetcher)(url, options));
        let raw = if let Some(handle_fetch) = &self.context.handle_fetch {
            handle_fetch.call(UniversalFetchHandleContext {
                url: &resolved,
                options: &options,
                fetch: &mut fetch,
            })?
        } else {
            fetch(resolved.as_str(), &options)?
        };
        capture_set_cookie_headers(&self.context, &resolved, &raw.headers)?;

        let readable = raw
            .headers
            .iter()
            .filter_map(|(name, value)| {
                let value = value.to_str().ok()?;
                ((self.filter)(name.as_str(), value)).then_some(name.as_str().to_ascii_lowercase())
            })
            .collect::<BTreeSet<_>>();

        let body_error = cors_body_error(
            &resolved,
            &self.context.event_url,
            &options.mode,
            &raw.headers,
        );
        let no_cors_empty = matches!(options.mode, UniversalFetchMode::NoCors)
            && is_network_scheme(&resolved)
            && !same_origin;
        let (body, recorded_body, is_b64) = if no_cors_empty {
            (
                UniversalFetchBody::Text(String::new()),
                String::new(),
                false,
            )
        } else {
            match &raw.body {
                UniversalFetchBody::Text(body) => {
                    (UniversalFetchBody::Text(body.clone()), body.clone(), false)
                }
                UniversalFetchBody::Binary(bytes) => (
                    UniversalFetchBody::Binary(bytes.clone()),
                    STANDARD.encode(bytes),
                    true,
                ),
            }
        };

        if same_origin && let Some(prerendering) = &mut self.state.prerendering {
            let mut response = ServerResponse::new(raw.status.as_u16());
            response.headers = raw.headers.clone();
            prerendering.dependencies.insert(
                resolved.path().to_string(),
                PrerenderDependency {
                    response,
                    body: Some(recorded_body.clone()),
                },
            );
        }

        self.fetched.push(FetchedResponse {
            url: recorded_url(&resolved, &self.context.event_url),
            method: self.context.request_method.clone(),
            request_body: options.body.clone(),
            request_headers: (!options.headers.is_empty())
                .then_some(normalize_headers(options.headers.clone())),
            response_body: recorded_body,
            response_status: raw.status,
            response_status_text: raw.status_text,
            response_headers: normalize_headers(raw.headers.clone()),
            is_b64,
        });

        Ok(UniversalFetchResponse {
            body,
            body_error,
            headers: UniversalFetchResponseHeaders {
                headers: raw.headers,
                readable,
                route_id: self.context.route_id.clone(),
            },
        })
    }

    pub fn take_fetched(&mut self) -> Vec<FetchedResponse> {
        std::mem::take(&mut self.fetched)
    }
}

fn resolve_fetch_url(base: &Url, input: &str) -> Result<Url> {
    if let Ok(url) = Url::parse(input) {
        return Ok(url);
    }

    base.join(input).map_err(|error| {
        RuntimeLoadError::InvalidFetchUrl {
            message: error.to_string(),
        }
        .into()
    })
}

fn recorded_url(url: &Url, event_url: &Url) -> String {
    if same_origin(url, event_url) {
        let mut path = url.path().to_string();
        if let Some(query) = url.query() {
            path.push('?');
            path.push_str(query);
        }
        path
    } else {
        url.to_string()
    }
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn is_network_scheme(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

fn cors_body_error(
    url: &Url,
    event_url: &Url,
    mode: &UniversalFetchMode,
    headers: &HeaderMap,
) -> Option<String> {
    if !is_network_scheme(url)
        || same_origin(url, event_url)
        || matches!(mode, UniversalFetchMode::NoCors)
    {
        return None;
    }

    match headers.get("access-control-allow-origin").and_then(|value| value.to_str().ok()) {
        Some(value) if value == "*" || value == event_url.origin().ascii_serialization().as_str() => {
            None
        }
        Some(_) => Some(
            "CORS error: Incorrect 'Access-Control-Allow-Origin' header is present on the requested resource"
                .to_string(),
        ),
        None => Some(
            "CORS error: No 'Access-Control-Allow-Origin' header is present on the requested resource"
                .to_string(),
        ),
    }
}

fn normalize_headers(headers: HeaderMap) -> BTreeMap<String, String> {
    headers
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name?;
            let value = value.to_str().ok()?.to_string();
            Some((name.as_str().to_ascii_lowercase(), value))
        })
        .collect()
}

fn propagate_request_headers(
    context: &UniversalFetchContext,
    destination: &Url,
    options: &mut UniversalFetchOptions,
    same_origin: bool,
) {
    if !options.headers.contains_key("accept") {
        options.headers.insert(
            HeaderName::from_static("accept"),
            HeaderValue::from_static("*/*"),
        );
    }

    if !options.headers.contains_key("accept-language")
        && let Some(value) = context.request_headers.get("accept-language")
    {
        options
            .headers
            .insert(HeaderName::from_static("accept-language"), value.clone());
    }

    if !matches!(options.credentials, UniversalFetchCredentials::Omit)
        && let Some(get_cookie_header) = &context.get_cookie_header
        && (same_origin || same_origin_domain(destination, &context.event_url))
    {
        let existing = options
            .headers
            .get("cookie")
            .and_then(|value| value.to_str().ok());
        let cookie = get_cookie_header.call(destination, existing);
        if !cookie.is_empty() {
            options.headers.insert(
                HeaderName::from_static("cookie"),
                HeaderValue::from_str(&cookie).expect("cookie header value must be valid"),
            );
        }
    }

    if matches!(options.credentials, UniversalFetchCredentials::Omit) || !same_origin {
        return;
    }

    if !options.headers.contains_key("authorization")
        && let Some(value) = context.request_headers.get("authorization")
    {
        options
            .headers
            .insert(HeaderName::from_static("authorization"), value.clone());
    }
}

fn same_origin_domain(left: &Url, right: &Url) -> bool {
    match (left.host_str(), right.host_str()) {
        (Some(left), Some(right)) => format!(".{left}").ends_with(&format!(".{right}")),
        _ => false,
    }
}

fn normalize_origin_header(
    event_url: &Url,
    destination: &Url,
    options: &mut UniversalFetchOptions,
) {
    if !is_network_scheme(destination) {
        return;
    }

    if matches!(options.method, Method::GET | Method::HEAD)
        && (same_origin(destination, event_url)
            || matches!(options.mode, UniversalFetchMode::NoCors))
    {
        options.headers.remove("origin");
        return;
    }

    if options.headers.contains_key("origin") {
        return;
    }

    options.headers.insert(
        HeaderName::from_static("origin"),
        HeaderValue::from_str(&event_url.origin().ascii_serialization())
            .expect("origin header value must be valid"),
    );
}

fn capture_set_cookie_headers(
    context: &UniversalFetchContext,
    destination: &Url,
    headers: &HeaderMap,
) -> Result<()> {
    let Some(set_internal_cookie) = &context.set_internal_cookie else {
        return Ok(());
    };

    for value in headers.get_all("set-cookie") {
        let Ok(value) = value.to_str() else {
            continue;
        };

        let Some((name, cookie_value, options)) = parse_set_cookie(value, destination) else {
            continue;
        };

        set_internal_cookie.call(&name, &cookie_value, options)?;
    }

    Ok(())
}

fn parse_set_cookie(header: &str, destination: &Url) -> Option<(String, String, CookieOptions)> {
    let mut parts = header.split(';').map(str::trim);
    let (name, value) = parts.next()?.split_once('=')?;
    let mut options = CookieOptions {
        path: Some(default_cookie_path(destination)),
        encode: Some(identity_cookie_encode),
        ..Default::default()
    };

    for attribute in parts {
        let (key, raw_value) = match attribute.split_once('=') {
            Some((key, value)) => (key.trim(), Some(value.trim())),
            None => (attribute, None),
        };

        match key.to_ascii_lowercase().as_str() {
            "path" => options.path = raw_value.map(str::to_string),
            "domain" => options.domain = raw_value.map(str::to_string),
            "max-age" => {
                options.max_age = raw_value.and_then(|value| value.parse::<i64>().ok());
            }
            "samesite" => {
                options.same_site = match raw_value.map(|value| value.to_ascii_lowercase()) {
                    Some(value) if value == "lax" => Some(SameSite::Lax),
                    Some(value) if value == "strict" => Some(SameSite::Strict),
                    Some(value) if value == "none" => Some(SameSite::None),
                    _ => options.same_site,
                };
            }
            "secure" => options.secure = Some(true),
            "httponly" => options.http_only = Some(true),
            _ => {}
        }
    }

    Some((name.to_string(), value.to_string(), options))
}

fn default_cookie_path(destination: &Url) -> String {
    let path = destination.path();
    path.rsplit_once('/')
        .map(|(prefix, _)| {
            if prefix.is_empty() {
                "/".to_string()
            } else {
                prefix.to_string()
            }
        })
        .unwrap_or_else(|| "/".to_string())
}

fn identity_cookie_encode(value: &str) -> String {
    value.to_string()
}
