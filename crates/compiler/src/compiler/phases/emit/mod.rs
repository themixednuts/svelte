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
    if component.options().generate == GenerateTarget::None {
        let warnings = crate::compiler::phases::analyze::collect_compile_warnings(
            component.source_text(),
            component.options(),
            component.root(),
        );
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

    let resolved_css_hash =
        resolve_css_hash(component.source(), component.root(), component.options());
    let warnings = crate::compiler::phases::analyze::collect_compile_warnings(
        component.source_text(),
        component.options(),
        component.root(),
    );
    let js_code = crate::compiler::phases::transform::codegen::compile_component_js_code(
        crate::compiler::phases::transform::codegen::ComponentCodegenContext {
            source: component.source(),
            root: component.root(),
            target: component.options().generate,
            fragments: component.options().fragments,
            runes_mode: component.runes(),
            hmr: component.options().hmr,
            filename: component.options().filename.as_deref(),
            css_hash: resolved_css_hash.as_deref(),
            scoped_element_starts: component.scoped_element_starts(),
        },
    );
    let Some(js_code) = js_code.map(Arc::<str>::from) else {
        // Strict mode for the port: never emit placeholder output for unsupported component shapes.
        return Err(crate::CompileError::unimplemented(
            "component code generation for this template shape",
        ));
    };

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

    let filename = normalized_css_hash_filename(options);

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
            filename: &filename,
            css,
            hash: svelte_hash,
        }));
    }

    let hash_input = if filename == "(unknown)" {
        css
    } else {
        filename.as_str()
    };
    Some(Arc::from(format!("svelte-{}", svelte_hash(hash_input))))
}

fn normalized_css_hash_filename(options: &CompileOptions) -> String {
    let Some(filename) = options.filename.as_deref() else {
        return "(unknown)".to_string();
    };

    let mut normalized = filename.as_str().replace('\\', "/");
    if let Some(root_dir) = effective_root_dir(options) {
        let normalized_root = root_dir.as_str().replace('\\', "/");
        if let Some(stripped) = normalized.strip_prefix(&normalized_root) {
            normalized = stripped.trim_start_matches(['/', '\\']).to_string();
        }
    }

    normalized
}

fn effective_root_dir(options: &CompileOptions) -> Option<camino::Utf8PathBuf> {
    options.root_dir.clone().or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|path| camino::Utf8PathBuf::from_path_buf(path).ok())
    })
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

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::{normalized_css_hash_filename, svelte_hash};
    use crate::api::CompileOptions;

    #[test]
    fn css_hash_filename_defaults_root_dir_to_current_working_directory() {
        let cwd = Utf8PathBuf::from_path_buf(std::env::current_dir().expect("cwd"))
            .expect("cwd should be valid utf-8");
        let filename = cwd.join("src").join("App.svelte");

        let options = CompileOptions {
            filename: Some(filename),
            ..CompileOptions::default()
        };

        assert_eq!(normalized_css_hash_filename(&options), "src/App.svelte");
        assert_eq!(
            svelte_hash(&normalized_css_hash_filename(&options)),
            "1n46o8q"
        );
    }

    #[test]
    fn css_hash_filename_normalizes_separators_when_outside_root() {
        let options = CompileOptions {
            filename: Some(Utf8PathBuf::from("C:\\repo\\pkg\\App.svelte")),
            root_dir: Some(Utf8PathBuf::from("D:\\other")),
            ..CompileOptions::default()
        };

        assert_eq!(
            normalized_css_hash_filename(&options),
            "C:/repo/pkg/App.svelte"
        );
    }
}
