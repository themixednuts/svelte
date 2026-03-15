#![allow(clippy::needless_pass_by_value)]

use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::FunctionRef;
use napi::{Env, Error};
use napi_derive::napi;
use svelte_vite_rolldown::{
    BridgeError, BundlerTarget, RequestKind, RustCompilerBridge, SvelteBundlerBridge,
    TransformRequest, classify_request_id as classify_bridge_request_id,
    compile_options_for_request, transform_json as bridge_transform_json, transform_with_options,
};

const STRUCTURED_ERROR_PREFIX: &str = "__SVELTE_NATIVE_ERROR__";

#[napi(string_enum)]
pub enum JsBundlerTarget {
    Vite,
    Rolldown,
}

impl From<JsBundlerTarget> for BundlerTarget {
    fn from(value: JsBundlerTarget) -> Self {
        match value {
            JsBundlerTarget::Vite => BundlerTarget::Vite,
            JsBundlerTarget::Rolldown => BundlerTarget::Rolldown,
        }
    }
}

#[napi(object)]
pub struct JsTransformRequest {
    pub id: String,
    pub code: String,
    pub ssr: Option<bool>,
    pub hmr: Option<bool>,
    pub target: Option<JsBundlerTarget>,
    pub request_kind: Option<String>,
    pub compiler_options_json: Option<String>,
}

#[napi(object)]
pub struct JsTransformResult {
    pub code: String,
    pub map_json: Option<String>,
    pub css: Option<String>,
    pub css_map_json: Option<String>,
    pub css_has_global: Option<bool>,
    pub warnings_json: Option<String>,
}

#[napi]
pub fn transform_sync(request: JsTransformRequest) -> napi::Result<JsTransformResult> {
    let bridge = RustCompilerBridge;
    let result = bridge
        .transform(to_bridge_request(request))
        .map_err(compile_error_to_napi)?;

    Ok(js_transform_result(result))
}

#[napi]
pub fn transform_sync_with_callbacks(
    env: Env,
    request: JsTransformRequest,
    css_hash_callback: Option<FunctionRef<(String,), String>>,
    warning_filter_callback: Option<FunctionRef<(String,), bool>>,
) -> napi::Result<JsTransformResult> {
    let request = to_bridge_request(request);
    let mut options = compile_options_for_request(&request).map_err(compile_error_to_napi)?;
    let callback_error = Arc::new(Mutex::new(None));

    if let Some(callback) = css_hash_callback {
        let callback_error_ref = Arc::clone(&callback_error);
        options.css_hash_getter = Some(svelte_compiler::CssHashGetterCallback::new(move |input| {
            let payload = serde_json::json!({
                "name": input.name,
                "filename": input.filename,
                "css": input.css,
                "hashInput": if input.filename == "(unknown)" { input.css } else { input.filename },
            });

            match serde_json::to_string(&payload)
                .map_err(|error| {
                    Error::from_reason(format!("failed to serialize cssHash payload: {error}"))
                })
                .and_then(|payload| call_js_function(&env, &callback, payload))
            {
                Ok(result) => std::sync::Arc::from(result),
                Err(error) => {
                    store_callback_error(&callback_error_ref, error);
                    std::sync::Arc::from("svelte-callback-error")
                }
            }
        }));
    }

    if let Some(callback) = warning_filter_callback {
        let callback_error_ref = Arc::clone(&callback_error);
        options.warning_filter = Some(svelte_compiler::WarningFilterCallback::new(
            move |warning| {
                let payload = match serde_json::to_string(warning) {
                    Ok(payload) => payload,
                    Err(error) => {
                        store_callback_error(
                            &callback_error_ref,
                            Error::from_reason(format!(
                                "failed to serialize warningFilter payload: {error}"
                            )),
                        );
                        return true;
                    }
                };

                match call_js_function(&env, &callback, payload) {
                    Ok(result) => result,
                    Err(error) => {
                        store_callback_error(&callback_error_ref, error);
                        true
                    }
                }
            },
        ));
    }

    let result = transform_with_options(&request, options).map_err(compile_error_to_napi)?;
    if let Some(error) = take_callback_error(&callback_error) {
        return Err(error);
    }
    Ok(js_transform_result(result))
}

#[napi]
pub fn transform_json(input_json: String) -> napi::Result<String> {
    let bridge = RustCompilerBridge;
    bridge_transform_json(&bridge, &input_json).map_err(bridge_error_to_napi)
}

#[napi]
pub fn classify_request_id(id: String) -> String {
    match classify_bridge_request_id(&id) {
        RequestKind::SvelteComponent => "svelte-component",
        RequestKind::SvelteModule => "svelte-module",
        RequestKind::VirtualCss => "virtual-css",
        RequestKind::Unknown => "unknown",
    }
    .to_string()
}

fn to_bridge_request(request: JsTransformRequest) -> TransformRequest {
    TransformRequest {
        id: request.id,
        code: request.code,
        ssr: request.ssr.unwrap_or(false),
        hmr: request.hmr.unwrap_or(false),
        target: request
            .target
            .map(Into::into)
            .unwrap_or(BundlerTarget::Vite),
        request_kind: request.request_kind.as_deref().and_then(parse_request_kind),
        compiler_options_json: request.compiler_options_json,
    }
}

fn parse_request_kind(value: &str) -> Option<RequestKind> {
    match value {
        "svelte-component" => Some(RequestKind::SvelteComponent),
        "svelte-module" => Some(RequestKind::SvelteModule),
        "virtual-css" => Some(RequestKind::VirtualCss),
        "unknown" => Some(RequestKind::Unknown),
        _ => None,
    }
}

fn js_transform_result(result: svelte_vite_rolldown::TransformResult) -> JsTransformResult {
    JsTransformResult {
        code: result.code,
        map_json: result.map_json,
        css: result.css,
        css_map_json: result.css_map_json,
        css_has_global: result.css_has_global,
        warnings_json: result.warnings_json,
    }
}

fn call_js_function<Return>(
    env: &Env,
    callback: &FunctionRef<(String,), Return>,
    payload: String,
) -> napi::Result<Return>
where
    Return: napi::bindgen_prelude::FromNapiValue,
{
    callback.borrow_back(env)?.call((payload,))
}

fn store_callback_error(target: &Arc<Mutex<Option<Error>>>, error: Error) {
    if let Ok(mut guard) = target.lock()
        && guard.is_none()
    {
        *guard = Some(error);
    }
}

fn take_callback_error(target: &Arc<Mutex<Option<Error>>>) -> Option<Error> {
    target.lock().ok().and_then(|mut guard| guard.take())
}

fn compile_error_to_napi(error: <RustCompilerBridge as SvelteBundlerBridge>::Error) -> Error {
    Error::from_reason(structured_error_reason(
        &error.code,
        &error.message,
        error.filename.as_deref().map(|path| path.as_str()),
        error.start.as_deref(),
        error.end.as_deref(),
        error.position.as_deref(),
    ))
}

fn bridge_error_to_napi(error: BridgeError) -> Error {
    match error {
        BridgeError::Transform(inner) => compile_error_to_napi(inner),
        BridgeError::InvalidRequest(message) => Error::from_reason(structured_error_reason(
            "invalid_request",
            &message,
            None,
            None,
            None,
            None,
        )),
        BridgeError::InvalidResponse(message) => Error::from_reason(structured_error_reason(
            "invalid_response",
            &message,
            None,
            None,
            None,
            None,
        )),
    }
}

fn structured_error_reason(
    code: &str,
    message: &str,
    filename: Option<&str>,
    start: Option<&svelte_compiler::SourceLocation>,
    end: Option<&svelte_compiler::SourceLocation>,
    position: Option<&svelte_compiler::SourcePosition>,
) -> String {
    let payload = serde_json::json!({
        "code": code,
        "message": message,
        "filename": filename,
        "start": start,
        "end": end,
        "position": position,
    });
    format!("{STRUCTURED_ERROR_PREFIX}{payload}")
}
