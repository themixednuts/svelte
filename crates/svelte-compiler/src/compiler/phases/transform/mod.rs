pub(crate) mod codegen;
mod css;
pub(crate) mod output;
pub(crate) mod sourcemap;

use std::sync::Arc;

use camino::Utf8Path;

use crate::api::{
    CompileMetadata, CompileOptions, CompileResult, ModernPrintTarget, OutputArtifact, ParseMode,
    ParseOptions, PrintOptions, PrintedOutput,
};
use crate::ast::Document;
use crate::error::CompileError;
use crate::printing::{
    print_document, print_modern_attribute, print_modern_comment, print_modern_css,
    print_modern_css_node, print_modern_fragment, print_modern_node, print_modern_options,
    print_modern_root, print_modern_script,
};

pub(crate) fn print_component(
    ast: &Document,
    options: PrintOptions,
) -> Result<PrintedOutput, CompileError> {
    let code = if options.preserve_whitespace {
        ast.source.clone()
    } else {
        Arc::from(print_document(ast, &options))
    };

    let map = sourcemap::build_sparse_sourcemap(sourcemap::SparseMappingOptions {
        output: &code,
        output_filename: None,
        sources: vec![sourcemap::SourceMapSource {
            filename: Arc::from("input.svelte"),
            code: ast.source(),
        }],
        hints: vec![],
    });

    Ok(PrintedOutput { code, map })
}

pub(crate) fn print_modern_target(
    ast: ModernPrintTarget<'_>,
    options: PrintOptions,
) -> Result<PrintedOutput, CompileError> {
    let code = if options.preserve_whitespace {
        ast.raw_slice()
            .map(Arc::from)
            .unwrap_or_else(|| Arc::from(render_modern_target(ast, &options)))
    } else {
        Arc::from(render_modern_target(ast, &options))
    };

    let map = sourcemap::build_sparse_sourcemap(sourcemap::SparseMappingOptions {
        output: &code,
        output_filename: None,
        sources: vec![sourcemap::SourceMapSource {
            filename: Arc::from("input.svelte"),
            code: ast.source(),
        }],
        hints: vec![],
    });

    Ok(PrintedOutput { code, map })
}

fn render_modern_target(ast: ModernPrintTarget<'_>, options: &PrintOptions) -> String {
    match ast {
        ModernPrintTarget::Root { source, root } => print_modern_root(source, root, options),
        ModernPrintTarget::Fragment { source, fragment } => print_modern_fragment(source, fragment),
        ModernPrintTarget::Node { source, node } => print_modern_node(source, node),
        ModernPrintTarget::Script { source, script } => {
            print_modern_script(source, script, options)
        }
        ModernPrintTarget::Css { source, stylesheet } => print_modern_css(source, stylesheet),
        ModernPrintTarget::CssNode { node, .. } => print_modern_css_node(node),
        ModernPrintTarget::Attribute { source, attribute } => {
            print_modern_attribute(source, attribute)
        }
        ModernPrintTarget::Options { source, options } => print_modern_options(source, options),
        ModernPrintTarget::Comment { comment, .. } => print_modern_comment(comment),
    }
}

pub(crate) fn compile_component(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    let parsed_component = crate::compiler::phases::parse::parse_component_for_compile(source)?;
    let ast = crate::compiler::phases::parse::parse_component(
        source,
        ParseOptions {
            mode: if options.modern_ast {
                ParseMode::Modern
            } else {
                ParseMode::Legacy
            },
            loose: false,
            ..Default::default()
        },
    )?;
    let component_analysis =
        crate::compiler::phases::analyze::analyze_component(&parsed_component, &options)?;
    let ir = crate::compiler::phases::lower::lower_component(&component_analysis);
    crate::compiler::phases::emit::emit_component(
        &ir,
        &options,
        Some(ast),
        crate::api::infer_runes_mode(&options, parsed_component.root()),
    )
}

pub(crate) fn compile_module(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    let normalized_source = source.replace('\r', "");
    let source = normalized_source.as_str();

    crate::compiler::phases::analyze::validate_module(source)?;
    if !crate::compiler::phases::parse::can_parse_js_program(source) {
        return Err(CompileError::internal(
            "failed to parse module source with oxc parser",
        ));
    }

    let js_code =
        codegen::compile_module_js_code(source, options.generate, options.filename.as_deref())?;
    let include_map = options.sourcemap.is_some()
        || options.output_filename.is_some()
        || options.css_output_filename.is_some();
    let js_map = include_map.then(|| {
        sourcemap::build_sparse_sourcemap(sourcemap::SparseMappingOptions {
            output: &js_code,
            output_filename: options.output_filename.as_deref(),
            sources: vec![sourcemap::SourceMapSource {
                filename: Arc::from(
                    options
                        .filename
                        .as_deref()
                        .map(Utf8Path::as_str)
                        .unwrap_or("module.svelte.js"),
                ),
                code: source,
            }],
            hints: vec![],
        })
    });
    let js_map = match (js_map, options.sourcemap.as_ref()) {
        (Some(map), Some(input))
            if !input.mappings.is_empty()
                || !input.sources.is_empty()
                || !input.names.is_empty() =>
        {
            Some(sourcemap::compose_sourcemaps(&map, input))
        }
        (map, _) => map,
    };

    Ok(CompileResult {
        js: OutputArtifact {
            code: Arc::from(js_code),
            map: js_map,
            has_global: None,
        },
        css: None,
        warnings: Box::new([]),
        metadata: CompileMetadata { runes: true },
        ast: None,
    })
}
