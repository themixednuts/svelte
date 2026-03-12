use std::sync::Arc;

use crate::api::{
    CompileMetadata, CompileOptions, CompileResult, CssHashInput, GenerateTarget, OutputArtifact,
};
use crate::ast::{Document, modern::Root};
use crate::compiler::phases::component::LoweredComponent;

pub(crate) fn emit_component(
    component: &LoweredComponent<'_>,
    ast: Option<Document>,
) -> Result<CompileResult, crate::CompileError> {
    let warnings = crate::compiler::phases::analyze::collect_compile_warnings(
        component.source_text(),
        component.options(),
        component.root(),
    );

    if component.options().generate == GenerateTarget::None {
        return Ok(CompileResult {
            // `generate: false` in JS skips template codegen.
            js: OutputArtifact {
                code: Arc::from(""),
                map: None,
                has_global: None,
            },
            css: None,
            warnings,
            metadata: CompileMetadata {
                runes: component.runes(),
            },
            ast,
        });
    }

    let Some(js_code) = crate::compiler::phases::transform::codegen::compile_component_js_code(
        component.source(),
        component.options().generate,
        component.options().fragments,
        component.root(),
        component.runes(),
        component.options().hmr,
        component.options().filename.as_deref(),
    )
    .map(Arc::<str>::from) else {
        // Strict mode for the port: never emit placeholder output for unsupported component shapes.
        return Err(crate::CompileError::unimplemented(
            "component code generation for this template shape",
        ));
    };

    let resolved_css_hash =
        resolve_css_hash(component.source(), component.root(), component.options());

    Ok(
        crate::compiler::phases::transform::output::build_compile_result(
            crate::compiler::phases::transform::output::BuildCompileResultArgs {
                ctx: crate::compiler::phases::transform::output::OutputContext::new(
                    component.source_text(),
                    component.options().output_filename.as_deref(),
                    component.options().sourcemap.as_ref(),
                ),
                root: component.root(),
                ast,
                js_code,
                css_hash: resolved_css_hash.as_deref(),
                dev: component.options().dev,
                runes: component.runes(),
                css_output_filename: component.options().css_output_filename.as_deref(),
                warnings,
            },
        ),
    )
}

fn resolve_css_hash(source: &str, root: &Root, options: &CompileOptions) -> Option<Arc<str>> {
    let (_, _, content_start, content_end) =
        crate::compiler::phases::parse::style_block_ranges(root)
            .into_iter()
            .next()?;
    let css = source.get(content_start..content_end)?;

    if let Some(hash) = &options.css_hash {
        return Some(hash.clone());
    }

    let filename = options
        .filename
        .as_deref()
        .map(|path| path.as_str())
        .unwrap_or("(unknown)");

    if let Some(getter) = &options.css_hash_getter {
        let name = options
            .name
            .as_deref()
            .or_else(|| {
                options
                    .filename
                    .as_deref()
                    .and_then(|path| path.file_stem())
            })
            .unwrap_or("Component");
        return Some(getter.call(CssHashInput {
            name,
            filename,
            css,
            hash: svelte_hash,
        }));
    }

    let hash_input = if filename == "(unknown)" {
        css
    } else {
        filename
    };
    Some(Arc::from(format!("svelte-{}", svelte_hash(hash_input))))
}

fn svelte_hash(input: &str) -> String {
    let normalized = input.replace('\r', "");
    let mut hash = 5381u32;
    for ch in normalized.chars().rev() {
        hash = hash.wrapping_shl(5).wrapping_sub(hash) ^ (ch as u32);
    }
    radix36(hash)
}

fn radix36(mut value: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize] as char);
        value /= 36;
    }
    out.iter().rev().collect()
}
