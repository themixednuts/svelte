//! Native Rust-facing bridge surface for Vite/Rolldown integration.
//!
//! This crate is intentionally small for now. It defines a stable internal API
//! for driving Svelte transforms from bundlers while we continue compiler phase
//! alignment and AST parity work in `svelte-compiler`.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum BridgeError {
    InvalidRequest(String),
    Transform(svelte_compiler::CompileError),
    InvalidResponse(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BundlerTarget {
    Vite,
    Rolldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequestKind {
    SvelteComponent,
    SvelteModule,
    VirtualCss,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformRequest {
    pub id: String,
    pub code: String,
    pub ssr: bool,
    pub hmr: bool,
    pub target: BundlerTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformResult {
    pub code: String,
    pub map_json: Option<String>,
    pub css: Option<String>,
}

pub trait SvelteBundlerBridge {
    type Error;

    fn transform(&self, request: TransformRequest) -> Result<TransformResult, Self::Error>;
}

pub fn transform_json(
    bridge: &RustCompilerBridge,
    input_json: &str,
) -> Result<String, BridgeError> {
    let request = serde_json::from_str::<TransformRequest>(input_json)
        .map_err(|error| BridgeError::InvalidRequest(error.to_string()))?;
    let result = bridge.transform(request).map_err(BridgeError::Transform)?;
    serde_json::to_string(&result).map_err(|error| BridgeError::InvalidResponse(error.to_string()))
}

pub fn classify_request_id(id: &str) -> RequestKind {
    let (path, query) = split_id_query(id);
    let path_lower = path.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();

    if (path_lower.contains(".svelte") || query_lower.contains("svelte"))
        && query_lower.contains("type=style")
    {
        return RequestKind::VirtualCss;
    }

    if path_lower.ends_with(".svelte.js") {
        return RequestKind::SvelteModule;
    }

    if path_lower.ends_with(".svelte") {
        return RequestKind::SvelteComponent;
    }

    RequestKind::Unknown
}

pub fn should_transform_id(id: &str) -> bool {
    matches!(
        classify_request_id(id),
        RequestKind::SvelteComponent | RequestKind::SvelteModule
    )
}

fn split_id_query(id: &str) -> (&str, &str) {
    if let Some(index) = id.find('?') {
        let path = &id[..index];
        let query = id.get(index + 1..).unwrap_or_default();
        (path, query)
    } else {
        (id, "")
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RustCompilerBridge;

impl SvelteBundlerBridge for RustCompilerBridge {
    type Error = svelte_compiler::CompileError;

    fn transform(&self, request: TransformRequest) -> Result<TransformResult, Self::Error> {
        let kind = classify_request_id(&request.id);

        if matches!(kind, RequestKind::VirtualCss) {
            return Ok(TransformResult {
                code: request.code,
                map_json: None,
                css: None,
            });
        }

        if matches!(kind, RequestKind::Unknown) {
            return Err(svelte_compiler::CompileError::unimplemented(
                "bundler transform for unknown request kind",
            ));
        }

        let options = compile_options_for_request(&request);

        let result = match kind {
            RequestKind::SvelteComponent => svelte_compiler::compile(&request.code, options)?,
            RequestKind::SvelteModule => svelte_compiler::compile_module(&request.code, options)?,
            RequestKind::VirtualCss | RequestKind::Unknown => unreachable!("handled above"),
        };
        let map_json = result
            .js
            .map
            .as_ref()
            .and_then(|map| serde_json::to_string(map).ok());

        Ok(TransformResult {
            code: result.js.code.to_string(),
            map_json,
            css: result.css.map(|artifact| artifact.code.to_string()),
        })
    }
}

fn request_path(id: &str) -> Option<&str> {
    let (path, _) = split_id_query(id);
    (!path.is_empty()).then_some(path)
}

fn compile_options_for_request(request: &TransformRequest) -> svelte_compiler::CompileOptions {
    let mut options = svelte_compiler::CompileOptions::default();
    options.generate = if request.ssr {
        svelte_compiler::GenerateTarget::Server
    } else {
        svelte_compiler::GenerateTarget::Client
    };
    options.hmr = request.hmr;
    options.filename = request_path(&request.id).map(Utf8PathBuf::from);
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_svelte_component_request() {
        assert_eq!(
            classify_request_id("/src/App.svelte"),
            RequestKind::SvelteComponent
        );
    }

    #[test]
    fn classifies_svelte_module_request() {
        assert_eq!(
            classify_request_id("/src/module.svelte.js"),
            RequestKind::SvelteModule
        );
    }

    #[test]
    fn classifies_virtual_css_request() {
        assert_eq!(
            classify_request_id("/src/App.svelte?direct&type=style&lang.css"),
            RequestKind::VirtualCss
        );
    }

    #[test]
    fn bridge_compiles_simple_component() {
        let bridge = RustCompilerBridge;
        let result = bridge
            .transform(TransformRequest {
                id: "/src/App.svelte".to_string(),
                code: "<h1>Hello</h1>".to_string(),
                ssr: false,
                hmr: false,
                target: BundlerTarget::Vite,
            })
            .expect("bridge transform should compile simple component");

        assert!(result.code.contains("export default"));
    }

    #[test]
    fn bridge_compiles_module_request() {
        let bridge = RustCompilerBridge;
        let result = bridge
            .transform(TransformRequest {
                id: "/src/module.svelte.js".to_string(),
                code: "export const answer = 42;".to_string(),
                ssr: false,
                hmr: false,
                target: BundlerTarget::Vite,
            })
            .expect("bridge transform should compile module request");

        assert!(result.code.contains("generated by Svelte VERSION"));
    }

    #[test]
    fn bridge_passthroughs_virtual_css() {
        let bridge = RustCompilerBridge;
        let result = bridge
            .transform(TransformRequest {
                id: "/src/App.svelte?direct&type=style&lang.css".to_string(),
                code: "h1 { color: red; }".to_string(),
                ssr: false,
                hmr: false,
                target: BundlerTarget::Vite,
            })
            .expect("bridge transform should passthrough virtual css");

        assert_eq!(result.code, "h1 { color: red; }");
        assert!(result.css.is_none());
    }

    #[test]
    fn transform_json_round_trips_request_response() {
        let bridge = RustCompilerBridge;
        let input = r#"{"id":"/src/App.svelte","code":"<h1>Hello</h1>","ssr":false,"hmr":false,"target":"vite"}"#;
        let output = transform_json(&bridge, input).expect("json transform should succeed");
        let parsed = serde_json::from_str::<TransformResult>(&output)
            .expect("output should deserialize to TransformResult");
        assert!(parsed.code.contains("export default"));
    }

    #[test]
    fn transform_json_rejects_invalid_request_payload() {
        let bridge = RustCompilerBridge;
        let error = transform_json(&bridge, "{not json}").expect_err("invalid payload should fail");
        match error {
            BridgeError::InvalidRequest(_) => {}
            other => panic!("expected InvalidRequest error, got: {other:?}"),
        }
    }

    #[test]
    fn bridge_rejects_unknown_request_kind() {
        let bridge = RustCompilerBridge;
        let error = bridge
            .transform(TransformRequest {
                id: "/src/entry.ts".to_string(),
                code: "export const x = 1;".to_string(),
                ssr: false,
                hmr: false,
                target: BundlerTarget::Vite,
            })
            .expect_err("unknown ids should be rejected");
        assert_eq!(error.code.as_ref(), "unimplemented");
    }
}
