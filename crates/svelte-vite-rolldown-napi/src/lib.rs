#![allow(clippy::needless_pass_by_value)]

use napi::Error;
use napi_derive::napi;
use svelte_vite_rolldown::{
    BundlerTarget, RequestKind, RustCompilerBridge, SvelteBundlerBridge, TransformRequest,
    classify_request_id as classify_bridge_request_id, transform_json as bridge_transform_json,
};

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
}

#[napi(object)]
pub struct JsTransformResult {
    pub code: String,
    pub map_json: Option<String>,
    pub css: Option<String>,
}

#[napi]
pub fn transform_sync(request: JsTransformRequest) -> napi::Result<JsTransformResult> {
    let bridge = RustCompilerBridge;
    let result = bridge
        .transform(to_bridge_request(request))
        .map_err(compile_error_to_napi)?;

    Ok(JsTransformResult {
        code: result.code,
        map_json: result.map_json,
        css: result.css,
    })
}

#[napi]
pub fn transform_json(input_json: String) -> napi::Result<String> {
    let bridge = RustCompilerBridge;
    bridge_transform_json(&bridge, &input_json)
        .map_err(|error| Error::from_reason(format!("{error:?}")))
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
    }
}

fn compile_error_to_napi(error: <RustCompilerBridge as SvelteBundlerBridge>::Error) -> Error {
    Error::from_reason(error.to_string())
}
