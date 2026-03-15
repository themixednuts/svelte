mod dynamic_markup;
mod generic_client;
mod generic_renderer;
mod generic_server;
mod static_markup;

use std::collections::BTreeSet;

use crate::api::{FragmentStrategy, GenerateTarget};
use crate::ast::modern::Root;
use crate::compiler::phases::parse::ParsedModuleProgram;
use crate::error::CompileError;
use crate::js::codegen_options;
use camino::Utf8Path;
use oxc_ast::ast::{
    AssignmentTarget, Declaration, Expression as OxcExpression, Statement,
};
use oxc_ast_visit::Visit;
use oxc_codegen::{Codegen, Context, Gen};
use oxc_span::{GetSpan, Span};

pub(crate) use dynamic_markup::compile_dynamic_markup_js;
pub(crate) use generic_client::compile_generic_client_markup_js;
pub(crate) use generic_server::compile_generic_server_markup_js;
pub(crate) use static_markup::compile_static_markup_js;

// ---------------------------------------------------------------------------
// Shared codegen helpers
// ---------------------------------------------------------------------------

/// A source-range replacement: replace bytes `start..end` with `text`.
#[derive(Debug)]
pub(super) struct SourceReplacement {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl SourceReplacement {
    pub fn new(start: usize, end: usize, text: String) -> Self {
        Self { start, end, text }
    }

    /// Build from an OXC `Span` (u32 offsets → usize).
    pub fn from_span(span: Span, text: String) -> Self {
        Self { start: span.start as usize, end: span.end as usize, text }
    }

    /// Deletion: replace a span with nothing.
    pub fn delete(span: Span) -> Self {
        Self::from_span(span, String::new())
    }
}

/// Create an OXC `Codegen` pre-configured with our standard options and source text.
pub(super) fn oxc_codegen_for<'a>(source: &'a str) -> Codegen<'a> {
    Codegen::new()
        .with_options(codegen_options())
        .with_source_text(source)
}

const TEMPLATE_MODULE_CLIENT: &str = include_str!("codegen/templates/module.client.js");
const TEMPLATE_MODULE_SERVER: &str = include_str!("codegen/templates/module.server.js");

#[derive(Debug, Clone, Copy)]
pub(crate) struct ComponentCodegenContext<'a> {
    pub(crate) source: &'a str,
    pub(crate) root: &'a Root,
    pub(crate) target: GenerateTarget,
    pub(crate) fragments: FragmentStrategy,
    pub(crate) runes_mode: bool,
    pub(crate) hmr: bool,
    pub(crate) filename: Option<&'a Utf8Path>,
    pub(crate) css_hash: Option<&'a str>,
    pub(crate) scoped_element_starts: &'a [usize],
}

pub(crate) fn compile_component_js_code(ctx: ComponentCodegenContext<'_>) -> Option<String> {
    if let Some(output) = compile_static_markup_js(ctx) {
        return Some(output);
    }
    if ctx.scoped_element_starts.is_empty()
        && let Some(output) = compile_dynamic_markup_js(&ctx)
    {
        return Some(output);
    }
    if ctx.target == GenerateTarget::Client {
        return compile_generic_client_markup_js(ctx.source, ctx.root, ctx.filename);
    }
    if ctx.target == GenerateTarget::Server {
        return compile_generic_server_markup_js(ctx.source, ctx.root, ctx.filename);
    }
    None
}

pub(crate) fn compile_module_js_code(
    parsed: &ParsedModuleProgram<'_>,
    target: GenerateTarget,
) -> Result<String, CompileError> {
    let transformed_body = compile_module_body_from_ast(parsed, target)?;
    let transformed_body = transformed_body.trim_end();

    if target == GenerateTarget::None {
        return Ok(format!("{transformed_body}\n"));
    }

    let basename = parsed
        .source_text()
        .filename
        .and_then(Utf8Path::file_name)
        .unwrap_or("module.svelte.js");
    let import_gap = if starts_with_import_statement(transformed_body.trim_start()) {
        ""
    } else {
        "\n"
    };

    let template = match target {
        GenerateTarget::Client => TEMPLATE_MODULE_CLIENT,
        GenerateTarget::Server => TEMPLATE_MODULE_SERVER,
        GenerateTarget::None => unreachable!("GenerateTarget::None handled before template path"),
    };
    Ok(render_module_template(
        template,
        basename,
        import_gap,
        transformed_body,
    ))
}

fn compile_module_body_from_ast(
    parsed: &ParsedModuleProgram<'_>,
    target: GenerateTarget,
) -> Result<String, CompileError> {
    let source = parsed.source_text().text;
    let state_bindings = collect_module_state_bindings(parsed);
    let mut output = String::new();
    let mut previous_group = None;

    for statement in &parsed.program().program().body {
        let Some(rendered) =
            render_module_statement_from_oxc(parsed, statement, source, target, &state_bindings)?
        else {
            continue;
        };
        let group = statement_group_oxc(statement);
        if let Some(previous_group) = previous_group {
            if previous_group != group {
                output.push_str("\n\n");
            } else {
                output.push('\n');
            }
        }
        output.push_str(rendered.trim_end());
        previous_group = Some(group);
    }

    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleStatementGroup {
    Import,
    Declaration,
    FunctionOrExport,
}

fn statement_group_oxc(statement: &Statement<'_>) -> ModuleStatementGroup {
    match statement {
        Statement::ImportDeclaration(_) => ModuleStatementGroup::Import,
        Statement::VariableDeclaration(_) => ModuleStatementGroup::Declaration,
        Statement::ExportNamedDeclaration(export) => match export.declaration.as_ref() {
            Some(Declaration::VariableDeclaration(_)) => ModuleStatementGroup::Declaration,
            _ => ModuleStatementGroup::FunctionOrExport,
        },
        _ => ModuleStatementGroup::FunctionOrExport,
    }
}

fn render_module_statement_from_oxc(
    parsed: &ParsedModuleProgram<'_>,
    statement: &Statement<'_>,
    source: &str,
    target: GenerateTarget,
    state_bindings: &BTreeSet<String>,
) -> Result<Option<String>, CompileError> {
    if statement_is_typescript_only(statement) {
        return Ok(None);
    }

    if let Some(rendered) = render_state_declaration_statement(source, statement, target)? {
        return Ok(Some(rendered));
    }

    if target == GenerateTarget::Client
        && let Some(rendered) =
            render_function_statement_with_state_rewrites(source, statement, state_bindings)?
    {
        return Ok(Some(rendered));
    }

    if target == GenerateTarget::Client {
        if let Some(rendered) = render_effect_statement(source, statement)? {
            return Ok(Some(rendered));
        }
    } else if target == GenerateTarget::Server && is_effect_statement(statement) {
        return Ok(None);
    }

    if parsed.language().is_typescript() && target != GenerateTarget::None {
        return Ok(render_typescript_statement_source(source, statement));
    }

    Ok(Some(render_statement_with_codegen(source, statement)))
}

fn render_statement_with_codegen(source: &str, statement: &Statement<'_>) -> String {
    let mut codegen = oxc_codegen_for(source);
    statement.print(&mut codegen, Context::default());
    codegen.into_source_text()
}

fn collect_module_state_bindings(parsed: &ParsedModuleProgram<'_>) -> BTreeSet<String> {
    let mut bindings = BTreeSet::new();
    for statement in &parsed.program().program().body {
        let declaration = match statement {
            Statement::VariableDeclaration(declaration) => Some(declaration),
            Statement::ExportNamedDeclaration(export) => match export.declaration.as_ref() {
                Some(Declaration::VariableDeclaration(declaration)) => Some(declaration),
                _ => None,
            },
            _ => None,
        };
        let Some(declaration) = declaration else {
            continue;
        };
        for declarator in &declaration.declarations {
            let Some(identifier) = declarator.id.get_binding_identifier() else {
                continue;
            };
            let Some(init) = declarator.init.as_ref() else {
                continue;
            };
            if oxc_state_call_argument(init).is_some() {
                bindings.insert(identifier.name.to_string());
            }
        }
    }
    bindings
}

pub(super) fn render_state_declaration_statement(
    source: &str,
    statement: &Statement<'_>,
    target: GenerateTarget,
) -> Result<Option<String>, CompileError> {
    let declaration = match statement {
        Statement::VariableDeclaration(declaration) => Some((false, declaration)),
        Statement::ExportNamedDeclaration(export) => match export.declaration.as_ref() {
            Some(Declaration::VariableDeclaration(declaration)) => Some((true, declaration)),
            _ => None,
        },
        _ => None,
    };
    let Some((exported, declaration)) = declaration else {
        return Ok(None);
    };

    let mut replacements = Vec::new();
    for declarator in &declaration.declarations {
        let Some(init) = declarator.init.as_ref() else {
            continue;
        };
        let Some(argument) = oxc_state_call_argument(init) else {
            continue;
        };
        let replacement = match target {
            GenerateTarget::Client => {
                let helper = if argument.is_proxy_like() {
                    "$.proxy"
                } else {
                    "$.state"
                };
                format!("{helper}({})", argument.render(source)?)
            }
            GenerateTarget::Server => argument.render(source)?,
            GenerateTarget::None => continue,
        };
        replacements.push(SourceReplacement::from_span(init.span(), replacement));
    }

    if replacements.is_empty() {
        return Ok(None);
    }

    let declaration_span = declaration.span();
    let mut rendered = replace_source_ranges(
        source,
        declaration_span,
        replacements,
    )?;
    if exported {
        rendered = format!("export {rendered}");
    }
    Ok(Some(rendered))
}

fn render_function_statement_with_state_rewrites(
    source: &str,
    statement: &Statement<'_>,
    state_bindings: &BTreeSet<String>,
) -> Result<Option<String>, CompileError> {
    let (exported, function) = match statement {
        Statement::FunctionDeclaration(function) => (false, function),
        Statement::ExportNamedDeclaration(export) => match export.declaration.as_ref() {
            Some(Declaration::FunctionDeclaration(function)) => (true, function),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };

    let Some(body) = function.body.as_ref() else {
        return Ok(None);
    };

    let mut replacements = Vec::new();
    for statement in &body.statements {
        let Some(replacement) =
            render_client_state_assignment_statement_oxc(source, statement, state_bindings)?
        else {
            continue;
        };
        replacements.push(SourceReplacement::from_span(statement.span(), replacement));
    }

    if replacements.is_empty() {
        return Ok(None);
    }

    let mut rendered = replace_source_ranges(
        source,
        function.span(),
        replacements,
    )?;
    if exported {
        rendered = format!("export {rendered}");
    }
    Ok(Some(rendered))
}

fn render_client_state_assignment_statement_oxc(
    source: &str,
    statement: &Statement<'_>,
    state_bindings: &BTreeSet<String>,
) -> Result<Option<String>, CompileError> {
    let Statement::ExpressionStatement(statement) = statement else {
        return Ok(None);
    };
    let OxcExpression::AssignmentExpression(assignment) =
        statement.expression.get_inner_expression()
    else {
        return Ok(None);
    };
    let AssignmentTarget::ArrayAssignmentTarget(pattern) = &assignment.left else {
        return Ok(None);
    };

    let mut bindings = Vec::new();
    for element in pattern.elements.iter().flatten() {
        let Some(identifier) = element.identifier() else {
            return Ok(None);
        };
        if !state_bindings.contains(identifier.name.as_str()) {
            return Ok(None);
        }
        bindings.push(identifier.name.to_string());
    }
    if bindings.is_empty() {
        return Ok(None);
    }

    let argument = expression_source_from_oxc_span(source, assignment.right.span())?;
    let mut output = String::new();
    output.push_str(&format!(
        "(({argument}) => {{\n\t\tvar $$array = $.to_array({argument}, {});\n\n",
        bindings.len()
    ));
    for (index, binding) in bindings.iter().enumerate() {
        output.push_str(&format!("\t\t$.set({binding}, $$array[{index}], true);\n"));
    }
    output.push_str(&format!("\t}})({argument});\n"));
    Ok(Some(output))
}

fn render_typescript_statement_source(
    source: &str,
    statement: &Statement<'_>,
) -> Option<String> {
    if statement_is_typescript_only(statement) {
        return None;
    }

    let statement_span = statement.span();
    let mut replacements = collect_typescript_strip_ranges(statement);

    if let Some((start, end)) = single_identifier_arrow_parameter_parens(statement) {
        replacements.push(SourceReplacement::new(start as usize, start as usize + 1, String::new()));
        replacements.push(SourceReplacement::new(end as usize - 1, end as usize, String::new()));
    }

    replace_source_ranges(source, statement_span, replacements).ok()
}

fn statement_is_typescript_only(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::TSImportEqualsDeclaration(_)
        | Statement::TSExportAssignment(_)
        | Statement::TSNamespaceExportDeclaration(_)
        | Statement::TSEnumDeclaration(_)
        | Statement::TSInterfaceDeclaration(_)
        | Statement::TSModuleDeclaration(_)
        | Statement::TSTypeAliasDeclaration(_) => true,
        Statement::ExportNamedDeclaration(export) => matches!(
            export.declaration.as_ref(),
            Some(
                Declaration::TSImportEqualsDeclaration(_)
                    | Declaration::TSEnumDeclaration(_)
                    | Declaration::TSInterfaceDeclaration(_)
                    | Declaration::TSModuleDeclaration(_)
                    | Declaration::TSTypeAliasDeclaration(_)
            )
        ),
        _ => false,
    }
}

fn collect_typescript_strip_ranges(statement: &Statement<'_>) -> Vec<SourceReplacement> {
    #[derive(Default)]
    struct TypeStripVisitor {
        ranges: Vec<SourceReplacement>,
    }

    impl<'a> Visit<'a> for TypeStripVisitor {
        fn visit_ts_type_annotation(&mut self, annotation: &oxc_ast::ast::TSTypeAnnotation<'a>) {
            self.ranges.push(SourceReplacement::delete(annotation.span));
        }

        fn visit_ts_type_parameter_declaration(
            &mut self,
            declaration: &oxc_ast::ast::TSTypeParameterDeclaration<'a>,
        ) {
            self.ranges.push(SourceReplacement::delete(declaration.span));
        }

        fn visit_ts_type_parameter_instantiation(
            &mut self,
            instantiation: &oxc_ast::ast::TSTypeParameterInstantiation<'a>,
        ) {
            self.ranges.push(SourceReplacement::delete(instantiation.span));
        }
    }

    let mut visitor = TypeStripVisitor::default();
    visitor.visit_statement(statement);
    visitor.ranges
}

fn single_identifier_arrow_parameter_parens(statement: &Statement<'_>) -> Option<(u32, u32)> {
    let declaration = match statement {
        Statement::VariableDeclaration(declaration) => declaration,
        Statement::ExportNamedDeclaration(export) => match export.declaration.as_ref() {
            Some(Declaration::VariableDeclaration(declaration)) => declaration,
            _ => return None,
        },
        _ => return None,
    };
    if declaration.declarations.len() != 1 {
        return None;
    }
    let init = declaration.declarations.first()?.init.as_ref()?;
    let OxcExpression::ArrowFunctionExpression(arrow) = init.get_inner_expression() else {
        return None;
    };
    if arrow.params.items.len() != 1 || arrow.params.rest.is_some() {
        return None;
    }
    let parameter = arrow.params.items.first()?;
    let oxc_ast::ast::BindingPattern::BindingIdentifier(_) = &parameter.pattern else {
        return None;
    };
    Some((parameter.span.start, parameter.span.end))
}

#[derive(Clone, Copy)]
pub(super) enum OxcStateArgument<'a> {
    ProxyLike(&'a OxcExpression<'a>),
    Other(&'a OxcExpression<'a>),
}

impl<'a> OxcStateArgument<'a> {
    fn is_proxy_like(self) -> bool {
        matches!(self, Self::ProxyLike(_))
    }

    fn render(self, source: &str) -> Result<String, CompileError> {
        match self {
            Self::ProxyLike(expression) | Self::Other(expression) => {
                let mut codegen = oxc_codegen_for(source);
                codegen.print_expression(expression);
                let text = codegen.into_source_text();
                // OXC wraps standalone object/sequence expressions in parens;
                // strip them since we embed in a function call context.
                Ok(strip_outer_parens(&text))
            }
        }
    }
}

pub(super) fn oxc_state_call_argument<'a>(expression: &'a OxcExpression<'a>) -> Option<OxcStateArgument<'a>> {
    let OxcExpression::CallExpression(call) = expression.get_inner_expression() else {
        return None;
    };
    let OxcExpression::Identifier(callee) = call.callee.get_inner_expression() else {
        return None;
    };
    if callee.name.as_str() != "$state" {
        return None;
    }
    let argument = call.arguments.first()?.as_expression()?;
    match argument.get_inner_expression() {
        OxcExpression::ObjectExpression(_) | OxcExpression::ArrayExpression(_) => {
            Some(OxcStateArgument::ProxyLike(argument))
        }
        _ => Some(OxcStateArgument::Other(argument)),
    }
}

/// Detect `$effect(...)` or `$effect.pre(...)` expression statements.
fn effect_call_callee_span(statement: &Statement<'_>) -> Option<(Span, &'static str)> {
    let Statement::ExpressionStatement(expr_stmt) = statement else {
        return None;
    };
    let OxcExpression::CallExpression(call) = expr_stmt.expression.get_inner_expression() else {
        return None;
    };
    match call.callee.get_inner_expression() {
        OxcExpression::Identifier(id) if id.name.as_str() == "$effect" => {
            Some((id.span, "$.user_effect"))
        }
        OxcExpression::StaticMemberExpression(member) => {
            if let OxcExpression::Identifier(obj) = &member.object
                && obj.name.as_str() == "$effect"
                && member.property.name.as_str() == "pre"
            {
                return Some((member.span, "$.user_pre_effect"));
            }
            None
        }
        _ => None,
    }
}

fn is_effect_statement(statement: &Statement<'_>) -> bool {
    effect_call_callee_span(statement).is_some()
}

/// Rewrite `$effect(fn)` → `$.user_effect(fn)` and `$effect.pre(fn)` → `$.user_pre_effect(fn)`.
fn render_effect_statement(
    source: &str,
    statement: &Statement<'_>,
) -> Result<Option<String>, CompileError> {
    let Some((callee_span, replacement)) = effect_call_callee_span(statement) else {
        return Ok(None);
    };
    let replacements = vec![SourceReplacement::from_span(callee_span, replacement.to_string())];
    let rendered = replace_source_ranges(source, statement.span(), replacements)?;
    Ok(Some(rendered))
}

fn replace_source_ranges(
    source: &str,
    container_span: Span,
    mut replacements: Vec<SourceReplacement>,
) -> Result<String, CompileError> {
    replacements.sort_unstable_by_key(|r| r.start);
    let mut output = String::new();
    let mut cursor = container_span.start as usize;
    let end = container_span.end as usize;
    for r in &replacements {
        if r.start < cursor || r.end > end {
            return Err(CompileError::internal(
                "invalid replacement range while rewriting module source",
            ));
        }
        output.push_str(source.get(cursor..r.start).ok_or_else(|| {
            CompileError::internal("invalid source slice while rewriting module source")
        })?);
        output.push_str(&r.text);
        cursor = r.end;
    }
    output.push_str(source.get(cursor..end).ok_or_else(|| {
        CompileError::internal("invalid source slice while rewriting module source")
    })?);
    Ok(output)
}

pub(super) fn strip_outer_parens(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        // Verify balanced: the outer parens actually wrap the whole expression
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut depth = 0i32;
        let mut balanced = true;
        for ch in inner.chars() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        balanced = false;
                        break;
                    }
                }
                _ => {}
            }
        }
        if balanced && depth == 0 {
            return inner.to_string();
        }
    }
    trimmed.to_string()
}

fn expression_source_from_oxc_span(source: &str, span: Span) -> Result<String, CompileError> {
    source
        .get(span.start as usize..span.end as usize)
        .map(ToString::to_string)
        .ok_or_else(|| CompileError::internal("invalid expression source span"))
}
fn starts_with_import_statement(source: &str) -> bool {
    source.starts_with("import ") || source.starts_with("import\t") || source.starts_with("import{")
}

fn render_module_template(template: &str, basename: &str, import_gap: &str, body: &str) -> String {
    template
        .replace("__BASENAME__", basename)
        .replace("VERSION", &format!("v{}", crate::api::VERSION))
        .replace("__IMPORT_GAP__", import_gap)
        .replace("__BODY__", body)
}

#[cfg(test)]
mod tests {
    use super::compile_module_js_code;
    use crate::api::{GenerateTarget, VERSION};
    use crate::compiler::phases::parse::parse_module_program_for_compile_source;
    use crate::{SourceId, SourceText};
    use camino::Utf8Path;

    fn parsed_module(source: &str) -> crate::compiler::phases::parse::ParsedModuleProgram<'_> {
        parse_module_program_for_compile_source(SourceText::new(SourceId::new(0), source, None))
            .expect("parse module")
    }

    fn parsed_module_with_filename<'src>(
        source: &'src str,
        filename: &'src Utf8Path,
    ) -> crate::compiler::phases::parse::ParsedModuleProgram<'src> {
        parse_module_program_for_compile_source(SourceText::new(
            SourceId::new(0),
            source,
            Some(filename),
        ))
        .expect("parse module")
    }

    #[test]
    fn module_codegen_renders_named_exports_from_ast() {
        let parsed = parsed_module("import x from './x';\nexport { x };");
        let output = compile_module_js_code(&parsed, GenerateTarget::None).expect("module codegen");

        assert_eq!(output, "import x from './x';\n\nexport { x };\n");
    }

    #[test]
    fn module_codegen_renders_function_exports_from_ast() {
        let parsed = parsed_module("export function load(){return 1;}");
        let output = compile_module_js_code(&parsed, GenerateTarget::None).expect("module codegen");

        assert_eq!(output, "export function load() {\n\treturn 1;\n}\n");
    }

    #[test]
    fn module_client_codegen_rewrites_state_bindings_from_ast() {
        let parsed = parsed_module(
            "let count = $state({ value: 1 });\nfunction set(next) {\n\t[count] = next;\n}",
        );
        let output =
            compile_module_js_code(&parsed, GenerateTarget::Client).expect("module codegen");

        assert!(output.contains("let count = $.proxy({ value: 1 });"));
        assert!(output.contains("var $$array = $.to_array(next, 1);"));
        assert!(output.contains("$.set(count, $$array[0], true);"));
        assert!(!output.contains("[count] = next;"));
    }

    #[test]
    fn module_codegen_renders_version_banner() {
        let parsed = parsed_module_with_filename(
            "export const answer = 42;",
            camino::Utf8Path::new("index.svelte.js"),
        );
        let output =
            compile_module_js_code(&parsed, GenerateTarget::Client).expect("module codegen");

        assert!(
            output.starts_with(&format!(
                "/* index.svelte.js generated by Svelte v{VERSION} */"
            )),
            "module banner should include the concrete Svelte version",
        );
    }

    #[test]
    fn module_codegen_strips_typescript_annotations() {
        let parsed = parsed_module_with_filename(
            "export function loadImage(src: string, onLoad: () => void): string { onLoad(); return src; }",
            Utf8Path::new("image-loader.svelte.ts"),
        );
        let output =
            compile_module_js_code(&parsed, GenerateTarget::Client).expect("module codegen");

        assert!(output.contains("export function loadImage(src, onLoad)"));
        assert!(!output.contains(": string"));
    }

    #[test]
    fn module_codegen_drops_type_only_declarations() {
        let parsed = parsed_module_with_filename(
            "export interface DragAndDropOptions { index: number; }\nexport const withDefault = (value: string | undefined): string => value ?? 'ok';",
            Utf8Path::new("drag-and-drop.svelte.ts"),
        );
        let output =
            compile_module_js_code(&parsed, GenerateTarget::Client).expect("module codegen");

        assert!(output.contains("import * as $ from 'svelte/internal/client';"));
        assert!(!output.contains("interface DragAndDropOptions"));
        assert!(!output.contains(": string"));
    }
}
