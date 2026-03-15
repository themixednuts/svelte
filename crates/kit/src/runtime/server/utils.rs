use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
};

use ::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use camino::Utf8Path;
use serde_json::{Map, Value};
use url::Url;

use crate::cookie::CookieJar;
use crate::http::{is_form_content_type, negotiate};
use crate::manifest::{ClientRoute, KitManifest};
use crate::{Result, RuntimeHttpError};

use super::{
    RequestKind, RouteResolutionAssets, RuntimeDevalueError, RuntimeFatalError, ServerDataNode,
    ServerRequest, ServerResponse, SpecialRuntimeRequestOptions, prepare_request_url,
    resolve_runtime_request,
};

pub fn create_server_routing_response(
    route: Option<&ClientRoute>,
    params: &BTreeMap<String, String>,
    request_url: &Url,
    assets: &RouteResolutionAssets,
) -> ServerResponse {
    let mut response = ServerResponse::new(200);
    response.set_header("content-type", "application/javascript; charset=utf-8");

    let Some(route) = route else {
        response.body = Some(String::new());
        return response;
    };

    let mut body = create_css_import(route, request_url, assets);
    if !body.is_empty() {
        body.push('\n');
    }
    body.push_str("export const route = ");
    body.push_str(&generate_route_object(route, request_url, assets));
    body.push_str("; export const params = ");
    body.push_str(&serde_json::to_string(params).expect("serialize route params"));
    body.push(';');
    response.body = Some(body);
    response
}

pub fn resolve_route_request_response<F>(
    manifest: &KitManifest,
    request_url: &Url,
    base: &str,
    assets: &RouteResolutionAssets,
    matches: F,
) -> Result<ServerResponse>
where
    F: FnMut(&str, &str) -> bool,
{
    let resolved = resolve_runtime_request(manifest, request_url, base, matches)?;
    let client_routes = manifest.build_client_routes();
    let route = resolved.as_ref().and_then(|resolved| {
        client_routes
            .iter()
            .find(|route| route.id == resolved.route.id)
    });
    let empty_params = BTreeMap::new();
    let params = resolved
        .as_ref()
        .map(|resolved| &resolved.params)
        .unwrap_or(&empty_params);

    Ok(create_server_routing_response(
        route,
        params,
        request_url,
        assets,
    ))
}

pub fn public_env_response(
    request: &ServerRequest,
    public_env: &Map<String, Value>,
) -> ServerResponse {
    let body = format!(
        "export const env={}",
        serde_json::to_string(public_env).expect("serialize public env")
    );
    let etag = weak_etag(&body);

    if request.header("if-none-match") == Some(etag.as_str()) {
        return ServerResponse::builder(304)
            .header("content-type", "application/javascript; charset=utf-8")
            .expect("public env content-type is valid")
            .header("etag", etag)
            .expect("public env etag is valid")
            .build()
            .expect("public env 304 response is valid");
    }

    ServerResponse::builder(200)
        .header("content-type", "application/javascript; charset=utf-8")
        .expect("public env content-type is valid")
        .header("etag", etag)
        .expect("public env etag is valid")
        .body(body)
        .build()
        .expect("public env response is valid")
}

pub fn dispatch_special_runtime_request<F>(
    manifest: &KitManifest,
    request: &ServerRequest,
    options: &SpecialRuntimeRequestOptions<'_>,
    matches: F,
) -> Result<Option<ServerResponse>>
where
    F: FnMut(&str, &str) -> bool,
{
    let request_url = &request.url;
    if options.hash_routing
        && request_url.path() != format!("{}/", options.base.trim_end_matches('/'))
        && request_url.path() != "/[fallback]"
    {
        let mut response = ServerResponse::new(404);
        response.body = Some("Not found".to_string());
        response.set_header("content-type", "text/plain; charset=utf-8");
        return Ok(Some(response));
    }

    let prepared = prepare_request_url(request_url)?;
    let app_dir_path = format!(
        "{}/{}",
        options.base.trim_end_matches('/'),
        options.app_dir.trim_start_matches('/')
    );

    if prepared.url.path() == format!("{app_dir_path}/env.js") {
        return Ok(Some(public_env_response(request, options.public_env)));
    }

    if prepared.kind == RequestKind::RouteResolution {
        return resolve_route_request_response(
            manifest,
            request_url,
            &options.base,
            options.route_assets,
            matches,
        )
        .map(Some);
    }

    if prepared.url.path().starts_with(&app_dir_path) {
        let mut response = ServerResponse::new(404);
        response.body = Some("Not found".to_string());
        response.set_header("content-type", "text/plain; charset=utf-8");
        response.set_header("cache-control", "public, max-age=0, must-revalidate");
        return Ok(Some(response));
    }

    Ok(None)
}

pub fn has_prerendered_path(prerendered_routes: &BTreeSet<String>, pathname: &str) -> bool {
    prerendered_routes.contains(pathname)
        || (pathname.ends_with('/') && prerendered_routes.contains(pathname.trim_end_matches('/')))
}

pub fn handle_fatal_error(
    request: &ServerRequest,
    is_data_request: bool,
    error: &RuntimeFatalError,
    render_error: impl FnOnce(u16, &str) -> String,
    dev: bool,
) -> ServerResponse {
    let status = runtime_error_status(error);
    let body = runtime_error_body(error);

    let negotiated = negotiate(
        request.header("accept").unwrap_or("text/html"),
        &["application/json", "text/html"],
    );

    if is_data_request || negotiated == Some("application/json") {
        let mut response = ServerResponse::new(status);
        response.set_header("content-type", "application/json; charset=utf-8");
        response.body = Some(serde_json::to_string(&body).expect("serialize fatal error body"));
        return response;
    }

    static_error_page(
        status,
        body.get("message")
            .and_then(Value::as_str)
            .unwrap_or("Internal Error"),
        render_error,
        dev,
    )
}

pub fn format_server_error(
    status: u16,
    stack: Option<&str>,
    request: &ServerRequest,
    _dev: bool,
) -> String {
    let formatted = format!(
        "\n\x1b[1;31m[{status}] {} {}\x1b[0m",
        request.method,
        request.url.path()
    );

    if status == 404 {
        return formatted;
    }

    match stack {
        Some(stack) if !stack.is_empty() => format!("{formatted}\n{stack}"),
        _ => formatted,
    }
}

pub fn clarify_devalue_error(route_id: &str, error: &RuntimeDevalueError) -> String {
    match error.path.as_deref() {
        Some(path) if !path.is_empty() => format!(
            "Data returned from `load` while rendering {route_id} is not serializable: {} ({}). If you need to serialize/deserialize custom types, use transport hooks: https://svelte.dev/docs/kit/hooks#Universal-hooks-transport.",
            error.message, path
        ),
        Some(_) => {
            format!("Data returned from `load` while rendering {route_id} is not a plain object")
        }
        None => error.message.clone(),
    }
}

pub fn set_response_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
    prerendering: Option<&mut super::PrerenderState>,
) -> Result<()> {
    let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
        RuntimeHttpError::InvalidHeaderName {
            name: name.to_string(),
            message: error.to_string(),
        }
    })?;
    let lower = header_name.as_str().to_string();
    let header_value =
        HeaderValue::from_str(value).map_err(|error| RuntimeHttpError::InvalidHeaderValue {
            name: lower.clone(),
            message: error.to_string(),
        })?;

    if lower == "set-cookie" {
        return Err(RuntimeHttpError::SetCookieViaHeaders.into());
    }

    if let Some(existing) = headers.get_mut(&header_name) {
        if lower == "server-timing" {
            let mut combined = existing
                .to_str()
                .map_err(|error| RuntimeHttpError::InvalidHeaderValue {
                    name: lower.clone(),
                    message: error.to_string(),
                })?
                .to_string();
            combined.push_str(", ");
            combined.push_str(value);
            *existing = HeaderValue::from_str(&combined).map_err(|error| {
                RuntimeHttpError::InvalidHeaderValue {
                    name: lower.clone(),
                    message: error.to_string(),
                }
            })?;
            return Ok(());
        }

        return Err(RuntimeHttpError::DuplicateHeader { name: lower }.into());
    }

    headers.insert(header_name, header_value);

    if lower == "cache-control" {
        if let Some(prerendering) = prerendering {
            prerendering.cache = Some(value.to_string());
        }
    }

    Ok(())
}

pub fn set_response_cookies(response: &mut ServerResponse, cookies: &CookieJar) {
    for header in cookies.set_cookie_headers() {
        response.append_header("set-cookie", header);
    }
}

pub fn serialize_uses(node: &ServerDataNode) -> Map<String, Value> {
    let mut uses = Map::new();

    if let Some(node_uses) = &node.uses {
        if !node_uses.dependencies.is_empty() {
            uses.insert(
                "dependencies".to_string(),
                Value::Array(
                    node_uses
                        .dependencies
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }

        if !node_uses.search_params.is_empty() {
            uses.insert(
                "search_params".to_string(),
                Value::Array(
                    node_uses
                        .search_params
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }

        if !node_uses.params.is_empty() {
            uses.insert(
                "params".to_string(),
                Value::Array(
                    node_uses
                        .params
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }

        if node_uses.parent {
            uses.insert("parent".to_string(), Value::from(1));
        }
        if node_uses.route {
            uses.insert("route".to_string(), Value::from(1));
        }
        if node_uses.url {
            uses.insert("url".to_string(), Value::from(1));
        }
    }

    uses
}

pub fn get_node_type(node_id: Option<&str>) -> String {
    node_id
        .and_then(|node_id| node_id.rsplit('/').next())
        .map(|file| {
            file.rsplit_once('.')
                .map(|(stem, _)| stem)
                .unwrap_or(file)
                .to_string()
        })
        .unwrap_or_default()
}

pub fn check_csrf(
    request: &ServerRequest,
    csrf_check_origin: bool,
    trusted_origins: &[String],
) -> Option<ServerResponse> {
    if !csrf_check_origin
        || !is_form_content_type(request.header("content-type"))
        || !request.method_is(&Method::POST)
            && !request.method_is(&Method::PUT)
            && !request.method_is(&Method::PATCH)
            && !request.method_is(&Method::DELETE)
    {
        return None;
    }

    let request_origin = request.header("origin");
    let same_origin = request.url.origin().ascii_serialization();
    if request_origin == Some(same_origin.as_str()) {
        return None;
    }

    if let Some(origin) = request_origin {
        if trusted_origins.iter().any(|trusted| trusted == origin) {
            return None;
        }
    }

    let message = format!(
        "Cross-site {} form submissions are forbidden",
        request.method
    );
    let accept = request.header("accept");
    let mut response = ServerResponse::new(403);

    if accept == Some("application/json") {
        response.set_header("content-type", "application/json; charset=utf-8");
        response.body = Some(
            serde_json::to_string(&serde_json::json!({ "message": message }))
                .expect("serialize csrf json response"),
        );
    } else {
        response.set_header("content-type", "text/plain; charset=utf-8");
        response.body = Some(message);
    }

    Some(response)
}

pub fn maybe_not_modified_response(
    request: &ServerRequest,
    response: &ServerResponse,
) -> Option<ServerResponse> {
    if response.status != 200 {
        return None;
    }

    let etag = response.header("etag")?.to_string();
    let mut if_none_match = request.header("if-none-match")?.to_string();
    if let Some(strong) = if_none_match.strip_prefix("W/") {
        if_none_match = strong.to_string();
    }

    if if_none_match != etag {
        return None;
    }

    let mut not_modified = ServerResponse::new(304);
    not_modified.set_header("etag", etag);

    for header in [
        "cache-control",
        "content-location",
        "date",
        "expires",
        "vary",
        "set-cookie",
    ] {
        for value in response.headers.get_all(header) {
            not_modified.headers.append(header, value.clone());
        }
    }

    Some(not_modified)
}

pub fn response_with_vary_accept(response: &ServerResponse) -> ServerResponse {
    let mut updated = response.clone();
    let varies = response
        .header_values("vary")
        .unwrap_or_default()
        .join(", ");

    let includes_accept = varies
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .any(|value| value == "accept" || value == "*");

    if includes_accept {
        return updated;
    }

    if varies.is_empty() {
        updated.set_header("vary", "Accept");
    } else {
        updated.set_header("vary", format!("{varies}, Accept"));
    }

    updated
}

impl ServerResponse {
    pub fn new(status: u16) -> Self {
        Self {
            status: StatusCode::from_u16(status).expect("server response status must be valid"),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    pub fn set_header(&mut self, name: &str, value: impl Into<String>) {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .expect("server response header names must be valid HTTP names");
        let value = value.into();
        let header_value = HeaderValue::from_str(&value)
            .expect("server response header values must be valid HTTP values");
        self.headers.insert(header_name, header_value);
    }

    pub fn append_header(&mut self, name: &str, value: impl Into<String>) {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .expect("server response header names must be valid HTTP names");
        let value = value.into();
        let header_value = HeaderValue::from_str(&value)
            .expect("server response header values must be valid HTTP values");
        self.headers.append(header_name, header_value);
    }
}

pub fn redirect_response(status: u16, location: &str) -> ServerResponse {
    ServerResponse::builder(status)
        .header("location", location)
        .expect("redirect location is valid")
        .build()
        .expect("redirect response is valid")
}

pub fn static_error_page(
    status: u16,
    message: &str,
    render_error: impl FnOnce(u16, &str) -> String,
    dev: bool,
) -> ServerResponse {
    let escaped = escape_html(message);
    let mut page = render_error(status, &escaped);

    if dev {
        page = page.replace(
            "</head>",
            "<script type=\"module\" src=\"/@vite/client\"></script></head>",
        );
    }

    ServerResponse::builder(status)
        .header("content-type", "text/html; charset=utf-8")
        .expect("error page content-type is valid")
        .body(page)
        .build()
        .expect("error page response is valid")
}

pub fn get_global_name(version_hash: &str, dev: bool) -> String {
    if dev {
        "__sveltekit_dev".to_string()
    } else {
        format!("__sveltekit_{version_hash}")
    }
}

pub fn is_pojo(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Object(_) | serde_json::Value::Null
    )
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn generate_route_object(
    route: &ClientRoute,
    request_url: &Url,
    assets: &RouteResolutionAssets,
) -> String {
    let node_entries = route_node_indexes(route)
        .into_iter()
        .filter_map(|index| {
            let import_path = assets.nodes.get(index).and_then(|path| path.as_deref())?;
            Some(format!(
                "'{index}': () => {}",
                create_client_import(Some(import_path), request_url, assets)
            ))
        })
        .collect::<Vec<_>>()
        .join(",\n\t\t");

    let errors = serde_json::to_string(&route.errors).expect("serialize route errors");
    let layouts = serde_json::to_string(
        &route
            .layouts
            .iter()
            .map(|layout| {
                layout
                    .as_ref()
                    .map(|layout| (layout.uses_server_data, layout.node))
            })
            .collect::<Vec<_>>(),
    )
    .expect("serialize route layouts");
    let leaf = serde_json::to_string(&(route.leaf.uses_server_data, route.leaf.node))
        .expect("serialize route leaf");

    format!(
        "{{\n\tid: {},\n\terrors: {},\n\tlayouts: {},\n\tleaf: {},\n\tnodes: {{\n\t\t{}\n\t}}\n}}",
        serde_json::to_string(&route.id).expect("serialize route id"),
        errors,
        layouts,
        leaf,
        node_entries
    )
}

fn create_css_import(
    route: &ClientRoute,
    request_url: &Url,
    assets: &RouteResolutionAssets,
) -> String {
    let mut css = Vec::new();
    for node in route_node_indexes(route) {
        if let Some(node_css) = assets.css.get(&node) {
            for css_path in node_css {
                let prefix = if assets.assets.is_empty() {
                    assets.base.as_str()
                } else {
                    assets.assets.as_str()
                };
                css.push(format!(
                    "'{}{}{}'",
                    prefix,
                    slash_if_needed(prefix),
                    css_path
                ));
            }
        }
    }

    if css.is_empty() {
        return String::new();
    }

    format!(
        "{}.then(x => x.load_css([{}]));",
        create_client_import(assets.start.as_deref(), request_url, assets),
        css.join(",")
    )
}

fn create_client_import(
    import_path: Option<&str>,
    request_url: &Url,
    assets: &RouteResolutionAssets,
) -> String {
    let Some(import_path) = import_path else {
        return "Promise.resolve({})".to_string();
    };

    if import_path.starts_with('/') {
        return format!(
            "import({})",
            serde_json::to_string(import_path).expect("serialize path")
        );
    }

    let resolved = if !assets.assets.is_empty() {
        format!(
            "{}{}{}",
            assets.assets,
            slash_if_needed(&assets.assets),
            import_path
        )
    } else if !assets.relative {
        format!(
            "{}{}{}",
            assets.base,
            slash_if_needed(&assets.base),
            import_path
        )
    } else {
        relative_url_path(
            request_url.path(),
            &format!(
                "{}{}{}",
                assets.base,
                slash_if_needed(&assets.base),
                import_path
            ),
        )
    };

    format!(
        "import({})",
        serde_json::to_string(&resolved).expect("serialize import path")
    )
}

fn route_node_indexes(route: &ClientRoute) -> Vec<usize> {
    route
        .errors
        .iter()
        .flatten()
        .copied()
        .chain(route.layouts.iter().flatten().map(|layout| layout.node))
        .chain(std::iter::once(route.leaf.node))
        .collect()
}

fn relative_url_path(from_request_path: &str, to_path: &str) -> String {
    let from = Utf8Path::new(from_request_path);
    let target = Utf8Path::new(to_path);
    let mut relative = pathdiff::diff_paths(target, from.parent().unwrap_or(from))
        .unwrap_or_else(|| target.to_path_buf().into())
        .to_string_lossy()
        .replace('\\', "/");

    if !relative.starts_with('.') {
        relative.insert_str(0, "./");
    }

    relative
}

fn slash_if_needed(prefix: &str) -> &'static str {
    if prefix.is_empty() || prefix.ends_with('/') {
        ""
    } else {
        "/"
    }
}

fn weak_etag(body: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    format!("W/{:x}", hasher.finish())
}

fn runtime_error_status(error: &RuntimeFatalError) -> u16 {
    match error {
        RuntimeFatalError::Http { status, .. } | RuntimeFatalError::Kit { status, .. } => *status,
        RuntimeFatalError::Other(_) => 500,
    }
}

fn runtime_error_body(error: &RuntimeFatalError) -> Map<String, Value> {
    match error {
        RuntimeFatalError::Http { body, .. } => {
            let mut merged = Map::from_iter([(
                "message".to_string(),
                Value::String("Unknown Error".to_string()),
            )]);
            merged.extend(body.clone());
            merged
        }
        RuntimeFatalError::Kit { text, .. } => {
            Map::from_iter([("message".to_string(), Value::String(text.clone()))])
        }
        RuntimeFatalError::Other(_) => Map::from_iter([(
            "message".to_string(),
            Value::String("Internal Error".to_string()),
        )]),
    }
}
