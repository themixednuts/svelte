mod dynamic_markup;
mod generic_client;
mod generic_renderer;
mod generic_server;
mod static_markup;

use std::collections::BTreeSet;

use crate::api::modern::{
    RawField, estree_node_field_array, estree_node_field_object, estree_node_field_str,
    estree_node_type,
};
use crate::api::{FragmentStrategy, GenerateTarget};
use crate::ast::modern::{EstreeNode, EstreeValue, Root};
use crate::compiler::phases::parse::parse_js_program_for_compile;
use crate::error::CompileError;
use crate::js::render;
use camino::Utf8Path;

pub(crate) use dynamic_markup::compile_dynamic_markup_js;
pub(crate) use generic_client::compile_generic_client_markup_js;
pub(crate) use generic_server::compile_generic_server_markup_js;
pub(crate) use static_markup::compile_static_markup_js;

const TEMPLATE_MODULE_CLIENT: &str = include_str!("codegen/templates/module.client.js");
const TEMPLATE_MODULE_SERVER: &str = include_str!("codegen/templates/module.server.js");

pub(crate) fn compile_component_js_code(
    source: &str,
    target: GenerateTarget,
    fragments: FragmentStrategy,
    root: &Root,
    runes_mode: bool,
    hmr: bool,
    filename: Option<&Utf8Path>,
) -> Option<String> {
    if let Some(output) = compile_static_markup_js(source, target, fragments, root, hmr, filename) {
        return Some(output);
    }
    if let Some(output) = compile_dynamic_markup_js(source, target, root, runes_mode, hmr, filename)
    {
        return Some(output);
    }
    if target == GenerateTarget::Client {
        return compile_generic_client_markup_js(source, target, root, filename);
    }
    if target == GenerateTarget::Server {
        return compile_generic_server_markup_js(source, target, root, filename);
    }
    None
}

pub(crate) fn compile_module_js_code(
    source: &str,
    target: GenerateTarget,
    filename: Option<&Utf8Path>,
) -> Result<String, CompileError> {
    let transformed_body = compile_module_body_from_ast(source, target)?;
    let transformed_body = transformed_body.trim_end();

    if target == GenerateTarget::None {
        return Ok(format!("{transformed_body}\n"));
    }

    let basename = filename
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
    source: &str,
    target: GenerateTarget,
) -> Result<String, CompileError> {
    let program = parse_js_program_for_compile(source).ok_or_else(|| {
        CompileError::internal("failed to parse module source in AST module codegen path")
    })?;
    let body = estree_node_field_array(&program, RawField::Body).unwrap_or(&[]);
    let state_bindings = collect_state_bindings(body)?;

    let mut parts: Vec<(StatementGroup, String)> = Vec::with_capacity(body.len());
    for value in body {
        let stmt = node(value)?;
        let group = statement_group(stmt);
        let rendered = render_module_statement(stmt, target, &state_bindings)?;
        parts.push((group, rendered));
    }

    let mut output = String::new();
    for (i, (group, rendered)) in parts.iter().enumerate() {
        if i > 0 {
            output.push('\n');
            if *group != parts[i - 1].0 {
                output.push('\n');
            }
        }
        output.push_str(rendered);
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatementGroup {
    Import,
    Declaration,
    Export,
    Other,
}

fn statement_group(statement: &EstreeNode) -> StatementGroup {
    match estree_node_type(statement) {
        Some("ImportDeclaration") => StatementGroup::Import,
        Some("VariableDeclaration") => StatementGroup::Declaration,
        Some("FunctionDeclaration") => StatementGroup::Other,
        Some("ExportNamedDeclaration") => {
            if let Some(decl) = estree_node_field_object(statement, RawField::Declaration) {
                statement_group(decl)
            } else {
                StatementGroup::Export
            }
        }
        Some("ExportDefaultDeclaration") => StatementGroup::Export,
        Some("ExportAllDeclaration") => StatementGroup::Export,
        _ => StatementGroup::Other,
    }
}

fn collect_state_bindings(statements: &[EstreeValue]) -> Result<BTreeSet<String>, CompileError> {
    let mut bindings = BTreeSet::new();
    for value in statements {
        let statement = node(value)?;
        match estree_node_type(statement) {
            Some("VariableDeclaration") => {
                collect_state_bindings_from_variable_declaration(statement, &mut bindings)?
            }
            Some("ExportNamedDeclaration") => {
                let Some(declaration) = estree_node_field_object(statement, RawField::Declaration)
                else {
                    continue;
                };
                if estree_node_type(declaration) == Some("VariableDeclaration") {
                    collect_state_bindings_from_variable_declaration(declaration, &mut bindings)?;
                }
            }
            _ => {}
        }
    }
    Ok(bindings)
}

fn collect_state_bindings_from_variable_declaration(
    declaration: &EstreeNode,
    bindings: &mut BTreeSet<String>,
) -> Result<(), CompileError> {
    let declarations = estree_node_field_array(declaration, RawField::Declarations).unwrap_or(&[]);
    for value in declarations {
        let declarator = node(value)?;
        let Some(id) = estree_node_field_object(declarator, RawField::Id) else {
            continue;
        };
        if estree_node_type(id) != Some("Identifier") {
            continue;
        }
        let Some(init) = estree_node_field_object(declarator, RawField::Init) else {
            continue;
        };
        if extract_state_call_argument(init).is_some() {
            if let Some(name) = estree_node_field_str(id, RawField::Name) {
                bindings.insert(name.to_string());
            }
        }
    }
    Ok(())
}

fn render_module_statement(
    statement: &EstreeNode,
    target: GenerateTarget,
    state_bindings: &BTreeSet<String>,
) -> Result<String, CompileError> {
    match estree_node_type(statement) {
        Some("VariableDeclaration") => render_variable_declaration(statement, target, false),
        Some("FunctionDeclaration") => {
            render_function_declaration(statement, target, state_bindings, false)
        }
        Some("ExportNamedDeclaration") => {
            let Some(declaration) = estree_node_field_object(statement, RawField::Declaration)
            else {
                return render_node(statement);
            };
            match estree_node_type(declaration) {
                Some("VariableDeclaration") => {
                    render_variable_declaration(declaration, target, true)
                }
                Some("FunctionDeclaration") => {
                    render_function_declaration(declaration, target, state_bindings, true)
                }
                _ => render_node(statement),
            }
        }
        _ => render_node(statement),
    }
}

fn render_variable_declaration(
    declaration: &EstreeNode,
    target: GenerateTarget,
    exported: bool,
) -> Result<String, CompileError> {
    let declaration_kind = match estree_node_field_str(declaration, RawField::Kind) {
        Some("var" | "let" | "const") => {
            estree_node_field_str(declaration, RawField::Kind).expect("kind checked")
        }
        Some("using" | "await using") => {
            return Err(CompileError::unimplemented(
                "module variable declarations with using/await using",
            ));
        }
        _ => return render_node(declaration),
    };

    let declarations = estree_node_field_array(declaration, RawField::Declarations).unwrap_or(&[]);
    let mut declarators = Vec::with_capacity(declarations.len());
    for value in declarations {
        declarators.push(render_variable_declarator(node(value)?, target)?);
    }

    let mut out = String::new();
    if exported {
        out.push_str("export ");
    }
    out.push_str(declaration_kind);
    out.push(' ');
    out.push_str(&declarators.join(", "));
    out.push(';');
    Ok(out)
}

fn render_variable_declarator(
    declarator: &EstreeNode,
    target: GenerateTarget,
) -> Result<String, CompileError> {
    let Some(id) = estree_node_field_object(declarator, RawField::Id) else {
        return render_node(declarator);
    };
    if estree_node_type(id) != Some("Identifier") {
        return render_node(declarator);
    }
    let Some(init) = estree_node_field_object(declarator, RawField::Init) else {
        return render_node(declarator);
    };
    let Some(state_argument) = extract_state_call_argument(init) else {
        return render_node(declarator);
    };

    let Some(name) = estree_node_field_str(id, RawField::Name) else {
        return render_node(declarator);
    };
    let initializer = match target {
        GenerateTarget::None => return render_node(declarator),
        GenerateTarget::Server => render_state_initializer_server(state_argument)?,
        GenerateTarget::Client => {
            let state_fn = if state_argument.is_proxy_like() {
                "$.proxy"
            } else {
                "$.state"
            };
            format!("{state_fn}({})", render_state_argument(state_argument)?)
        }
    };

    Ok(format!("{name} = {initializer}"))
}

fn render_state_initializer_server(argument: StateArgument<'_>) -> Result<String, CompileError> {
    render_state_argument(argument)
}

fn render_state_argument(argument: StateArgument<'_>) -> Result<String, CompileError> {
    match argument {
        StateArgument::Object(node) | StateArgument::Array(node) | StateArgument::Other(node) => {
            render_node(node)
        }
    }
}

fn render_function_declaration(
    function: &EstreeNode,
    target: GenerateTarget,
    state_bindings: &BTreeSet<String>,
    exported: bool,
) -> Result<String, CompileError> {
    let rendered = render_node(function)?;
    if target != GenerateTarget::Client {
        if exported {
            return Ok(format!("export {rendered}"));
        }
        return Ok(rendered);
    }

    let Some(body) = estree_node_field_object(function, RawField::Body) else {
        return Err(CompileError::unimplemented(
            "module client function declarations without bodies",
        ));
    };
    let statements = estree_node_field_array(body, RawField::Body).unwrap_or(&[]);

    let mut rendered_statements = Vec::with_capacity(statements.len());
    let mut transformed_any = false;
    for value in statements {
        let statement = node(value)?;
        if let Some(transformed) =
            render_client_state_assignment_statement(statement, state_bindings)?
        {
            transformed_any = true;
            rendered_statements.push(transformed);
        } else {
            rendered_statements.push(render_node(statement)?);
        }
    }

    if !transformed_any {
        if exported {
            return Ok(format!("export {rendered}"));
        }
        return Ok(rendered);
    }

    let body_source = rendered_statements
        .iter()
        .map(|statement| indent_block(statement, 1))
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut output = String::new();
    if exported {
        output.push_str("export ");
    }
    output.push_str(&render_function_head(function)?);
    output.push_str(" {\n");
    output.push_str(&body_source);
    output.push_str("\n}");
    Ok(output)
}

fn render_client_state_assignment_statement(
    statement: &EstreeNode,
    state_bindings: &BTreeSet<String>,
) -> Result<Option<String>, CompileError> {
    if estree_node_type(statement) != Some("ExpressionStatement") {
        return Ok(None);
    }
    let Some(assignment) = estree_node_field_object(statement, RawField::Expression) else {
        return Ok(None);
    };
    if estree_node_type(assignment) != Some("AssignmentExpression") {
        return Ok(None);
    }
    let Some(left) = estree_node_field_object(assignment, RawField::Left) else {
        return Ok(None);
    };
    let left = inner(left);
    if estree_node_type(left) != Some("ArrayPattern") {
        return Ok(None);
    }
    let elements = estree_node_field_array(left, RawField::Elements).unwrap_or(&[]);

    let mut bindings = Vec::with_capacity(elements.len());
    for value in elements {
        let EstreeValue::Object(identifier) = value else {
            return Ok(None);
        };
        let identifier = inner(identifier);
        if estree_node_type(identifier) != Some("Identifier") {
            return Ok(None);
        }
        let Some(binding_name) = estree_node_field_str(identifier, RawField::Name) else {
            return Ok(None);
        };
        if !state_bindings.contains(binding_name) {
            return Ok(None);
        }
        bindings.push(binding_name.to_string());
    }
    if bindings.is_empty() {
        return Ok(None);
    }

    let Some(right) = estree_node_field_object(assignment, RawField::Right) else {
        return Ok(None);
    };
    let right = inner(right);
    if estree_node_type(right) != Some("Identifier") {
        return Ok(None);
    }
    let Some(parameter_name) = estree_node_field_str(right, RawField::Name) else {
        return Ok(None);
    };
    let argument = render_node(right)?;

    let mut output = String::new();
    output.push_str(&format!(
        "(({parameter_name}) => {{\n\tvar $$array = $.to_array({parameter_name}, {});\n\n",
        bindings.len()
    ));
    for (index, binding) in bindings.iter().enumerate() {
        output.push_str(&format!("\t$.set({binding}, $$array[{index}], true);\n"));
    }
    output.push_str(&format!("}})({argument});"));
    Ok(Some(output))
}

fn indent_block(value: &str, level: usize) -> String {
    let indent = "\t".repeat(level);
    value
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy)]
enum StateArgument<'a> {
    Object(&'a EstreeNode),
    Array(&'a EstreeNode),
    Other(&'a EstreeNode),
}

impl StateArgument<'_> {
    fn is_proxy_like(&self) -> bool {
        matches!(self, Self::Object(_) | Self::Array(_))
    }
}

fn extract_state_call_argument<'a>(expression: &'a EstreeNode) -> Option<StateArgument<'a>> {
    let expression = inner(expression);
    if estree_node_type(expression) != Some("CallExpression") {
        return None;
    }
    let callee = estree_node_field_object(expression, RawField::Callee)?;
    if !is_identifier_named(inner(callee), "$state") {
        return None;
    }
    let arguments = estree_node_field_array(expression, RawField::Arguments)?;
    let [argument] = arguments else {
        return None;
    };
    let argument = inner(node(argument).ok()?);
    match estree_node_type(argument) {
        Some("ObjectExpression") => Some(StateArgument::Object(argument)),
        Some("ArrayExpression") => Some(StateArgument::Array(argument)),
        _ => Some(StateArgument::Other(argument)),
    }
}

fn render_function_head(function: &EstreeNode) -> Result<String, CompileError> {
    let mut function = function.clone();
    function.fields.insert(
        RawField::Body.as_str().to_string(),
        EstreeValue::Object(empty_block()),
    );
    let rendered = render_node(&function)?;
    rendered
        .strip_suffix(" {}")
        .map(ToString::to_string)
        .ok_or_else(|| CompileError::internal("failed to render function header"))
}

fn empty_block() -> EstreeNode {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        RawField::Type.as_str().to_string(),
        EstreeValue::String("BlockStatement".into()),
    );
    fields.insert(
        RawField::Body.as_str().to_string(),
        EstreeValue::Array(Box::default()),
    );
    EstreeNode { fields }
}

fn render_node(node: &EstreeNode) -> Result<String, CompileError> {
    render(node).ok_or_else(|| CompileError::internal("failed to render ESTree node"))
}

fn node(value: &EstreeValue) -> Result<&EstreeNode, CompileError> {
    match value {
        EstreeValue::Object(node) => Ok(node),
        _ => Err(CompileError::internal("expected ESTree node")),
    }
}

fn inner(mut node: &EstreeNode) -> &EstreeNode {
    while estree_node_type(node) == Some("ParenthesizedExpression") {
        let Some(next) = estree_node_field_object(node, RawField::Expression) else {
            break;
        };
        node = next;
    }
    node
}

fn is_identifier_named(node: &EstreeNode, name: &str) -> bool {
    estree_node_type(node) == Some("Identifier")
        && estree_node_field_str(node, RawField::Name) == Some(name)
}

fn starts_with_import_statement(source: &str) -> bool {
    source.starts_with("import ") || source.starts_with("import\t") || source.starts_with("import{")
}

fn render_module_template(template: &str, basename: &str, import_gap: &str, body: &str) -> String {
    template
        .replace("__BASENAME__", basename)
        .replace("__IMPORT_GAP__", import_gap)
        .replace("__BODY__", body)
}

#[cfg(test)]
mod tests {
    use super::compile_module_js_code;
    use crate::api::GenerateTarget;

    #[test]
    fn module_codegen_renders_named_exports_from_ast() {
        let output = compile_module_js_code(
            "import x from './x';\nexport { x };",
            GenerateTarget::None,
            None,
        )
        .expect("module codegen");

        assert_eq!(output, "import x from './x';\n\nexport { x };\n");
    }

    #[test]
    fn module_codegen_renders_function_exports_from_ast() {
        let output = compile_module_js_code(
            "export function load(){return 1;}",
            GenerateTarget::None,
            None,
        )
        .expect("module codegen");

        assert_eq!(output, "export function load() { return 1; }\n");
    }

    #[test]
    fn module_client_codegen_rewrites_state_bindings_from_ast() {
        let output = compile_module_js_code(
            "let count = $state({ value: 1 });\nfunction set(next) {\n\t[count] = next;\n}",
            GenerateTarget::Client,
            None,
        )
        .expect("module codegen");

        assert!(output.contains("let count = $.proxy({ value: 1 });"));
        assert!(output.contains("var $$array = $.to_array(next, 1);"));
        assert!(output.contains("$.set(count, $$array[0], true);"));
        assert!(!output.contains("[count] = next;"));
    }
}
