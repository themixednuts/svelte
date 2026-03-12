pub(crate) mod codegen;
mod css;
pub(crate) mod output;
pub(crate) mod sourcemap;

use std::sync::Arc;

use crate::api::{
    CompileMetadata, CompileOptions, CompileResult, ModernPrintTarget, OutputArtifact, ParseMode,
    ParseOptions, PrintOptions, PrintedOutput,
};
use crate::ast::Document;
use crate::compiler::phases::component::LoweredComponent;
use crate::error::CompileError;
use crate::printing::{
    print_document, print_modern_attribute, print_modern_comment, print_modern_css,
    print_modern_css_node, print_modern_fragment, print_modern_node, print_modern_options,
    print_modern_root, print_modern_script,
};
use crate::{SourceId, SourceText};

pub(crate) fn print_component(
    ast: &Document,
    options: PrintOptions,
) -> Result<PrintedOutput, CompileError> {
    let code = if options.preserve_whitespace {
        ast.source.clone()
    } else {
        Arc::from(print_document(ast, &options))
    };

    Ok(build_printed_output(ast.source(), code))
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

    Ok(build_printed_output(ast.source(), code))
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

fn build_printed_output(source: &str, code: Arc<str>) -> PrintedOutput {
    let map =
        output::OutputContext::new(SourceText::new(SourceId::new(0), source, None), None, None)
            .build_sparse_sourcemap(&code, "input.svelte", vec![]);
    PrintedOutput { code, map }
}

pub(crate) fn compile_component(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    let lowered = compile_internal_component(source, &options)
        .map_err(|error| error.with_filename(options.filename.as_deref()))?;
    let ast = parse_public_component_ast(source, &options)
        .map_err(|error| error.with_filename(options.filename.as_deref()))?;
    crate::compiler::phases::emit::emit_component(&lowered, Some(ast))
        .map_err(|error| error.with_filename(options.filename.as_deref()))
}

pub(crate) fn compile_module(
    source: &str,
    options: CompileOptions,
) -> Result<CompileResult, CompileError> {
    let source_text = SourceText::new(SourceId::new(0), source, options.filename.as_deref());
    crate::compiler::phases::analyze::validate_module(source_text)
        .map_err(|error| error.with_filename(options.filename.as_deref()))?;
    if !crate::compiler::phases::parse::can_parse_js_program(source) {
        return Err(
            CompileError::internal("failed to parse module source with oxc parser")
                .with_filename(options.filename.as_deref()),
        );
    }

    let js_code =
        codegen::compile_module_js_code(source, options.generate, options.filename.as_deref())
            .map_err(|error| error.with_filename(options.filename.as_deref()))?;
    let output_ctx = output::OutputContext::new(
        source_text,
        options.output_filename.as_deref(),
        options.sourcemap.as_ref(),
    );
    let js_map = output_ctx
        .include_map(options.css_output_filename.as_deref())
        .then(|| output_ctx.build_sparse_sourcemap(&js_code, "module.svelte.js", vec![]));
    let js_map = output_ctx.compose_input_map(js_map);

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

fn compile_internal_component<'a>(
    source: &'a str,
    options: &'a CompileOptions,
) -> Result<LoweredComponent<'a>, CompileError> {
    let parsed_component = crate::compiler::phases::parse::parse_component_for_compile(source)?;
    let component_analysis =
        crate::compiler::phases::analyze::analyze_component(parsed_component, options)?;
    Ok(crate::compiler::phases::lower::lower_component(
        component_analysis,
    ))
}

fn parse_public_component_ast(
    source: &str,
    options: &CompileOptions,
) -> Result<Document, CompileError> {
    crate::compiler::phases::parse::parse_component(
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
    )
}
