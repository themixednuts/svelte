use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::ControlFlow;
use std::sync::Arc;

use oxc_ast::ast::{
    AssignmentTarget, Declaration, Expression as OxcExpression, ModuleExportName, Statement,
    VariableDeclaration,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span as OxcSpan};
use svelte_syntax::ParsedJsProgram;

use crate::CompileError;
use crate::api::modern::expression_literal_string;
use crate::api::scan::migrate_svelte_ignore;
use crate::api::{MigrateOptions, MigrateResult, ParseMode, ParseOptions, is_void_element_name};
use crate::ast::common::Span;
use crate::ast::modern::{
    Alternate, Attribute, AttributeValue, AttributeValueList, AwaitBlock, Comment, Fragment,
    IfBlock, KeyBlock, Node, RegularElement, Root as ModernRoot, Script, SvelteElement,
};
use crate::ast::{Document, Root};
use crate::compiler::phases::parse::parse_component;

pub(crate) fn migrate(
    source: &str,
    options: MigrateOptions,
) -> Result<MigrateResult, CompileError> {
    let document = match parse_component(
        source,
        ParseOptions {
            mode: ParseMode::Modern,
            loose: false,
            ..Default::default()
        },
    ) {
        Ok(document) => document,
        Err(error) => {
            return Ok(MigrateResult {
                code: migration_task_result(
                    source,
                    &format!("{}\nhttps://svelte.dev/e/{}", error.message, error.code),
                ),
            });
        }
    };
    if let Some(code) = migrate_parse_error(&document, source) {
        return Ok(MigrateResult { code });
    }

    if let Some(code) = migrate_impossible_before_after_update(&document, source) {
        return Ok(MigrateResult { code });
    }
    if let Some(code) = migrate_impossible_export_pattern(&document, source) {
        return Ok(MigrateResult { code });
    }
    if let Some(code) = migrate_impossible_named_props_with_dollar_props(&document, source) {
        return Ok(MigrateResult { code });
    }
    if let Some(code) = migrate_impossible_slot_name_change(&document, source) {
        return Ok(MigrateResult { code });
    }
    if let Some(code) = migrate_impossible_rune_binding_conflict(&document, source) {
        return Ok(MigrateResult { code });
    }

    let mut edits = Vec::new();
    collect_migrate_edits(
        source,
        &document,
        options.use_ts,
        options.filename.as_deref(),
        options.filename.is_none(),
        &mut edits,
    );

    let code = if edits.is_empty() {
        Arc::from(source)
    } else {
        Arc::<str>::from(apply_edits(source, &mut edits))
    };
    let code = Arc::<str>::from(normalize_svelte_ignore_comments(&code));

    Ok(MigrateResult { code })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SimpleExportProp {
    replacement_start: usize,
    replacement_end: usize,
    statement_start: usize,
    statement_end: usize,
    leading_comment_range: Option<(usize, usize)>,
    trailing_comment_range: Option<(usize, usize)>,
    name: String,
    has_init: bool,
    has_type: bool,
    type_source: Option<String>,
    default_source: Option<String>,
    bindable: bool,
    comment: Option<String>,
    trailing_comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PropsTypeMember {
    name: String,
    type_source: String,
    optional: bool,
    comment: Option<String>,
    trailing_comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PropsTypeDeclaration {
    start: usize,
    end: usize,
    from_type_alias: bool,
    members: Vec<PropsTypeMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScriptSlotBindingKind {
    ExportAlias,
    LocalAlias,
    ReactiveDerived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScriptSlotBinding {
    kind: ScriptSlotBindingKind,
    local_name: String,
    slot_name: String,
    statement_start: usize,
    statement_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlotPropRequirement {
    name: String,
    accepts_args: bool,
    order: usize,
}

#[derive(Debug, Clone)]
struct EventHandlerNames {
    create_bubbler: String,
    handlers: String,
    prevent_default: String,
    stop_propagation: String,
    stop_immediate_propagation: String,
    self_name: String,
    trusted: String,
    once: String,
    passive: String,
    nonpassive: String,
    bubble: String,
}

#[derive(Debug, Default)]
struct EventHandlerMigrationState {
    used_imports: HashSet<&'static str>,
    needs_bubble: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SvelteComponentScopeKind {
    IfBlock,
    EachBlock,
    AwaitBlock,
    SnippetBlock,
    Component,
    SvelteComponent,
}

#[derive(Debug, Clone, Copy)]
struct SvelteComponentPathEntry {
    kind: Option<SvelteComponentScopeKind>,
    start: usize,
    skip_dynamic_children: bool,
}

#[derive(Debug, Default, Clone)]
struct SvelteComponentMigrationState {
    derived_components: Vec<(String, String)>,
    derived_component_names: HashMap<String, String>,
    used_names: HashSet<String>,
    needs_rest_props: bool,
}

#[derive(Debug, Clone)]
struct RenderedSvelteComponentTag {
    name: String,
    prelude: Option<String>,
}

fn migrate_impossible_before_after_update(document: &Document, source: &str) -> Option<Arc<str>> {
    let Root::Modern(root) = &document.root else {
        return None;
    };
    let script = root.instance.as_ref()?;
    if !program_has_call(&script.content, "beforeUpdate")
        || !program_has_call(&script.content, "afterUpdate")
    {
        return None;
    }

    Some(Arc::from(format!(
        "<!-- @migration-task Error while migrating Svelte code: Can't migrate code with beforeUpdate and afterUpdate. Please migrate by hand. -->\n{source}"
    )))
}

fn migrate_parse_error(document: &Document, source: &str) -> Option<Arc<str>> {
    let Root::Modern(root) = &document.root else {
        return None;
    };
    let error = root.errors.first()?;
    let (message, code) = match &error.kind {
        crate::ast::common::ParseErrorKind::UnexpectedEof => {
            ("Unexpected end of input", "unexpected_eof")
        }
        _ => return None,
    };

    Some(migration_task_result(
        source,
        &format!("{message}\nhttps://svelte.dev/e/{code}"),
    ))
}

fn migrate_impossible_export_pattern(document: &Document, source: &str) -> Option<Arc<str>> {
    let Root::Modern(root) = &document.root else {
        return None;
    };
    let script = root.instance.as_ref()?;
    if !program_has_non_identifier_export_let(&script.content) {
        return None;
    }

    Some(Arc::from(format!(
        "<!-- @migration-task Error while migrating Svelte code: Encountered an export declaration pattern that is not supported for automigration. -->\n{source}"
    )))
}

fn migrate_impossible_named_props_with_dollar_props(
    document: &Document,
    source: &str,
) -> Option<Arc<str>> {
    let Root::Modern(root) = &document.root else {
        return None;
    };
    let script = root.instance.as_ref()?;
    if !program_has_export_let(&script.content) || !source.contains("$$props") {
        return None;
    }
    if can_rewrite_simple_rest_props(root, source) {
        return None;
    }

    Some(Arc::from(format!(
        "<!-- @migration-task Error while migrating Svelte code: $$props is used together with named props in a way that cannot be automatically migrated. -->\n{source}"
    )))
}

fn migrate_impossible_slot_name_change(document: &Document, source: &str) -> Option<Arc<str>> {
    let Root::Modern(root) = &document.root else {
        return None;
    };
    if root
        .options
        .as_ref()
        .and_then(|options| options.custom_element.as_ref())
        .is_some()
    {
        return None;
    }

    let declared_names = root
        .instance
        .as_ref()
        .map(|script| declared_names_in_program(&script.content))
        .unwrap_or_default();
    let (slot_name, migrated_name) =
        first_impossible_slot_name_change(&root.fragment, &declared_names)?;

    Some(Arc::from(format!(
        "<!-- @migration-task Error while migrating Svelte code: This migration would change the name of a slot ({slot_name} to {migrated_name}) making the component unusable -->\n{source}"
    )))
}

fn span_range(span: OxcSpan) -> (usize, usize) {
    (span.start as usize, span.end as usize)
}

fn callee_name(callee: &OxcExpression<'_>) -> Option<String> {
    match callee.get_inner_expression() {
        OxcExpression::Identifier(reference) => Some(reference.name.as_str().to_owned()),
        OxcExpression::StaticMemberExpression(member) => {
            let OxcExpression::Identifier(object) = member.object.get_inner_expression() else {
                return None;
            };
            Some(format!("{}.{}", object.name, member.property.name))
        }
        _ => None,
    }
}

fn is_literal_expression(expression: &OxcExpression<'_>) -> bool {
    match expression.get_inner_expression() {
        OxcExpression::BooleanLiteral(_)
        | OxcExpression::NullLiteral(_)
        | OxcExpression::NumericLiteral(_)
        | OxcExpression::BigIntLiteral(_)
        | OxcExpression::RegExpLiteral(_)
        | OxcExpression::StringLiteral(_) => true,
        OxcExpression::TemplateLiteral(template) => template.expressions.is_empty(),
        _ => false,
    }
}

fn expression_source_from_expression(
    source: &str,
    source_offset: usize,
    expression: &OxcExpression<'_>,
) -> Option<String> {
    expression_source_from_span_with_offset(source, source_offset, expression.span())
}

fn assignment_target_source(
    source: &str,
    source_offset: usize,
    target: &AssignmentTarget<'_>,
) -> Option<String> {
    expression_source_from_span_with_offset(source, source_offset, target.span())
}

fn module_export_name_as_str<'a>(name: &'a ModuleExportName<'a>) -> Option<&'a str> {
    match name {
        ModuleExportName::IdentifierName(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::StringLiteral(_) => None,
    }
}

fn collect_declared_names_from_variable_declaration(
    declaration: &VariableDeclaration<'_>,
    names: &mut HashSet<String>,
) {
    for declarator in &declaration.declarations {
        collect_binding_pattern_names(&declarator.id, names);
    }
}

fn collect_binding_pattern_names(
    pattern: &oxc_ast::ast::BindingPattern<'_>,
    names: &mut HashSet<String>,
) {
    match pattern {
        oxc_ast::ast::BindingPattern::BindingIdentifier(identifier) => {
            names.insert(identifier.name.to_string());
        }
        oxc_ast::ast::BindingPattern::AssignmentPattern(pattern) => {
            collect_binding_pattern_names(&pattern.left, names);
        }
        oxc_ast::ast::BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_binding_pattern_names(element, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_binding_pattern_names(&rest.argument, names);
            }
        }
        oxc_ast::ast::BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_binding_pattern_names(&property.value, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_binding_pattern_names(&rest.argument, names);
            }
        }
    }
}

fn export_let_declaration<'a>(statement: &'a Statement<'a>) -> Option<&'a VariableDeclaration<'a>> {
    let Statement::ExportNamedDeclaration(statement) = statement else {
        return None;
    };
    let Some(Declaration::VariableDeclaration(declaration)) = statement.declaration.as_ref() else {
        return None;
    };
    (declaration.kind.as_str() == "let").then_some(declaration)
}

fn program_has_call(program: &ParsedJsProgram, expected_callee_name: &str) -> bool {
    struct CallVisitor<'a> {
        callee_name: &'a str,
        found: bool,
    }

    impl<'a> Visit<'a> for CallVisitor<'_> {
        fn visit_call_expression(&mut self, expression: &oxc_ast::ast::CallExpression<'a>) {
            if self.found {
                return;
            }
            self.found = callee_name(&expression.callee) == Some(self.callee_name.to_string());
            if !self.found {
                walk::walk_call_expression(self, expression);
            }
        }
    }

    let mut visitor = CallVisitor {
        callee_name: expected_callee_name,
        found: false,
    };
    visitor.visit_program(program.program());
    visitor.found
}

fn program_has_export_let(program: &ParsedJsProgram) -> bool {
    program
        .program()
        .body
        .iter()
        .any(|statement| export_let_declaration(statement).is_some())
}

fn program_has_non_identifier_export_let(program: &ParsedJsProgram) -> bool {
    program.program().body.iter().any(|statement| {
        let Some(declaration) = export_let_declaration(statement) else {
            return false;
        };
        declaration
            .declarations
            .iter()
            .any(|declarator| declarator.id.get_binding_identifier().is_none())
    })
}

fn collect_simple_export_props(
    program: &ParsedJsProgram,
    bind_targets: &HashSet<String>,
    source: &str,
    source_offset: usize,
) -> Vec<SimpleExportProp> {
    let mut props = Vec::new();
    for statement in &program.program().body {
        let Some(declaration) = export_let_declaration(statement) else {
            continue;
        };
        let (statement_start, statement_end) = span_range_with_offset(source_offset, statement.span());
        let Some(statement_source) = source.get(statement_start..statement_end) else {
            continue;
        };
        let declarations = &declaration.declarations;
        if declarations.len() != 1 {
            return Vec::new();
        }

        let declarator = &declarations[0];
        let Some(id) = declarator.id.get_binding_identifier() else {
            return Vec::new();
        };
        let name = id.name.as_str();
        let leading_comment_start = leading_comment_start_stmt(statement, source, source_offset)
            .or_else(|| raw_preceding_comment_start(source, statement_start));
        let trailing_end = trailing_comment_end_stmt(statement, source, source_offset).unwrap_or(statement_end);
        let comment = raw_preceding_comment(source, statement_start);
        let trailing_comment = raw_trailing_comment_text(source, statement_end);
        let leading_comment_range = raw_preceding_comment_range(source, statement_start);
        let trailing_comment_range = raw_trailing_comment_range(source, statement_end);
        let default_source = declarator
            .init
            .as_ref()
            .and_then(|init| {
                expression_source_from_span_with_offset(source, source_offset, init.span())
            });

        props.push(SimpleExportProp {
            replacement_start: leading_comment_start.unwrap_or(statement_start),
            replacement_end: trailing_end,
            statement_start,
            statement_end,
            leading_comment_range,
            trailing_comment_range,
            name: name.to_string(),
            has_init: declarator.init.is_some(),
            has_type: statement_source.contains(':'),
            type_source: extract_simple_export_type(statement, statement_source, source, name)
                .or_else(|| comment.as_deref().and_then(extract_jsdoc_type))
                .or_else(|| default_source.as_deref().and_then(infer_type_from_default)),
            default_source,
            bindable: bind_targets.contains(name),
            comment,
            trailing_comment,
        });
    }

    props
}

fn can_rewrite_simple_rest_props(root: &ModernRoot, source: &str) -> bool {
    let Some(instance) = root.instance.as_ref() else {
        return false;
    };
    let bind_targets = prop_bind_targets(root);
    let props =
        collect_simple_export_props(&instance.content, &bind_targets, source, instance.content_start);
    if props.is_empty()
        || props
            .iter()
            .any(|prop| prop.has_init || prop.has_type || prop.bindable)
    {
        return false;
    }

    let ranges = props
        .iter()
        .map(|prop| (prop.statement_start, prop.statement_end))
        .collect::<Vec<_>>();
    let remainder = source_without_ranges(source, &ranges);
    props
        .iter()
        .all(|prop| !identifier_occurs(&remainder, prop.name.as_str()))
}

fn extract_simple_export_type(
    _statement: &Statement<'_>,
    statement_source: &str,
    _source: &str,
    name: &str,
) -> Option<String> {
    let marker = format!("let {name}:");
    let start = statement_source.find(&marker)? + marker.len();
    let remainder = &statement_source[start..];
    let end = remainder.find([';', '='])?;
    Some(remainder[..end].trim().to_string())
}

fn extract_jsdoc_type(comment: &str) -> Option<String> {
    // Extract type from JSDoc `/** @type {SomeType} */` comment
    let marker = "@type ";
    let type_start = comment.find(marker)? + marker.len();
    let remainder = &comment[type_start..];
    if !remainder.starts_with('{') {
        return None;
    }
    let mut depth = 0;
    let mut end = 0;
    for (i, ch) in remainder.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == 0 {
        return None;
    }
    // Extract just the type without braces
    Some(remainder[1..end].trim().to_string())
}

fn leading_comment_start_stmt(
    statement: &Statement<'_>,
    source: &str,
    source_offset: usize,
) -> Option<usize> {
    let (statement_start, _) = span_range_with_offset(source_offset, statement.span());
    raw_preceding_comment_start(source, statement_start)
}

fn trailing_comment_end_stmt(
    statement: &Statement<'_>,
    source: &str,
    source_offset: usize,
) -> Option<usize> {
    let (_, statement_end) = span_range_with_offset(source_offset, statement.span());
    raw_trailing_comment_end(source, statement_end)
}

/// Extract the trailing comment text after a statement on the same line.
fn raw_trailing_comment_text(source: &str, statement_end: usize) -> Option<String> {
    let line_end = source[statement_end..]
        .find('\n')
        .map(|offset| statement_end + offset)
        .unwrap_or(source.len());
    let after_stmt = &source[statement_end..line_end];

    // Check for `// ...` on the same line
    if let Some(slash_pos) = after_stmt.find("//") {
        let comment_start = statement_end + slash_pos;
        return source.get(comment_start..line_end).map(|s| s.trim_end().to_string());
    }

    // Check for `/* ... */` starting on the same line (may span multiple lines)
    if let Some(star_pos) = after_stmt.find("/*") {
        let comment_start = statement_end + star_pos;
        if let Some(close_offset) = source[comment_start..].find("*/") {
            let comment_end = comment_start + close_offset + 2;
            return source.get(comment_start..comment_end).map(|s| s.trim_end().to_string());
        }
    }

    None
}

/// Find the end position of a trailing comment after a statement on the same line.
fn raw_trailing_comment_end(source: &str, statement_end: usize) -> Option<usize> {
    let line_end = source[statement_end..]
        .find('\n')
        .map(|offset| statement_end + offset)
        .unwrap_or(source.len());
    let after_stmt = &source[statement_end..line_end];

    // Check for `// ...` on the same line
    if after_stmt.contains("//") {
        return Some(line_end);
    }

    // Check for `/* ... */` starting on the same line (may span multiple lines)
    if let Some(star_pos) = after_stmt.find("/*") {
        let comment_start = statement_end + star_pos;
        if let Some(close_offset) = source[comment_start..].find("*/") {
            return Some(comment_start + close_offset + 2);
        }
    }

    None
}

/// Find the range (start, end) of a trailing comment after a statement on the same line.
fn raw_trailing_comment_range(source: &str, statement_end: usize) -> Option<(usize, usize)> {
    let line_end = source[statement_end..]
        .find('\n')
        .map(|offset| statement_end + offset)
        .unwrap_or(source.len());
    let after_stmt = &source[statement_end..line_end];

    if let Some(slash_pos) = after_stmt.find("//") {
        let comment_start = statement_end + slash_pos;
        return Some((comment_start, line_end));
    }

    if let Some(star_pos) = after_stmt.find("/*") {
        let comment_start = statement_end + star_pos;
        if let Some(close_offset) = source[comment_start..].find("*/") {
            return Some((comment_start, comment_start + close_offset + 2));
        }
    }

    None
}

fn expression_source_from_span(source: &str, span: OxcSpan) -> Option<String> {
    let (start, end) = span_range(span);
    source.get(start..end).map(ToString::to_string)
}

fn expression_source(source: &str, expression: &crate::ast::modern::Expression) -> Option<String> {
    // Prefer the modern AST's absolute positions, which are correct for template expressions.
    // The OXC span is relative to the parsed snippet, not the full .svelte source.
    expression_source_from_range(source, expression.start, expression.end)
}

fn expression_source_from_range(source: &str, start: usize, end: usize) -> Option<String> {
    source.get(start..end).map(ToString::to_string)
}

fn infer_type_from_default(default_source: &str) -> Option<String> {
    let trimmed = default_source.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        Some(String::from("string"))
    } else if matches!(trimmed, "true" | "false") {
        Some(String::from("boolean"))
    } else if trimmed.parse::<f64>().is_ok() {
        Some(String::from("number"))
    } else {
        None
    }
}

fn source_without_ranges(source: &str, ranges: &[(usize, usize)]) -> String {
    if ranges.is_empty() {
        return source.to_string();
    }

    let mut sorted = ranges.to_vec();
    sorted.sort_unstable_by_key(|(start, _)| *start);

    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;

    for (start, end) in sorted {
        if start > cursor {
            output.push_str(&source[cursor..start]);
        }
        cursor = cursor.max(end);
    }

    if cursor < source.len() {
        output.push_str(&source[cursor..]);
    }

    output
}

fn identifier_occurs(source: &str, name: &str) -> bool {
    let mut search_start = 0usize;
    while let Some(relative) = source[search_start..].find(name) {
        let start = search_start + relative;
        let end = start + name.len();
        let prev = source[..start].chars().next_back();
        let next = source[end..].chars().next();
        let prev_ok = prev.is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'));
        let next_ok = next.is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'));
        if prev_ok && next_ok {
            return true;
        }
        search_start = end;
    }

    false
}

fn prop_bind_targets(root: &ModernRoot) -> HashSet<String> {
    let mut names = fragment_bind_targets(&root.fragment);
    if let Some(instance) = root.instance.as_ref() {
        names.extend(script_updated_names(&instance.content));
    }
    names
}

fn script_updated_names(program: &ParsedJsProgram) -> HashSet<String> {
    struct UpdatedNamesVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<'a> Visit<'a> for UpdatedNamesVisitor<'_> {
        fn visit_assignment_target(&mut self, target: &AssignmentTarget<'a>) {
            collect_assignment_target_binding_names(target, self.names);
            walk::walk_assignment_target(self, target);
        }

        fn visit_update_expression(&mut self, expression: &oxc_ast::ast::UpdateExpression<'a>) {
            if let Some(name) = expression.argument.get_identifier_name() {
                self.names.insert(name.to_string());
            }
            walk::walk_update_expression(self, expression);
        }
    }

    let mut names = HashSet::new();
    let mut visitor = UpdatedNamesVisitor { names: &mut names };
    visitor.visit_program(program.program());
    names
}

fn should_render_props_multiline(props: &[SimpleExportProp]) -> bool {
    props.len() > 3
}

fn render_destructured_props_with_slots(
    props: &[SimpleExportProp],
    slot_props: &[SlotPropRequirement],
    multiline: bool,
    indent: &str,
) -> String {
    let separator = if multiline {
        format!(",\n{indent}{indent}")
    } else {
        String::from(", ")
    };
    props
        .iter()
        .map(|prop| {
            if prop.bindable {
                if let Some(default) = prop.default_source.as_deref() {
                    format!("{} = $bindable({default})", prop.name)
                } else {
                    format!("{} = $bindable()", prop.name)
                }
            } else if let Some(default) = prop.default_source.as_deref() {
                format!("{} = {default}", prop.name)
            } else {
                prop.name.clone()
            }
        })
        .chain(slot_props.iter().map(|prop| prop.name.clone()))
        .collect::<Vec<_>>()
        .join(&separator)
}

fn replace_props_declarations(
    source: &str,
    props: &[SimpleExportProp],
    replacement: String,
    insertion_point: usize,
    edits: &mut Vec<Edit>,
) {
    let props_end = props
        .iter()
        .map(|prop| prop.statement_end)
        .max()
        .unwrap_or_default();

    if insertion_point > props_end {
        for prop in props {
            edits.push(Edit {
                start: line_start(source, prop.replacement_start),
                end: line_end_including_newline(source, prop.replacement_end),
                replacement: String::new(),
            });
        }
        edits.push(Edit {
            start: insertion_point,
            end: insertion_point,
            replacement,
        });
        return;
    }

    edits.push(Edit {
        start: line_start(source, props[0].replacement_start),
        end: line_end_including_newline(source, props[0].replacement_end),
        replacement,
    });
    for prop in props.iter().skip(1) {
        edits.push(Edit {
            start: line_start(source, prop.replacement_start),
            end: line_end_including_newline(source, prop.replacement_end),
            replacement: String::new(),
        });
    }
}

fn replace_props_declarations_preserving_layout(
    source: &str,
    props: &[SimpleExportProp],
    insertion: String,
    insertion_point: usize,
    edits: &mut Vec<Edit>,
) {
    edits.push(Edit {
        start: insertion_point,
        end: insertion_point,
        replacement: insertion,
    });

    for prop in props {
        let statement_line_start = line_start(source, prop.statement_start);
        let statement_line_end = line_end_including_newline(source, prop.statement_end);

        if let Some((start, end)) = prop.leading_comment_range {
            if end <= statement_line_start {
                push_removal_edit(start, end, insertion_point, edits);
            } else if start < statement_line_start {
                push_removal_edit(start, statement_line_start, insertion_point, edits);
            }
        }
        push_removal_edit(
            statement_line_start,
            statement_line_end,
            insertion_point,
            edits,
        );
        if let Some((start, end)) = prop.trailing_comment_range {
            if start >= statement_line_end {
                push_removal_edit(start, end, insertion_point, edits);
            } else if end > statement_line_end {
                push_removal_edit(statement_line_end, end, insertion_point, edits);
            }
        }
    }
}

fn props_insertion_point(source: &str, instance: &Script, fallback: usize) -> usize {
    let mut insertion_point = fallback;
    for statement in &instance.content.program().body {
        let Statement::ImportDeclaration(statement) = statement else {
            continue;
        };
        let (_, end) = span_range_with_offset(instance.content_start, statement.span);
        insertion_point = insertion_point.max(line_end_including_newline(source, end));
    }
    insertion_point
}

fn props_replacement_insertion_end(source: &str, prop: &SimpleExportProp) -> usize {
    if prop.replacement_end > prop.statement_end {
        line_end_including_newline(source, prop.replacement_end)
    } else {
        prop.replacement_end
    }
}

fn push_removal_edit(start: usize, end: usize, insertion_point: usize, edits: &mut Vec<Edit>) {
    if start < insertion_point && insertion_point < end {
        edits.push(Edit {
            start,
            end: insertion_point,
            replacement: String::new(),
        });
        edits.push(Edit {
            start: insertion_point,
            end,
            replacement: String::new(),
        });
    } else {
        edits.push(Edit {
            start,
            end,
            replacement: String::new(),
        });
    }
}

fn props_have_comments(props: &[SimpleExportProp]) -> bool {
    props.iter().any(|prop| {
        prop.comment.is_some()
            || prop.trailing_comment.is_some()
            || prop.replacement_start != prop.statement_start
            || prop.replacement_end != prop.statement_end
    })
}

fn collect_props_interface_edits(
    source: &str,
    root: &ModernRoot,
    props: &[SimpleExportProp],
    slot_props: &[SlotPropRequirement],
    accessors: bool,
    edits: &mut Vec<Edit>,
) -> bool {
    let Some(instance) = root.instance.as_ref() else {
        return false;
    };
    let Some(interface) = props_type_declaration(&instance.content, source, instance.content_start) else {
        return false;
    };

    let props = props
        .iter()
        .map(|prop| {
            let mut prop = prop.clone();
            if let Some(member) = interface
                .members
                .iter()
                .find(|member| member.name == prop.name)
            {
                prop.type_source = Some(member.type_source.clone());
            }
            prop
        })
        .collect::<Vec<_>>();
    let interface = PropsTypeDeclaration {
        members: merge_props_interface_members(&interface, &props, slot_props),
        ..interface
    };
    let indent = leading_whitespace_before(source, props[0].statement_start).unwrap_or("\t");
    let props_insert_at = props_insertion_point(
        source,
        instance,
        props
            .iter()
            .map(|prop| props_replacement_insertion_end(source, prop))
            .max()
            .unwrap_or_else(|| props_replacement_insertion_end(source, &props[0])),
    );
    let multiline = accessors || should_render_props_multiline(&props);
    let destructured = render_destructured_props_with_slots(&props, slot_props, multiline, indent);
    let accessor_exports = if accessors {
        render_accessor_exports(&props, indent)
    } else {
        String::new()
    };

    edits.push(Edit {
        start: line_start(source, interface.start),
        end: line_start(source, props[0].statement_start),
        replacement: String::new(),
    });
    replace_props_declarations(
        source,
        &props,
        {
            let interface_gap = if interface.from_type_alias {
                format!("{indent}\n")
            } else {
                String::from("\n")
            };
            format!(
                "{indent}\n{interface_gap}{}\n\n{}{}",
                render_props_interface_source(&interface, indent),
                if multiline {
                    format!(
                        "{indent}let {{\n{indent}{indent}{destructured}\n{indent}}}: Props = $props();\n"
                    )
                } else {
                    format!("{indent}let {{ {destructured} }}: Props = $props();\n")
                },
                accessor_exports
            )
        },
        props_insert_at,
        edits,
    );
    true
}

fn slot_reference_name_from_source(source: &str) -> Option<&str> {
    let source = source.trim();
    source
        .strip_prefix("$$slots.")?
        .split_once([' ', ')', ';', '\n'])
        .map_or(Some(source.strip_prefix("$$slots.")?), |(name, _)| {
            Some(name)
        })
}

fn script_slot_reference_name(expression: &OxcExpression<'_>) -> Option<String> {
    match expression.get_inner_expression() {
        OxcExpression::StaticMemberExpression(member) => {
            let OxcExpression::Identifier(object) = member.object.get_inner_expression() else {
                return None;
            };
            (object.name.as_str() == "$$slots").then(|| member.property.name.to_string())
        }
        OxcExpression::ComputedMemberExpression(member) => {
            let OxcExpression::Identifier(object) = member.object.get_inner_expression() else {
                return None;
            };
            if object.name.as_str() != "$$slots" {
                return None;
            }
            match member.expression.get_inner_expression() {
                OxcExpression::StringLiteral(value) => Some(value.value.to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

fn collect_script_slot_bindings(
    program: &ParsedJsProgram,
    source: &str,
    source_offset: usize,
) -> Vec<ScriptSlotBinding> {
    let mut bindings = Vec::new();
    for statement in &program.program().body {
        match statement {
            Statement::ExportNamedDeclaration(statement) => {
                let Some(Declaration::VariableDeclaration(declaration)) =
                    statement.declaration.as_ref()
                else {
                    continue;
                };
                if declaration.kind.as_str() != "let" {
                    continue;
                }
                if declaration.declarations.len() != 1 {
                    continue;
                }
                let declarator = &declaration.declarations[0];
                let Some(id) = declarator.id.get_binding_identifier() else {
                    continue;
                };
                let Some(init) = declarator.init.as_ref() else {
                    continue;
                };
                let Some(slot_name) = script_slot_reference_name(init) else {
                    continue;
                };
                let (statement_start, statement_end) =
                    span_range_with_offset(source_offset, statement.span);
                bindings.push(ScriptSlotBinding {
                    kind: ScriptSlotBindingKind::ExportAlias,
                    local_name: id.name.to_string(),
                    slot_name,
                    statement_start,
                    statement_end,
                });
            }
            Statement::VariableDeclaration(statement) => {
                if statement.kind.as_str() != "let" {
                    continue;
                }
                if statement.declarations.len() != 1 {
                    continue;
                }
                let declarator = &statement.declarations[0];
                let Some(id) = declarator.id.get_binding_identifier() else {
                    continue;
                };
                let Some(init) = declarator.init.as_ref() else {
                    continue;
                };
                let Some(slot_name) = script_slot_reference_name(init) else {
                    continue;
                };
                let (statement_start, statement_end) =
                    span_range_with_offset(source_offset, statement.span);
                bindings.push(ScriptSlotBinding {
                    kind: ScriptSlotBindingKind::LocalAlias,
                    local_name: id.name.to_string(),
                    slot_name,
                    statement_start,
                    statement_end,
                });
            }
            statement @ Statement::LabeledStatement(_labeled_statement) => {
                let Some(reactive_assignment) =
                    reactive_single_assignment(statement, source, source_offset)
                else {
                    continue;
                };
                let Some(slot_name) = script_slot_reference_name(reactive_assignment.right) else {
                    continue;
                };
                bindings.push(ScriptSlotBinding {
                    kind: ScriptSlotBindingKind::ReactiveDerived,
                    local_name: reactive_assignment.name.to_string(),
                    slot_name,
                    statement_start: reactive_assignment.statement_start,
                    statement_end: reactive_assignment.statement_end,
                });
            }
            _ => {}
        }
    }
    bindings
}

fn extend_slot_props_with_script_bindings(
    slot_props: &mut Vec<SlotPropRequirement>,
    bindings: &[ScriptSlotBinding],
    use_rest_props: bool,
) {
    let mut seen = slot_props
        .iter()
        .map(|prop| prop.name.clone())
        .collect::<HashSet<_>>();
    for binding in bindings {
        let slot_name = normalize_slot_identifier(&binding.slot_name);
        if !seen.insert(slot_name.clone()) {
            continue;
        }
        slot_props.push(SlotPropRequirement {
            name: slot_name,
            accepts_args: use_rest_props,
            order: slot_props.len(),
        });
    }
}

fn collect_script_slot_binding_edits(
    source: &str,
    instance: &Script,
    use_rest_props: bool,
    slot_alias_props: &[(SimpleExportProp, String)],
    bindings: &[ScriptSlotBinding],
    slot_props: &[SlotPropRequirement],
    edits: &mut Vec<Edit>,
) {
    if use_rest_props {
        let indent = guess_indent(source);
        let insertion = render_slot_prop_prelude(
            &slot_props
                .iter()
                .cloned()
                .map(|prop| (prop.name.clone(), prop))
                .collect::<HashMap<_, _>>(),
            &HashMap::new(),
            false,
            true,
            indent,
        );
        let mut first_binding_rewritten = false;
        for binding in bindings {
            let indent = leading_whitespace_before(source, binding.statement_start).unwrap_or("\t");
            let replacement = match binding.kind {
                ScriptSlotBindingKind::LocalAlias => format!(
                    "{indent}let {} = props.{};\n",
                    binding.local_name,
                    normalize_slot_identifier(&binding.slot_name)
                ),
                ScriptSlotBindingKind::ReactiveDerived => format!(
                    "{indent}let {} = $derived(props.{});\n",
                    binding.local_name,
                    normalize_slot_identifier(&binding.slot_name)
                ),
                ScriptSlotBindingKind::ExportAlias => continue,
            };
            edits.push(Edit {
                start: line_start(source, binding.statement_start),
                end: line_end_including_newline(source, binding.statement_end),
                replacement: if !first_binding_rewritten {
                    first_binding_rewritten = true;
                    format!("{insertion}{replacement}")
                } else {
                    replacement
                },
            });
        }
        if !first_binding_rewritten {
            edits.push(Edit {
                start: instance.content_start,
                end: instance.content_start,
                replacement: format!("\n{insertion}"),
            });
        }
        return;
    }

    let mut destructured = Vec::new();
    let mut seen = HashSet::new();
    for (prop, slot_name) in slot_alias_props {
        let normalized = normalize_slot_identifier(slot_name);
        if seen.insert(normalized.clone()) {
            destructured.push(normalized.clone());
        }
        destructured.push(format!("{} = {}", prop.name, normalized));
    }
    for binding in bindings {
        let normalized = normalize_slot_identifier(&binding.slot_name);
        if seen.insert(normalized.clone()) {
            destructured.push(normalized);
        }
    }
    let indent = guess_indent(source);
    let replacement = if destructured.len() > 3 {
        format!(
            "{indent}let {{\n{indent}{indent}{}\n{indent}}} = $props();\n",
            destructured.join(&format!(",\n{indent}{indent}"))
        )
    } else {
        format!(
            "{indent}let {{ {} }} = $props();\n",
            destructured.join(", ")
        )
    };
    if let Some((first_prop, _)) = slot_alias_props.first() {
        let first_start = line_start(source, first_prop.statement_start);
        let last_end = line_end_including_newline(source, first_prop.statement_end);
        edits.push(Edit {
            start: first_start,
            end: last_end,
            replacement,
        });
        for (prop, _) in slot_alias_props.iter().skip(1) {
            edits.push(Edit {
                start: line_start(source, prop.statement_start),
                end: line_end_including_newline(source, prop.statement_end),
                replacement: String::new(),
            });
        }
    } else {
        edits.push(Edit {
            start: instance.content_start,
            end: instance.content_start,
            replacement: format!("\n{replacement}"),
        });
    }
    for binding in bindings {
        let indent = leading_whitespace_before(source, binding.statement_start).unwrap_or("\t");
        let normalized = normalize_slot_identifier(&binding.slot_name);
        let replacement = match binding.kind {
            ScriptSlotBindingKind::LocalAlias => {
                format!("{indent}let {} = {normalized};\n", binding.local_name)
            }
            ScriptSlotBindingKind::ReactiveDerived => {
                format!(
                    "{indent}let {} = $derived({normalized});\n",
                    binding.local_name
                )
            }
            ScriptSlotBindingKind::ExportAlias => continue,
        };
        edits.push(Edit {
            start: line_start(source, binding.statement_start),
            end: line_end_including_newline(source, binding.statement_end),
            replacement,
        });
    }
}

fn merge_props_interface_members(
    interface: &PropsTypeDeclaration,
    props: &[SimpleExportProp],
    slot_props: &[SlotPropRequirement],
) -> Vec<PropsTypeMember> {
    let mut members = interface.members.clone();
    let mut seen = interface
        .members
        .iter()
        .map(|member| member.name.clone())
        .collect::<HashSet<_>>();

    for prop in props {
        if seen.contains(&prop.name) {
            continue;
        }
        seen.insert(prop.name.clone());
        members.push(PropsTypeMember {
            name: prop.name.clone(),
            type_source: prop
                .type_source
                .clone()
                .unwrap_or_else(|| String::from("any")),
            optional: prop.default_source.is_some(),
            comment: prop.comment.clone(),
            trailing_comment: prop.trailing_comment.clone(),
        });
    }

    for slot_prop in slot_props {
        if seen.contains(&slot_prop.name) {
            continue;
        }
        seen.insert(slot_prop.name.clone());
        members.push(PropsTypeMember {
            name: slot_prop.name.clone(),
            type_source: if slot_prop.accepts_args {
                String::from("import('svelte').Snippet<[any]>")
            } else {
                String::from("import('svelte').Snippet")
            },
            optional: true,
            comment: None,
            trailing_comment: None,
        });
    }

    members
}

fn render_accessor_exports(props: &[SimpleExportProp], indent: &str) -> String {
    let exports = props
        .iter()
        .map(|prop| format!("{indent}\t{},\n", prop.name))
        .collect::<String>();
    format!("\n{indent}export {{\n{exports}{indent}}}\n")
}

fn props_type_declaration(
    program: &ParsedJsProgram,
    source: &str,
    source_offset: usize,
) -> Option<PropsTypeDeclaration> {
    for statement in &program.program().body {
        match statement {
            Statement::TSInterfaceDeclaration(decl) if decl.id.name.as_str() == "$$Props" => {
                let (start, end) = span_range_with_offset(source_offset, decl.span);
                let members = extract_ts_interface_members(source, source_offset, &decl.body.body);
                return Some(PropsTypeDeclaration {
                    start,
                    end,
                    from_type_alias: false,
                    members,
                });
            }
            Statement::TSTypeAliasDeclaration(decl)
                if decl.id.name.as_str() == "$$Props" =>
            {
                let (start, end) = span_range_with_offset(source_offset, decl.span);
                if let oxc_ast::ast::TSType::TSTypeLiteral(literal) = &decl.type_annotation {
                    let members =
                        extract_ts_interface_members(source, source_offset, &literal.members);
                    return Some(PropsTypeDeclaration {
                        start,
                        end,
                        from_type_alias: true,
                        members,
                    });
                }
            }
            _ => {}
        }
    }
    None
}

fn extract_ts_interface_members(
    source: &str,
    source_offset: usize,
    signatures: &[oxc_ast::ast::TSSignature<'_>],
) -> Vec<PropsTypeMember> {
    use oxc_ast::ast::TSSignature;

    signatures
        .iter()
        .filter_map(|sig| {
            let TSSignature::TSPropertySignature(prop) = sig else {
                return None;
            };
            let name = match &prop.key {
                oxc_ast::ast::PropertyKey::StaticIdentifier(id) => id.name.to_string(),
                _ => return None,
            };
            let (sig_start, sig_end) = span_range_with_offset(source_offset, prop.span);
            let sig_source = source.get(sig_start..sig_end)?;
            // Extract type from the source text after the name and optional `?` and `:`
            let type_source = {
                let colon_pos = sig_source.find(':')?;
                let remainder = &sig_source[colon_pos + 1..];
                let end = remainder.rfind([';', ',']).unwrap_or(remainder.len());
                remainder[..end].trim().to_string()
            };
            let comment = raw_preceding_comment(source, sig_start);
            let trailing_comment = raw_trailing_comment_text(source, sig_end);
            Some(PropsTypeMember {
                name,
                type_source,
                optional: prop.optional,
                comment,
                trailing_comment,
            })
        })
        .collect()
}

fn raw_preceding_comment(source: &str, statement_start: usize) -> Option<String> {
    let (comment_start, cursor) = raw_preceding_comment_range(source, statement_start)?;
    source
        .get(comment_start..cursor)
        .map(str::trim_end)
        .map(ToString::to_string)
}

fn raw_preceding_comment_range(source: &str, statement_start: usize) -> Option<(usize, usize)> {
    let comment_start = raw_preceding_comment_start(source, statement_start)?;
    let mut cursor = statement_start;
    while cursor > 0 && source.as_bytes()[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    Some((comment_start, cursor))
}

fn raw_preceding_comment_start(source: &str, statement_start: usize) -> Option<usize> {
    let mut cursor = statement_start;
    while cursor > 0 && source.as_bytes()[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }

    // Check for `/* ... */` block comment
    if cursor >= 2 && source[..cursor].ends_with("*/") {
        let comment_start = source[..cursor].rfind("/*")?;
        let line = line_start(source, comment_start);
        if source
            .get(line..comment_start)
            .is_some_and(|prefix| prefix.chars().all(char::is_whitespace))
        {
            return Some(comment_start);
        }
    }

    // Check for `// ...` single-line comment on the preceding line
    if cursor > 0 {
        let line = line_start(source, cursor);
        let line_text = &source[line..cursor];
        let trimmed = line_text.trim();
        if trimmed.starts_with("//") {
            return Some(line + line_text.find("//").unwrap_or(0));
        }
    }

    None
}

fn normalize_comment_indent(comment: &str, indent: &str) -> String {
    let mut lines = comment.lines();
    let first = lines
        .next()
        .map(|line| line.strip_prefix(indent).unwrap_or(line));
    first
        .into_iter()
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn reactive_statement_removal_range(
    statement: &Statement<'_>,
    source: &str,
    source_offset: usize,
) -> Option<(usize, usize)> {
    let (statement_start, statement_end) = span_range_with_offset(source_offset, statement.span());
    let start = leading_comment_start_stmt(statement, source, source_offset)
        .or_else(|| raw_preceding_comment_start(source, statement_start))
        .unwrap_or(statement_start);
    let end = trailing_comment_end_stmt(statement, source, source_offset).unwrap_or(statement_end);
    Some((
        line_start(source, start),
        line_end_including_newline(source, end),
    ))
}

fn reactive_statement_replacement_with_comments(
    statement: &Statement<'_>,
    source: &str,
    source_offset: usize,
    indent: &str,
    body: &str,
) -> String {
    let mut output = String::new();
    let (statement_start, statement_end) = span_range_with_offset(source_offset, statement.span());
    if let Some(comment) = raw_preceding_comment(source, statement_start) {
        output.push_str(indent);
        output.push_str(&normalize_comment_indent(&comment, indent));
        output.push('\n');
    }
    output.push_str(indent);
    output.push_str(body);
    // Append trailing comment on the same line (e.g., `// this too`)
    if let Some(trailing) = raw_trailing_comment_text(source, statement_end) {
        output.push(' ');
        output.push_str(&trailing);
    }
    output.push('\n');
    output
}

fn render_ts_prop_members_with_slots(
    props: &[SimpleExportProp],
    slot_props: &[SlotPropRequirement],
    indent: &str,
) -> String {
    let child_indent = format!("{indent}{indent}");
    props
        .iter()
        .map(|prop| {
            let mut line = String::new();
            if let Some(comment) = prop.comment.as_deref() {
                let comment = normalize_comment_indent(comment, indent);
                line.push_str(&child_indent);
                line.push_str(&comment);
                line.push('\n');
            }
            line.push_str(&child_indent);
            line.push_str(&prop.name);
            if prop.default_source.is_some() {
                line.push('?');
            }
            line.push_str(": ");
            line.push_str(prop.type_source.as_deref().unwrap_or("any"));
            line.push(';');
            if let Some(trailing_comment) = prop.trailing_comment.as_deref() {
                let trailing_comment = normalize_comment_indent(trailing_comment, indent);
                line.push(' ');
                line.push_str(&trailing_comment);
            }
            line
        })
        .chain(slot_props.iter().map(|prop| {
            let snippet_type = if prop.accepts_args {
                "Snippet<[any]>"
            } else {
                "Snippet"
            };
            format!(
                "{child_indent}{}?: import('svelte').{snippet_type};",
                prop.name
            )
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_jsdoc_prop_lines_with_slots(
    props: &[SimpleExportProp],
    slot_props: &[SlotPropRequirement],
    indent: &str,
) -> String {
    props
        .iter()
        .map(|prop| {
            let ty = prop.type_source.as_deref().unwrap_or("any");
            let optional = prop.default_source.is_some();
            let prop_name = if optional {
                format!("[{}]", prop.name)
            } else {
                prop.name.clone()
            };
            let mut line = format!("{indent} * @property {{{ty}}} {prop_name}");
            if let Some(description) = jsdoc_prop_description(prop) {
                line.push_str(" - ");
                line.push_str(&description);
            }
            line
        })
        .chain(slot_props.iter().map(|prop| {
            let snippet_type = if prop.accepts_args {
                "Snippet<[any]>"
            } else {
                "Snippet"
            };
            format!(
                "{indent} * @property {{import('svelte').{snippet_type}}} [{}]",
                prop.name
            )
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

fn jsdoc_prop_description(prop: &SimpleExportProp) -> Option<String> {
    let leading = prop
        .comment
        .as_deref()
        .and_then(|comment| summarize_prop_comment(comment, &prop.name));
    let trailing = prop
        .trailing_comment
        .as_deref()
        .and_then(|comment| summarize_prop_comment(comment, &prop.name));
    match (leading, trailing) {
        (Some(leading), Some(trailing)) => Some(format!("{leading} - {trailing}")),
        (Some(leading), None) => Some(leading),
        (None, Some(trailing)) => Some(trailing),
        (None, None) => None,
    }
}

fn summarize_prop_comment(raw: &str, prop_name: &str) -> Option<String> {
    let trimmed = raw.trim();
    let mut text = if let Some(stripped) = trimmed.strip_prefix("//") {
        stripped.trim().to_string()
    } else if trimmed.starts_with("/*") {
        trimmed
            .trim_start_matches("/*")
            .trim_end_matches("*/")
            .lines()
            .filter_map(|line| {
                let line = line.trim().trim_start_matches('*').trim();
                if line.is_empty() {
                    return None;
                }
                if let Some(index) = line.find("@type") {
                    let before = line[..index].trim();
                    let after = text_after_jsdoc_type(&line[index + "@type".len()..]);
                    let combined = [before, after]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    (!combined.is_empty()).then_some(combined)
                } else {
                    Some(line.to_string())
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        trimmed.to_string()
    };

    if !trimmed.starts_with("/*")
        && let Some(index) = text.find("@type")
    {
        text = text_after_jsdoc_type(&text[index + "@type".len()..]).to_string();
    }

    let bracket_default = format!("[{prop_name}=");
    if text.starts_with(&bracket_default) {
        if let Some(end) = text.find(']') {
            text = text[end + 1..].trim_start().to_string();
        }
    } else if text.starts_with(prop_name) {
        text = text[prop_name.len()..].trim_start().to_string();
    }

    text = text
        .trim_start_matches('-')
        .trim_start_matches(':')
        .trim()
        .to_string();

    (!text.is_empty()).then_some(text)
}

fn text_after_jsdoc_type(source: &str) -> &str {
    let Some(open_brace) = source.find('{') else {
        return source.trim();
    };
    let mut depth = 0usize;
    for (offset, ch) in source[open_brace..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[open_brace + offset + 1..].trim();
                }
            }
            _ => {}
        }
    }
    source.trim()
}

fn render_props_interface_source(interface: &PropsTypeDeclaration, indent: &str) -> String {
    let child_indent = format!("{indent}{indent}");
    let members = interface
        .members
        .iter()
        .map(|member| {
            let mut line = String::new();
            if let Some(comment) = member.comment.as_deref() {
                line.push_str(&child_indent);
                line.push_str(comment);
                line.push('\n');
            }
            line.push_str(&child_indent);
            line.push_str(&member.name);
            if member.optional {
                line.push('?');
            }
            line.push_str(": ");
            line.push_str(&member.type_source);
            line.push(';');
            if let Some(trailing_comment) = member.trailing_comment.as_deref() {
                line.push(' ');
                line.push_str(trailing_comment);
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{indent}interface Props {{\n{members}\n{indent}}}")
}

fn collect_basic_props_edits(
    source: &str,
    root: &ModernRoot,
    _use_ts: bool,
    edits: &mut Vec<Edit>,
) {
    let Some(instance) = root.instance.as_ref() else {
        return;
    };
    let use_ts = script_is_typescript(source, instance);
    let bind_targets = prop_bind_targets(root);
    let all_props = collect_simple_export_props(
        &instance.content,
        &bind_targets,
        source,
        instance.content_start,
    );
    let script_slot_bindings =
        collect_script_slot_bindings(&instance.content, source, instance.content_start);
    let props = all_props
        .iter()
        .filter(|prop| {
            prop.default_source
                .as_deref()
                .and_then(slot_reference_name_from_source)
                .is_none()
        })
        .cloned()
        .collect::<Vec<_>>();
    let slot_alias_props = all_props
        .iter()
        .filter_map(|prop| {
            slot_reference_name_from_source(prop.default_source.as_deref()?)
                .map(|slot_name| (prop.clone(), slot_name.to_string()))
        })
        .collect::<Vec<_>>();
    let mut slot_props = collect_slot_placeholder_requirements(source, &root.fragment);
    extend_slot_props_with_script_bindings(
        &mut slot_props,
        &script_slot_bindings,
        source.contains("$$props"),
    );
    if props.is_empty() {
        if !script_slot_bindings.is_empty() || !slot_alias_props.is_empty() {
            collect_script_slot_binding_edits(
                source,
                instance,
                source.contains("$$props"),
                &slot_alias_props,
                &script_slot_bindings,
                &slot_props,
                edits,
            );
        }
        return;
    }
    let accessors = has_svelte_options_accessors(source);
    let props_insert_at = props_insertion_point(
        source,
        instance,
        props
            .iter()
            .map(|prop| props_replacement_insertion_end(source, prop))
            .max()
            .unwrap_or_else(|| props_replacement_insertion_end(source, &props[0])),
    );

    if use_ts && collect_props_interface_edits(source, root, &props, &slot_props, accessors, edits)
    {
        if accessors {
            collect_svelte_options_accessors_edits(source, edits);
        }
        return;
    }

    if source.contains("$$props") {
        if !can_rewrite_simple_rest_props(root, source) {
            return;
        }
        let indent = leading_whitespace_before(source, props[0].statement_start).unwrap_or("\t");

        edits.push(Edit {
            start: line_start(source, props[0].statement_start),
            end: line_end_including_newline(source, props[0].statement_end),
            replacement: format!("{indent}let {{ ...props }} = $props();\n"),
        });
        for prop in props.iter().skip(1) {
            edits.push(Edit {
                start: line_start(source, prop.statement_start),
                end: line_end_including_newline(source, prop.statement_end),
                replacement: String::new(),
            });
        }
        for (start, _) in source.match_indices("$$props") {
            edits.push(Edit {
                start,
                end: start + "$$props".len(),
                replacement: String::from("props"),
            });
        }
        return;
    }

    if props
        .iter()
        .any(|prop| prop.has_init || prop.bindable || (prop.type_source.is_some() && !use_ts))
        && !source.contains("$$restProps")
    {
        let indent = leading_whitespace_before(source, props[0].statement_start).unwrap_or("\t");
        let multiline = should_render_props_multiline(&props);
        let destructured =
            render_destructured_props_with_slots(&props, &slot_props, multiline, indent);
        if use_ts {
            let members = render_ts_prop_members_with_slots(&props, &slot_props, indent);
            let replacement = if multiline {
                format!(
                    "{indent}interface Props {{\n{members}\n{indent}}}\n\n{indent}let {{\n{indent}{indent}{destructured}\n{indent}}}: Props = $props();\n"
                )
            } else {
                format!(
                    "{indent}interface Props {{\n{members}\n{indent}}}\n\n{indent}let {{ {destructured} }}: Props = $props();\n"
                )
            };
            if props_have_comments(&props) {
                replace_props_declarations_preserving_layout(
                    source,
                    &props,
                    replacement,
                    props_insert_at,
                    edits,
                );
            } else {
                replace_props_declarations(source, &props, replacement, props_insert_at, edits);
            }
            if accessors {
                let insertion_point = props
                    .iter()
                    .map(|prop| line_end_including_newline(source, prop.replacement_end))
                    .max()
                    .unwrap_or(props[0].statement_end);
                edits.push(Edit {
                    start: insertion_point,
                    end: insertion_point,
                    replacement: render_accessor_exports(&props, indent),
                });
                collect_svelte_options_accessors_edits(source, edits);
            }
        } else {
            let property_lines = render_jsdoc_prop_lines_with_slots(&props, &slot_props, indent);
            let replacement = if multiline {
                format!(
                    "{indent}/**\n{indent} * @typedef {{Object}} Props\n{property_lines}\n{indent} */\n\n{indent}/** @type {{Props}} */\n{indent}let {{\n{indent}{indent}{destructured}\n{indent}}} = $props();\n"
                )
            } else {
                format!(
                    "{indent}/**\n{indent} * @typedef {{Object}} Props\n{property_lines}\n{indent} */\n\n{indent}/** @type {{Props}} */\n{indent}let {{ {destructured} }} = $props();\n"
                )
            };
            if props_have_comments(&props) {
                replace_props_declarations_preserving_layout(
                    source,
                    &props,
                    replacement,
                    props_insert_at,
                    edits,
                );
            } else {
                replace_props_declarations(source, &props, replacement, props_insert_at, edits);
            }
            if accessors {
                let insertion_point = props
                    .iter()
                    .map(|prop| line_end_including_newline(source, prop.replacement_end))
                    .max()
                    .unwrap_or(props[0].statement_end);
                edits.push(Edit {
                    start: insertion_point,
                    end: insertion_point,
                    replacement: render_accessor_exports(&props, indent),
                });
                collect_svelte_options_accessors_edits(source, edits);
            }
        }
        return;
    }

    if source.contains("$$restProps") {
        let indent = leading_whitespace_before(source, props[0].statement_start).unwrap_or("\t");
        let destructured = render_destructured_props_with_slots(&props, &slot_props, false, indent);
        let replacement = if use_ts {
            let child_indent = format!("{indent}{indent}");
            let members = render_ts_prop_members_with_slots(&props, &slot_props, indent);
            format!(
                "{indent}interface Props {{\n{members}\n{child_indent}[key: string]: any\n{indent}}}\n\n{indent}let {{ {destructured}, ...rest }}: Props = $props();\n"
            )
        } else if props.iter().any(|prop| prop.type_source.is_some()) || !slot_props.is_empty() {
            let property_lines = render_jsdoc_prop_lines_with_slots(&props, &slot_props, indent);
            format!(
                "{indent}/**\n{indent} * @typedef {{Object}} Props\n{property_lines}\n{indent} */\n\n{indent}/** @type {{Props & {{ [key: string]: any }}}} */\n{indent}let {{ {destructured}, ...rest }} = $props();\n"
            )
        } else {
            format!("{indent}let {{ {destructured}, ...rest }} = $props();\n")
        };
        if props_have_comments(&props) {
            replace_props_declarations_preserving_layout(
                source,
                &props,
                replacement,
                props_insert_at,
                edits,
            );
        } else {
            replace_props_declarations(source, &props, replacement, props_insert_at, edits);
        }
        if accessors {
            let insertion_point = props
                .iter()
                .map(|prop| line_end_including_newline(source, prop.replacement_end))
                .max()
                .unwrap_or(props[0].statement_end);
            edits.push(Edit {
                start: insertion_point,
                end: insertion_point,
                replacement: render_accessor_exports(&props, indent),
            });
            collect_svelte_options_accessors_edits(source, edits);
        }
        for (start, _) in source.match_indices("$$restProps") {
            edits.push(Edit {
                start,
                end: start + "$$restProps".len(),
                replacement: String::from("rest"),
            });
        }
        return;
    }

    if use_ts && props.iter().all(|prop| prop.type_source.is_some()) {
        let indent = leading_whitespace_before(source, props[0].statement_start).unwrap_or("\t");
        let members = render_ts_prop_members_with_slots(&props, &slot_props, indent);
        let multiline = should_render_props_multiline(&props);
        let destructured =
            render_destructured_props_with_slots(&props, &slot_props, multiline, indent);
        let replacement = if multiline {
            format!(
                "{indent}interface Props {{\n{members}\n{indent}}}\n\n{indent}let {{\n{indent}{indent}{destructured}\n{indent}}}: Props = $props();\n"
            )
        } else {
            format!(
                "{indent}interface Props {{\n{members}\n{indent}}}\n\n{indent}let {{ {destructured} }}: Props = $props();\n"
            )
        };
        if props_have_comments(&props) {
            replace_props_declarations_preserving_layout(
                source,
                &props,
                replacement,
                props_insert_at,
                edits,
            );
        } else {
            replace_props_declarations(source, &props, replacement, props_insert_at, edits);
        }
        if accessors {
            let insertion_point = props
                .iter()
                .map(|prop| line_end_including_newline(source, prop.replacement_end))
                .max()
                .unwrap_or(props[0].statement_end);
            edits.push(Edit {
                start: insertion_point,
                end: insertion_point,
                replacement: render_accessor_exports(&props, indent),
            });
            collect_svelte_options_accessors_edits(source, edits);
        }
        return;
    }

    let destructured = render_destructured_props_with_slots(&props, &slot_props, false, "");
    let indent = leading_whitespace_before(source, props[0].statement_start).unwrap_or("\t");
    replace_props_declarations(
        source,
        &props,
        format!("{indent}let {{ {destructured} }} = $props();\n"),
        props_insert_at,
        edits,
    );
    if accessors {
        let insertion_point = props
            .iter()
            .map(|prop| line_end_including_newline(source, prop.replacement_end))
            .max()
            .unwrap_or(props[0].statement_end);
        edits.push(Edit {
            start: insertion_point,
            end: insertion_point,
            replacement: render_accessor_exports(&props, indent),
        });
        collect_svelte_options_accessors_edits(source, edits);
    }
}

fn collect_export_specifier_props_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let Some(instance) = root.instance.as_ref() else {
        return;
    };
    let body = &instance.content.program().body;

    let mut export_statement = None::<&oxc_ast::ast::ExportNamedDeclaration<'_>>;
    let mut exported_names = Vec::new();

    for statement in body {
        let Statement::ExportNamedDeclaration(statement) = statement else {
            continue;
        };
        if statement.declaration.is_some() {
            continue;
        }
        let mut names = Vec::new();
        for specifier in &statement.specifiers {
            let Some(local_name) = module_export_name_as_str(&specifier.local) else {
                return;
            };
            let Some(exported_name) = module_export_name_as_str(&specifier.exported) else {
                return;
            };
            if local_name != exported_name {
                return;
            }
            names.push(local_name.to_string());
        }
        export_statement = Some(statement);
        exported_names = names;
        break;
    }

    let Some(export_statement) = export_statement else {
        return;
    };
    if exported_names.is_empty() {
        return;
    }

    let exported_name_set = exported_names.iter().cloned().collect::<HashSet<_>>();
    let mut matched_names = Vec::new();
    let mut declaration_edits = Vec::new();
    let mut last_statement_end = None;

    for statement in body {
        let Statement::VariableDeclaration(statement) = statement else {
            continue;
        };
        if statement.kind.as_str() != "let" {
            continue;
        }
        let (statement_start, statement_end) = span_range_with_offset(instance.content_start, statement.span);

        let mut remaining = Vec::new();
        let mut removed_any = false;

        for declaration in &statement.declarations {
            let Some(id) = declaration.id.get_binding_identifier() else {
                return;
            };
            let name = id.name.as_str();
            let (decl_start, decl_end) = span_range_with_offset(instance.content_start, declaration.span);
            let Some(raw) = source.get(decl_start..decl_end) else {
                return;
            };
            if exported_name_set.contains(name) {
                if declaration.init.is_some() {
                    return;
                }
                matched_names.push(name.to_string());
                removed_any = true;
            } else {
                remaining.push(raw.trim().to_string());
            }
        }

        if !removed_any {
            continue;
        }

        let indent = leading_whitespace_before(source, statement_start).unwrap_or("\t");
        let replacement = if remaining.is_empty() {
            String::new()
        } else {
            format!("{indent}let {};\n", remaining.join(", "))
        };
        declaration_edits.push(Edit {
            start: line_start(source, statement_start),
            end: line_end_including_newline(source, statement_end),
            replacement,
        });
        last_statement_end = Some(statement_end);
    }

    if matched_names.len() != exported_names.len() {
        return;
    }
    let Some(last_statement_end) = last_statement_end else {
        return;
    };
    let (export_start, export_end) = span_range_with_offset(instance.content_start, export_statement.span);

    edits.extend(declaration_edits);

    let indent = leading_whitespace_before(source, export_start).unwrap_or("\t");
    let child_indent = format!("{indent}{indent}");
    let destructured = exported_names.join(&format!(",\n{child_indent}"));
    let export_line_end = line_end_including_newline(source, export_end);
    let trailing_padding = source
        .get(export_line_end..)
        .map(str::trim_start)
        .is_some_and(|tail| tail.starts_with("</script>"));
    edits.push(Edit {
        start: line_end_including_newline(source, last_statement_end),
        end: export_line_end,
        replacement: if trailing_padding {
            format!(
                "{indent}let {{\n{child_indent}{destructured}\n{indent}}} = $props();\n\n{indent}\n"
            )
        } else {
            format!("{indent}let {{\n{child_indent}{destructured}\n{indent}}} = $props();\n\n")
        },
    });
}

fn collect_reactive_assignment_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let Some(instance) = root.instance.as_ref() else {
        return;
    };
    let declared_names = declared_names_in_program(&instance.content);
    let declaration_starts = top_level_declaration_starts(&instance.content, instance.content_start);
    let reactive_rewrites = reactive_binding_rewrites(source, root);
    let reorder_required = reactive_reordering_required(source, root);

    for statement in &instance.content.program().body {
        let Statement::LabeledStatement(labeled_statement) = statement else {
            continue;
        };
        if labeled_statement.label.name.as_str() != "$" {
            continue;
        }
        if let Some(pattern_assignment) =
            reactive_destructuring_assignment(
                statement,
                source,
                instance.content_start,
                &declared_names,
            )
        {
            let indent = leading_whitespace_before(source, pattern_assignment.statement_start)
                .unwrap_or("\t");
            let replacement = format!(
                "{indent}let {} = $derived({}){}\n",
                pattern_assignment.pattern,
                pattern_assignment.rhs,
                if pattern_assignment.has_semicolon {
                    ";"
                } else {
                    ""
                }
            );
            if reorder_required {
                edits.push(Edit {
                    start: line_start(source, pattern_assignment.statement_start),
                    end: line_end_including_newline(source, pattern_assignment.statement_end),
                    replacement: String::new(),
                });
                edits.push(Edit {
                    start: instance.content_end,
                    end: instance.content_end,
                    replacement,
                });
            } else {
                edits.push(Edit {
                    start: line_start(source, pattern_assignment.statement_start),
                    end: line_end_including_newline(source, pattern_assignment.statement_end),
                    replacement,
                });
            }
            continue;
        }
        let Some(reactive_assignment) =
            reactive_single_assignment(statement, source, instance.content_start)
        else {
            continue;
        };
        let name = reactive_assignment.name;
        if name.starts_with('$') {
            continue;
        }

        if let Some(rewrite) = reactive_rewrites.get(name) {
            match rewrite {
                ReactiveBindingRewrite::Derived {
                    rhs,
                    statement_start,
                    statement_end,
                    depends_on_later,
                } => {
                    let Some(binding_statement) =
                        top_level_let_statement_for_name(&instance.content, name, source, instance.content_start)
                    else {
                        continue;
                    };
                    let indent =
                        leading_whitespace_before(source, *statement_start).unwrap_or("\t");
                    let binding_has_semicolon = source
                        .get(binding_statement.start..binding_statement.end)
                        .is_some_and(|statement| statement.trim_end().ends_with(';'));
                    let leading_comments: Vec<String> = reactive_assignment
                        .block_leading_comments
                        .iter()
                        .map(|c| format!("{indent}{c}"))
                        .collect();
                    let mut trailing_comments: Vec<String> = reactive_assignment
                        .block_trailing_comments
                        .iter()
                        .map(|c| format!("{indent}{c}"))
                        .collect();
                    if binding_has_semicolon
                        && !trailing_comments.is_empty()
                        && let Some(last) = trailing_comments.last_mut()
                    {
                        last.push(';');
                    }
                    let declaration_suffix =
                        if binding_has_semicolon && trailing_comments.is_empty() {
                            ";"
                        } else {
                            ""
                        };
                    let has_attached_comments =
                        !leading_comments.is_empty() || !trailing_comments.is_empty();
                    let mut declaration = String::new();
                    for comment in leading_comments {
                        declaration.push_str(&comment);
                        declaration.push('\n');
                    }
                    declaration.push_str(&format!(
                        "{indent}let {} = $derived({rhs}){declaration_suffix}\n",
                        binding_statement.head
                    ));
                    for comment in trailing_comments {
                        declaration.push_str(&comment);
                        declaration.push('\n');
                    }
                    if *depends_on_later {
                        edits.push(Edit {
                            start: line_start(source, binding_statement.start),
                            end: line_end_including_newline(source, binding_statement.end),
                            replacement: String::new(),
                        });
                        edits.push(Edit {
                            start: line_start(source, *statement_start),
                            end: line_end_including_newline(source, *statement_end),
                            replacement: format!("{indent}\n"),
                        });
                        edits.push(Edit {
                            start: instance.content_end,
                            end: instance.content_end,
                            replacement: declaration,
                        });
                    } else {
                        let preserve_blank_line =
                            statement_has_trailing_blank_line(source, *statement_end);
                        let reactive_statement_end = if preserve_blank_line && has_attached_comments
                        {
                            statement_blank_line_end(source, *statement_end)
                        } else {
                            line_end_including_newline(source, *statement_end)
                        };
                        edits.push(Edit {
                            start: line_start(source, binding_statement.start),
                            end: line_end_including_newline(source, binding_statement.end),
                            replacement: declaration,
                        });
                        edits.push(Edit {
                            start: line_start(source, *statement_start),
                            end: reactive_statement_end,
                            replacement: if preserve_blank_line && has_attached_comments {
                                String::from("\n")
                            } else {
                                format!("{indent}\n")
                            },
                        });
                    }
                }
                ReactiveBindingRewrite::StateInit {
                    statement_start,
                    statement_end,
                    ..
                } => {
                    let indent =
                        leading_whitespace_before(source, *statement_start).unwrap_or("\t");
                    edits.push(Edit {
                        start: line_start(source, *statement_start),
                        end: line_end_including_newline(source, *statement_end),
                        replacement: format!("{indent}\n"),
                    });
                }
            }
            continue;
        }

        if !declared_names.contains(name) && reactive_assignment.rhs_is_literal {
            let indent = leading_whitespace_before(source, reactive_assignment.statement_start)
                .unwrap_or("\t");
            let line_end = line_end_including_newline(source, reactive_assignment.statement_end);
            let trailing_padding = source
                .get(line_end..)
                .map(str::trim_start)
                .is_some_and(|tail| tail.starts_with("</script>"));
            edits.push(Edit {
                start: line_start(source, reactive_assignment.statement_start),
                end: line_end,
                replacement: if trailing_padding {
                    format!(
                        "{indent}let {name} = $state({});\n{indent}\n",
                        reactive_assignment.rhs
                    )
                } else {
                    format!(
                        "{indent}let {name} = $state({});\n",
                        reactive_assignment.rhs
                    )
                },
            });
            continue;
        }

        if declared_names.contains(name) || reactive_assignment.rhs_is_literal {
            continue;
        }

        let indent =
            leading_whitespace_before(source, reactive_assignment.statement_start).unwrap_or("\t");
        let depends_on_later = reorder_required
            || rhs_identifier_names(reactive_assignment.right)
                .into_iter()
                .filter_map(|identifier| declaration_starts.get(identifier.as_str()))
                .any(|start| *start > reactive_assignment.statement_start);
        if depends_on_later {
            let (remove_start, remove_end) =
                reactive_statement_removal_range(statement, source, instance.content_start)
                    .unwrap_or((
                        line_start(source, reactive_assignment.statement_start),
                        line_end_including_newline(source, reactive_assignment.statement_end),
                    ));
            edits.push(Edit {
                start: remove_start,
                end: remove_end,
                replacement: String::new(),
            });
            edits.push(Edit {
                start: instance.content_end,
                end: instance.content_end,
                replacement: reactive_statement_replacement_with_comments(
                    statement,
                    source,
                    instance.content_start,
                    indent,
                    &format!(
                        "let {name} = $derived({}){}",
                        reactive_assignment.rhs,
                        if reactive_assignment.has_semicolon {
                            ";"
                        } else {
                            ""
                        }
                    ),
                ),
            });
        } else {
            edits.push(Edit {
                start: line_start(source, reactive_assignment.statement_start),
                end: line_end_including_newline(source, reactive_assignment.statement_end),
                replacement: format!(
                    "{indent}let {name} = $derived({}){}\n",
                    reactive_assignment.rhs,
                    if reactive_assignment.has_semicolon {
                        ";"
                    } else {
                        ""
                    }
                ),
            });
        }
    }
}

fn collect_reactive_state_run_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let Some(instance) = root.instance.as_ref() else {
        return;
    };
    let declared_names = declared_names_in_program(&instance.content);
    let run_name = if declared_names.contains("run") {
        "run_1"
    } else {
        "run"
    };
    let reactive_rewrites = reactive_binding_rewrites(source, root);
    let reorder_required = reactive_reordering_required(source, root);
    let mut needs_run_import = false;

    for statement in &instance.content.program().body {
        let Statement::LabeledStatement(labeled_statement) = statement else {
            continue;
        };
        if labeled_statement.label.name.as_str() != "$" {
            continue;
        }
        let (statement_start, statement_end) =
            span_range_with_offset(instance.content_start, labeled_statement.span);
        let Some(statement_source) = source.get(statement_start..statement_end) else {
            continue;
        };
        if !statement_source.trim_start().starts_with("$:") {
            continue;
        }
        if let Some(reactive_assignment) =
            reactive_single_assignment(statement, source, instance.content_start)
        {
            if reactive_rewrites.contains_key(reactive_assignment.name) {
                continue;
            }
            if !declared_names.contains(reactive_assignment.name)
                && !reactive_assignment.name.starts_with('$')
            {
                continue;
            }
        }
        if reactive_destructuring_assignment(
            statement,
            source,
            instance.content_start,
            &declared_names,
        )
        .is_some()
        {
            continue;
        }
        let (body_start, body_end) =
            span_range_with_offset(instance.content_start, labeled_statement.body.span());

        needs_run_import = true;
        let indent = leading_whitespace_before(source, statement_start).unwrap_or("\t");
        let replacement = if matches!(&labeled_statement.body, Statement::BlockStatement(_)) {
            let Some(body_source) = source.get(body_start + 1..body_end.saturating_sub(1)) else {
                continue;
            };
            let body_source = normalize_reactive_run_body(body_source);
            format!("{indent}{run_name}(() => {{{body_source}}});\n")
        } else {
            let inner_indent = format!("{indent}{indent}");
            let body_source = source
                .get(body_start..body_end)
                .map(str::trim)
                .map(|body| normalize_reactive_run_expression_body(body, indent))
                .unwrap_or_default();
            format!("{indent}{run_name}(() => {{\n{inner_indent}{body_source}\n{indent}}});\n")
        };
        if reorder_required {
            let preserve_trailing_blank_line =
                statement_has_trailing_blank_line(source, statement_end);
            edits.push(Edit {
                start: line_start(source, statement_start),
                end: line_end_including_newline(source, statement_end),
                replacement: String::new(),
            });
            edits.push(Edit {
                start: instance.content_end,
                end: instance.content_end,
                replacement: if preserve_trailing_blank_line {
                    format!("{replacement}{indent}\n")
                } else {
                    replacement
                },
            });
        } else {
            edits.push(Edit {
                start: line_start(source, statement_start),
                end: line_end_including_newline(source, statement_end),
                replacement,
            });
        }
    }

    if needs_run_import
        && !source.contains("from 'svelte/legacy'")
        && !source.contains("from \"svelte/legacy\"")
    {
        let indent = guess_indent(source);
        edits.push(Edit {
            start: instance.content_start,
            end: instance.content_start,
            replacement: if run_name == "run" {
                format!("\n{indent}import {{ run }} from 'svelte/legacy';\n")
            } else {
                format!("\n{indent}import {{ run as {run_name} }} from 'svelte/legacy';\n")
            },
        });
    }
}

fn normalize_reactive_run_body(body_source: &str) -> String {
    body_source.replace("break $;", "return;")
}

fn normalize_reactive_run_expression_body(body_source: &str, indent: &str) -> String {
    normalize_reactive_run_body(body_source).replace('\n', &format!("\n{indent}"))
}

fn collect_export_alias_props_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let Some(instance) = root.instance.as_ref() else {
        return;
    };
    if !script_is_typescript(source, instance) {
        return;
    }
    let body = &instance.content.program().body;

    for (index, statement) in body.iter().enumerate() {
        let Statement::VariableDeclaration(statement) = statement else {
            continue;
        };
        if statement.kind.as_str() != "let" || statement.declarations.len() != 1 {
            continue;
        }
        let declarator = &statement.declarations[0];
        let Some(id) = declarator.id.get_binding_identifier() else {
            continue;
        };
        let local_name = id.name.as_str();
        let Some(init) = declarator.init.as_ref() else {
            continue;
        };
        let Some(init_source) =
            expression_source_from_expression(source, instance.content_start, init)
        else {
            continue;
        };
        let Some(next_statement) = body.get(index + 1) else {
            continue;
        };
        let Statement::ExportNamedDeclaration(next_statement) = next_statement else {
            continue;
        };
        if next_statement.declaration.is_some() || next_statement.specifiers.len() != 1 {
            continue;
        }
        let specifier = &next_statement.specifiers[0];
        if module_export_name_as_str(&specifier.local) != Some(local_name) {
            continue;
        }
        let Some(exported_name) = module_export_name_as_str(&specifier.exported) else {
            continue;
        };
        let Some(type_name) = infer_type_from_default(&init_source) else {
            continue;
        };
        let (statement_start, statement_end) =
            span_range_with_offset(instance.content_start, statement.span);
        let (export_start, export_end) =
            span_range_with_offset(instance.content_start, next_statement.span);
        let indent = leading_whitespace_before(source, statement_start).unwrap_or("\t");
        edits.push(Edit {
            start: line_start(source, statement_start),
            end: line_end_including_newline(source, statement_end),
            replacement: format!(
                "{indent}interface Props {{\n{indent}\t{exported_name}?: {type_name};\n{indent}}}\n\n{indent}let {{ {exported_name}: {local_name} = {init_source} }}: Props = $props();\n{indent}\n"
            ),
        });
        edits.push(Edit {
            start: line_start(source, export_start),
            end: line_end_including_newline(source, export_end),
            replacement: String::new(),
        });
    }
}

fn collect_stateful_let_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let Some(instance) = root.instance.as_ref() else {
        return;
    };
    let updated_names = stateful_names(root);
    let reactive_rewrites = reactive_binding_rewrites(source, root);
    if updated_names.is_empty() {
        return;
    }
    for statement in &instance.content.program().body {
        let Statement::VariableDeclaration(statement) = statement else {
            continue;
        };
        if statement.kind.as_str() != "let" || statement.declarations.len() != 1 {
            continue;
        }
        let declarator = &statement.declarations[0];
        let Some(id) = declarator.id.get_binding_identifier() else {
            continue;
        };
        let name = id.name.as_str();
        if !updated_names.contains(name) {
            continue;
        }
        if matches!(
            reactive_rewrites.get(name),
            Some(ReactiveBindingRewrite::Derived { .. })
        ) {
            continue;
        }
        let (statement_start, statement_end) =
            span_range_with_offset(instance.content_start, statement.span);
        let Some(statement_source) = source.get(statement_start..statement_end) else {
            continue;
        };
        let indent = leading_whitespace_before(source, statement_start).unwrap_or("\t");
        let Some(binding_head) = state_binding_head(statement_source) else {
            continue;
        };
        let has_semicolon = statement_source.trim_end().ends_with(';');
        let init = match reactive_rewrites.get(name) {
            Some(ReactiveBindingRewrite::StateInit { rhs, .. }) => format!("({rhs})"),
            _ => declarator
                .init
                .as_ref()
                .and_then(|init| {
                    expression_source_from_expression(source, instance.content_start, init)
                })
                .map(|init| format!("({init})"))
                .unwrap_or_else(|| String::from("()")),
        };
        edits.push(Edit {
            start: line_start(source, statement_start),
            end: line_end_including_newline(source, statement_end),
            replacement: format!(
                "{indent}let {binding_head} = $state{init}{}\n",
                if has_semicolon { ";" } else { "" }
            ),
        });
    }
}

fn reactive_reordering_required(source: &str, root: &ModernRoot) -> bool {
    let Some(instance) = root.instance.as_ref() else {
        return false;
    };
    let starts = reactive_dependency_starts(source, root);
    instance.content.program().body.iter().any(|statement| {
        let (statement_start, _) = span_range_with_offset(instance.content_start, statement.span());
        reactive_statement_dependencies(statement, source, instance.content_start)
            .into_iter()
            .filter_map(|name| starts.get(name.as_str()))
            .any(|start| *start > statement_start)
    })
}

fn reactive_dependency_starts(source: &str, root: &ModernRoot) -> HashMap<String, usize> {
    let Some(instance) = root.instance.as_ref() else {
        return HashMap::new();
    };
    let mut starts = top_level_declaration_starts(&instance.content, instance.content_start);
    let props_insert_at = props_insertion_point(source, instance, instance.content_start);
    for name in export_let_names(&instance.content) {
        starts.insert(name, props_insert_at);
    }
    let declared_names = declared_names_in_program(&instance.content);
    for statement in &instance.content.program().body {
        let (statement_start, _) = span_range_with_offset(instance.content_start, statement.span());
        if let Some(assignment) =
            reactive_single_assignment(statement, source, instance.content_start)
        {
            starts.insert(assignment.name.to_string(), statement_start);
            continue;
        }
        if let Some(assignment) =
            reactive_destructuring_assignment(
                statement,
                source,
                instance.content_start,
                &declared_names,
            )
        {
            let mut names = HashSet::new();
            if let Statement::LabeledStatement(statement) = statement
                && let Statement::ExpressionStatement(body) = &statement.body
                && let OxcExpression::AssignmentExpression(expression) =
                    unwrap_oxc_parenthesized_expression(&body.expression)
            {
                collect_assignment_target_binding_names(&expression.left, &mut names);
            }
            for name in names {
                starts.insert(name, statement_start);
            }
            let _ = assignment;
        }
    }
    starts
}

fn reactive_statement_dependencies(
    statement: &Statement<'_>,
    source: &str,
    source_offset: usize,
) -> HashSet<String> {
    if let Some(assignment) = reactive_single_assignment(statement, source, source_offset) {
        return rhs_identifier_names(assignment.right);
    }
    let declared_names = HashSet::new();
    if let Some(assignment) = reactive_destructuring_assignment(
        statement,
        source,
        source_offset,
        &declared_names,
    )
    {
        if let Statement::LabeledStatement(statement) = statement
            && let Statement::ExpressionStatement(body) = &statement.body
            && let OxcExpression::AssignmentExpression(expression) =
                unwrap_oxc_parenthesized_expression(&body.expression)
        {
            let right = expression.right.get_inner_expression();
            return rhs_identifier_names(right);
        }
        let _ = assignment;
    }
    // For non-assignment reactive statements (e.g. `$: console.log(mobile)`),
    // collect all identifier references from the body.
    if let Statement::LabeledStatement(labeled) = statement
        && labeled.label.name.as_str() == "$"
    {
        return body_identifier_names(&labeled.body);
    }
    HashSet::new()
}

fn collect_unused_lifecycle_import_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let Some(instance) = root.instance.as_ref() else {
        return;
    };
    for statement in &instance.content.program().body {
        let Statement::ImportDeclaration(import_declaration) = statement else {
            continue;
        };
        if import_declaration.source.value.as_str() != "svelte" {
            continue;
        }
        let Some(specifiers) = import_declaration.specifiers.as_ref() else {
            continue;
        };
        let (statement_start, statement_end) = span_range_with_offset(instance.content_start, import_declaration.span);
        let indent = leading_whitespace_before(source, statement_start).unwrap_or("\t");
        let statement_source = match source.get(statement_start..statement_end) {
            Some(value) => value,
            None => continue,
        };
        let mut updated_source = statement_source.to_string();
        let mut removed = 0usize;
        for specifier in specifiers {
            let name = match specifier {
                oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                    module_export_name_as_str(&specifier.imported)
                }
                _ => None,
            };
            let Some(name) = name else {
                continue;
            };
            if !matches!(name, "beforeUpdate" | "afterUpdate") {
                continue;
            }
            if identifier_occurs(
                &source_without_ranges(source, &[(statement_start, statement_end)]),
                name,
            ) {
                continue;
            }
            updated_source = updated_source.replace(&format!("{name}, "), "");
            updated_source = updated_source.replace(&format!(", {name}"), "");
            updated_source = updated_source.replace(name, "");
            removed += 1;
        }
        if removed == 0 {
            continue;
        }
        let replacement = if updated_source.contains('{') && updated_source.contains('}') {
            if updated_source.contains("{}")
                || updated_source.contains("{  }")
                || updated_source.contains("{ }")
            {
                format!("{indent}\n")
            } else {
                format!("{indent}{}\n", updated_source.trim())
            }
        } else {
            format!("{indent}\n")
        };
        edits.push(Edit {
            start: line_start(source, statement_start),
            end: line_end_including_newline(source, statement_end),
            replacement,
        });
    }
}

const EVENT_MODIFIER_ORDER: [&str; 6] = [
    "preventDefault",
    "stopPropagation",
    "stopImmediatePropagation",
    "self",
    "trusted",
    "once",
];

const EVENT_LEGACY_IMPORT_ORDER: [&str; 10] = [
    "createBubbler",
    "handlers",
    "preventDefault",
    "stopPropagation",
    "stopImmediatePropagation",
    "self",
    "trusted",
    "once",
    "passive",
    "nonpassive",
];

impl EventHandlerNames {
    fn new(declared_names: &HashSet<String>) -> Self {
        let mut used_names = declared_names.clone();
        let create_bubbler = unique_generated_name("createBubbler", &mut used_names);
        let handlers = unique_generated_name("handlers", &mut used_names);
        let prevent_default = unique_generated_name("preventDefault", &mut used_names);
        let stop_propagation = unique_generated_name("stopPropagation", &mut used_names);
        let stop_immediate_propagation =
            unique_generated_name("stopImmediatePropagation", &mut used_names);
        let self_name = unique_generated_name("self", &mut used_names);
        let trusted = unique_generated_name("trusted", &mut used_names);
        let once = unique_generated_name("once", &mut used_names);
        let passive = unique_generated_name("passive", &mut used_names);
        let nonpassive = unique_generated_name("nonpassive", &mut used_names);
        let bubble = unique_generated_name("bubble", &mut used_names);

        Self {
            create_bubbler,
            handlers,
            prevent_default,
            stop_propagation,
            stop_immediate_propagation,
            self_name,
            trusted,
            once,
            passive,
            nonpassive,
            bubble,
        }
    }

    fn import_name(&self, import_name: &str) -> &str {
        match import_name {
            "createBubbler" => &self.create_bubbler,
            "handlers" => &self.handlers,
            "preventDefault" => &self.prevent_default,
            "stopPropagation" => &self.stop_propagation,
            "stopImmediatePropagation" => &self.stop_immediate_propagation,
            "self" => &self.self_name,
            "trusted" => &self.trusted,
            "once" => &self.once,
            "passive" => &self.passive,
            "nonpassive" => &self.nonpassive,
            _ => unreachable!("unknown legacy event import: {import_name}"),
        }
    }
}

fn collect_event_handler_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let declared_names = root
        .instance
        .as_ref()
        .map(|instance| declared_names_in_program(&instance.content))
        .unwrap_or_default();
    let names = EventHandlerNames::new(&declared_names);
    let mut state = EventHandlerMigrationState::default();
    collect_event_handler_fragment_edits(source, &root.fragment, &names, &mut state, edits);
    collect_event_handler_script_edits(source, root, &names, &state, edits);
}

fn collect_event_handler_fragment_edits(
    source: &str,
    fragment: &Fragment,
    names: &EventHandlerNames,
    state: &mut EventHandlerMigrationState,
    edits: &mut Vec<Edit>,
) {
    for node in fragment.nodes.iter() {
        collect_event_handler_node_edits(source, node, names, state, edits);
    }
}

fn collect_event_handler_node_edits(
    source: &str,
    node: &Node,
    names: &EventHandlerNames,
    state: &mut EventHandlerMigrationState,
    edits: &mut Vec<Edit>,
) {
    if let Some((attributes, fragment)) = event_handler_target(node) {
        collect_element_event_handler_edits(source, attributes, names, state, edits);
        collect_event_handler_fragment_edits(source, fragment, names, state, edits);
        return;
    }

    node.for_each_child_fragment(|fragment| {
        collect_event_handler_fragment_edits(source, fragment, names, state, edits);
    });
}

fn event_handler_target(node: &Node) -> Option<(&[Attribute], &Fragment)> {
    match node {
        Node::RegularElement(element) => Some((&element.attributes, &element.fragment)),
        Node::SvelteElement(element) => Some((&element.attributes, &element.fragment)),
        Node::SvelteWindow(window) => Some((&window.attributes, &window.fragment)),
        Node::SvelteDocument(document) => Some((&document.attributes, &document.fragment)),
        Node::SvelteBody(body) => Some((&body.attributes, &body.fragment)),
        _ => None,
    }
}

fn collect_element_event_handler_edits(
    source: &str,
    attributes: &[Attribute],
    names: &EventHandlerNames,
    state: &mut EventHandlerMigrationState,
    edits: &mut Vec<Edit>,
) {
    let mut groups = Vec::<(String, Vec<&crate::ast::modern::DirectiveAttribute>)>::new();

    for attribute in attributes {
        let Attribute::OnDirective(directive) = attribute else {
            continue;
        };

        let mut event_name = format!("on{}", directive.name);
        if directive
            .modifiers
            .iter()
            .any(|modifier| modifier.as_ref() == "capture")
        {
            event_name.push_str("capture");
        }

        if let Some((_, group)) = groups.iter_mut().find(|(name, _)| *name == event_name) {
            group.push(directive);
        } else {
            groups.push((event_name, vec![directive]));
        }
    }

    for (event_name, directives) in groups {
        let mut handler_bodies = Vec::new();
        let mut first_handler = None;

        for directive in directives {
            let mut body = if let Some(expression_source) =
                event_handler_expression_source(source, directive)
            {
                expression_source
            } else {
                state.used_imports.insert("createBubbler");
                state.needs_bubble = true;
                format!("{}('{}')", names.bubble, directive.name)
            };

            for modifier in EVENT_MODIFIER_ORDER {
                if directive
                    .modifiers
                    .iter()
                    .any(|current| current.as_ref() == modifier)
                {
                    state.used_imports.insert(modifier);
                    body = format!("{}({body})", names.import_name(modifier));
                }
            }

            let has_passive = directive
                .modifiers
                .iter()
                .any(|modifier| modifier.as_ref() == "passive");
            let has_nonpassive = directive
                .modifiers
                .iter()
                .any(|modifier| modifier.as_ref() == "nonpassive");

            if has_passive || has_nonpassive {
                let action = if has_passive { "passive" } else { "nonpassive" };
                state.used_imports.insert(action);
                edits.push(Edit {
                    start: directive.start,
                    end: directive.end,
                    replacement: format!(
                        "use:{}={{['{}', () => {body}]}}",
                        names.import_name(action),
                        directive.name
                    ),
                });
                continue;
            }

            if first_handler.is_some() {
                let mut start = directive.start;
                while start > 0
                    && source
                        .as_bytes()
                        .get(start - 1)
                        .is_some_and(u8::is_ascii_whitespace)
                {
                    start -= 1;
                }
                edits.push(Edit {
                    start,
                    end: directive.end,
                    replacement: String::new(),
                });
            } else {
                first_handler = Some(directive);
            }

            handler_bodies.push(body);
        }

        if let Some(first_handler) = first_handler {
            let replacement = if handler_bodies.len() > 1 {
                state.used_imports.insert("handlers");
                format!(
                    "{event_name}={{{}({})}}",
                    names.import_name("handlers"),
                    handler_bodies.join(", ")
                )
            } else {
                format!("{event_name}={{{}}}", handler_bodies.join(", "))
            };
            edits.push(Edit {
                start: first_handler.start,
                end: first_handler.end,
                replacement,
            });
        }
    }
}

fn event_handler_expression_source(
    source: &str,
    directive: &crate::ast::modern::DirectiveAttribute,
) -> Option<String> {
    if directive
        .expression
        .identifier_name()
        .is_some_and(|name| name.is_empty())
    {
        return None;
    }

    let start = expression_start(&directive.expression)?;
    let end = expression_end(&directive.expression)?;
    if start >= end {
        return None;
    }
    let text = source.get(start..end)?.to_string();
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

fn collect_event_handler_script_edits(
    source: &str,
    root: &ModernRoot,
    names: &EventHandlerNames,
    state: &EventHandlerMigrationState,
    edits: &mut Vec<Edit>,
) {
    if state.used_imports.is_empty() {
        return;
    }

    let import_specifiers = EVENT_LEGACY_IMPORT_ORDER
        .iter()
        .copied()
        .filter(|import_name| state.used_imports.contains(import_name))
        .map(|import_name| {
            let alias = names.import_name(import_name);
            if alias == import_name {
                import_name.to_string()
            } else {
                format!("{import_name} as {alias}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let indent = guess_indent(source);

    if let Some(instance) = root.instance.as_ref() {
        let mut replacement =
            format!("\n{indent}import {{ {import_specifiers} }} from 'svelte/legacy';");
        if state.needs_bubble {
            replacement.push_str(&format!(
                "\n\n{indent}const {} = {}();",
                names.bubble, names.create_bubbler
            ));
        } else {
            replacement.push('\n');
        }
        edits.push(Edit {
            start: instance.content_start,
            end: instance.content_start,
            replacement,
        });
        return;
    }

    let mut replacement =
        format!("<script>\n{indent}import {{ {import_specifiers} }} from 'svelte/legacy';");
    if state.needs_bubble {
        replacement.push_str(&format!(
            "\n\n{indent}const {} = {}();",
            names.bubble, names.create_bubbler
        ));
    }
    replacement.push_str("\n</script>\n\n");

    let start = root
        .module
        .as_ref()
        .map(|script| line_end_including_newline(source, script.end))
        .unwrap_or(0);
    edits.push(Edit {
        start,
        end: start,
        replacement,
    });
}

fn collect_css_selector_migration_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    if !root.styles.is_empty() {
        for style in root.styles.iter() {
            collect_single_css_selector_migration_edits(source, style, edits);
        }
    } else if let Some(style) = root.css.as_ref() {
        collect_single_css_selector_migration_edits(source, style, edits);
    }
}

fn collect_single_css_selector_migration_edits(
    source: &str,
    style: &crate::ast::modern::Css,
    edits: &mut Vec<Edit>,
) {
    let Some(css_source) = source.get(style.content.start..style.content.end) else {
        return;
    };

    let mut starting = 0usize;
    while starting < css_source.len() {
        let code = &css_source[starting..];
        if !(code.starts_with(":has")
            || code.starts_with(":is")
            || code.starts_with(":where")
            || code.starts_with(":not"))
        {
            starting += 1;
            continue;
        }

        let Some(open_paren) = code.find('(') else {
            starting += 1;
            continue;
        };
        let mut inner_start = open_paren + 1;
        let mut is_global = false;
        let next_global = code.find(":global");
        let between = next_global
            .and_then(|next_global| code.get(inner_start..next_global))
            .unwrap_or_else(|| code.get(..inner_start).unwrap_or_default());

        if next_global.is_some() && between.trim().is_empty() {
            is_global = true;
            inner_start += ":global".len();
        } else if let Some(prev_global) = css_source
            .get(..starting)
            .and_then(|head| head.rfind(":global"))
            && let Some(global_open) = css_source
                .get(prev_global..)
                .and_then(|tail| tail.find('('))
                .map(|offset| prev_global + offset + 1)
            && let Some(global_end) = find_closing_parenthesis(global_open, css_source)
            && global_end.saturating_sub(starting) > inner_start
        {
            starting = global_end;
            continue;
        }

        let Some(inner_end) = find_closing_parenthesis(starting + inner_start, css_source) else {
            starting += 1;
            continue;
        };
        if !is_global && !code.starts_with(":not") {
            let absolute_start = style.content.start + starting + inner_start;
            let absolute_end = style.content.start + inner_end.saturating_sub(1);
            let Some(inner_source) = source.get(absolute_start..absolute_end) else {
                starting = inner_end;
                continue;
            };
            edits.push(Edit {
                start: absolute_start,
                end: absolute_end,
                replacement: format!(":global({inner_source})"),
            });
        }
        starting = inner_end;
    }
}

fn find_closing_parenthesis(start: usize, source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = start;
    let mut depth = 1usize;

    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

fn collect_dynamic_svelte_component_edits_with_state(
    source: &str,
    root: &ModernRoot,
    state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) {
    let mut path = Vec::new();
    collect_dynamic_svelte_component_fragment_edits(
        source,
        &root.fragment,
        &mut path,
        state,
        edits,
    );
    collect_dynamic_svelte_component_script_edits(source, root, state, edits);
}

fn generated_svelte_component_names_in_source(source: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("{@const ") {
            if let Some((name, _)) = rest.split_once(" = ")
                && name.starts_with("SvelteComponent")
            {
                names.insert(name.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("const ")
            && let Some((name, _)) = rest.split_once(" = ")
            && name.starts_with("SvelteComponent")
        {
            names.insert(name.to_string());
        }
    }
    names
}

fn collect_dynamic_svelte_component_fragment_edits(
    source: &str,
    fragment: &Fragment,
    path: &mut Vec<SvelteComponentPathEntry>,
    state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) {
    for node in fragment.nodes.iter() {
        collect_dynamic_svelte_component_node_edits(source, node, path, state, edits);
    }
}

fn collect_dynamic_svelte_component_node_edits(
    source: &str,
    node: &Node,
    path: &mut Vec<SvelteComponentPathEntry>,
    state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) {
    path.push(SvelteComponentPathEntry {
        kind: svelte_component_scope_kind(node),
        start: node.start(),
        skip_dynamic_children: dynamic_children_rewritten_by_structure(node),
    });

    if path
        .iter()
        .take(path.len().saturating_sub(1))
        .any(|entry| entry.skip_dynamic_children)
    {
        path.pop();
        return;
    }

    if let Node::SvelteComponent(component) = node {
        migrate_dynamic_svelte_component(source, component, path, state, edits);
    }

    node.for_each_child_fragment(|fragment| {
        collect_dynamic_svelte_component_fragment_edits(source, fragment, path, state, edits);
    });

    path.pop();
}

fn dynamic_children_rewritten_by_structure(node: &Node) -> bool {
    match node {
        Node::Component(component) => {
            attributes_have_slot_usage(&component.attributes)
                || fragment_has_slot_usage(&component.fragment)
        }
        Node::SvelteComponent(component) => {
            attributes_have_slot_usage(&component.attributes)
                || fragment_has_slot_usage(&component.fragment)
        }
        _ => false,
    }
}

fn svelte_component_scope_kind(node: &Node) -> Option<SvelteComponentScopeKind> {
    match node {
        Node::IfBlock(_) => Some(SvelteComponentScopeKind::IfBlock),
        Node::EachBlock(_) => Some(SvelteComponentScopeKind::EachBlock),
        Node::AwaitBlock(_) => Some(SvelteComponentScopeKind::AwaitBlock),
        Node::SnippetBlock(_) => Some(SvelteComponentScopeKind::SnippetBlock),
        Node::Component(_) => Some(SvelteComponentScopeKind::Component),
        Node::SvelteComponent(_) => Some(SvelteComponentScopeKind::SvelteComponent),
        _ => None,
    }
}

fn migrate_dynamic_svelte_component(
    source: &str,
    component: &crate::ast::modern::SvelteComponent,
    path: &[SvelteComponentPathEntry],
    state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) {
    let Some(expression) = component.expression.as_ref() else {
        return;
    };
    let Some(raw_expression) = source
        .get(expression.start..expression.end)
        .map(str::to_string)
    else {
        return;
    };

    if expression.identifier_name().is_some()
        && is_static_component_identifier(raw_expression.trim())
    {
        return;
    }

    let migrated_expression = migrate_svelte_component_expression(&raw_expression, state);
    let replacement_name =
        if is_direct_component_member_expression(expression, &migrated_expression) {
            migrated_expression.clone()
        } else {
            if let Some(alias) = scoped_svelte_component_alias(
                source,
                component,
                &migrated_expression,
                path,
                state,
                edits,
            ) {
                alias
            } else {
                // Match the JS migrator's scope generation behavior: it reserves a
                // fresh `SvelteComponent_*` name before deciding whether an existing
                // derived alias can be reused.
                let generated_alias =
                    unique_generated_name("SvelteComponent", &mut state.used_names);
                if let Some(alias) = state.derived_component_names.get(&migrated_expression) {
                    alias.clone()
                } else {
                    state
                        .derived_component_names
                        .insert(migrated_expression.clone(), generated_alias.clone());
                    state
                        .derived_components
                        .push((migrated_expression.clone(), generated_alias.clone()));
                    generated_alias
                }
            }
        };

    replace_svelte_component_tag_name(source, component, &replacement_name, edits);
    remove_svelte_component_this_attribute(source, component, edits);
}

fn render_svelte_component_tag(
    source: &str,
    component: &crate::ast::modern::SvelteComponent,
    state: &mut SvelteComponentMigrationState,
) -> Option<RenderedSvelteComponentTag> {
    let expression = component.expression.as_ref()?;
    let raw_expression = source.get(expression.start..expression.end)?.to_string();
    let raw_expression = raw_expression.trim().to_string();

    if is_static_component_identifier(&raw_expression) {
        return Some(RenderedSvelteComponentTag {
            name: raw_expression,
            prelude: None,
        });
    }

    let migrated_expression = migrate_svelte_component_expression(&raw_expression, state);
    if is_direct_component_member_expression(expression, &migrated_expression) {
        return Some(RenderedSvelteComponentTag {
            name: migrated_expression,
            prelude: None,
        });
    }

    let alias = unique_generated_name("SvelteComponent", &mut state.used_names);
    let indent = line_indent_at(source, component.start).unwrap_or("");
    Some(RenderedSvelteComponentTag {
        name: alias.clone(),
        prelude: Some(format!(
            "{{@const {alias} = {migrated_expression}}}\n{indent}"
        )),
    })
}

fn scoped_svelte_component_alias(
    source: &str,
    component: &crate::ast::modern::SvelteComponent,
    expression: &str,
    path: &[SvelteComponentPathEntry],
    state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) -> Option<String> {
    let scope_index = path
        .iter()
        .enumerate()
        .rev()
        .skip(1)
        .find_map(|(index, entry)| entry.kind.map(|_| index))?;
    let alias = unique_generated_name("SvelteComponent", &mut state.used_names);
    let insertion_start = path
        .get(scope_index + 1)
        .map(|entry| entry.start)
        .unwrap_or(component.start);
    let indent = line_indent_at(source, insertion_start).unwrap_or("");
    edits.push(Edit {
        start: insertion_start,
        end: insertion_start,
        replacement: format!("{{@const {alias} = {expression}}}\n{indent}"),
    });
    Some(alias)
}

fn migrate_svelte_component_expression(
    expression: &str,
    state: &mut SvelteComponentMigrationState,
) -> String {
    if expression.contains("$$restProps") {
        state.needs_rest_props = true;
        expression.replace("$$restProps", "rest")
    } else {
        expression.to_string()
    }
}

fn is_static_component_identifier(name: &str) -> bool {
    is_valid_identifier(name)
        && name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
}

fn is_direct_component_member_expression(
    expression: &crate::ast::modern::Expression,
    expression_source: &str,
) -> bool {
    expression
        .oxc_expression()
        .is_some_and(|expression| expression.is_member_expression())
        && !expression_source.contains('[')
}

fn replace_svelte_component_tag_name(
    source: &str,
    component: &crate::ast::modern::SvelteComponent,
    replacement_name: &str,
    edits: &mut Vec<Edit>,
) {
    let Some(open_name_end) = component.name.len().checked_add(component.start + 1) else {
        return;
    };
    edits.push(Edit {
        start: component.start + 1,
        end: open_name_end,
        replacement: replacement_name.to_string(),
    });

    if let Some(close_start) = source
        .get(component.start..component.end)
        .and_then(|raw| raw.rfind("</"))
        .map(|offset| component.start + offset + 2)
    {
        edits.push(Edit {
            start: close_start,
            end: close_start + component.name.len(),
            replacement: replacement_name.to_string(),
        });
    }
}

fn remove_svelte_component_this_attribute(
    source: &str,
    component: &crate::ast::modern::SvelteComponent,
    edits: &mut Vec<Edit>,
) {
    let Some(expression) = component.expression.as_ref() else {
        return;
    };
    let Some(expression_start) = expression_start(expression) else {
        return;
    };
    let Some(expression_end) = expression_end(expression) else {
        return;
    };
    let search_start = component.start.min(expression_start);
    let Some(relative_this) = source
        .get(search_start..expression_start)
        .and_then(|slice| slice.rfind("this"))
    else {
        return;
    };
    let mut start = search_start + relative_this;
    while start > component.start
        && source
            .as_bytes()
            .get(start - 1)
            .is_some_and(u8::is_ascii_whitespace)
    {
        start -= 1;
    }
    let Some(relative_end) = source
        .get(expression_end..component.end)
        .and_then(|slice| slice.find('}'))
    else {
        return;
    };
    edits.push(Edit {
        start,
        end: expression_end + relative_end + 1,
        replacement: String::new(),
    });
}

fn collect_dynamic_svelte_component_script_edits(
    source: &str,
    root: &ModernRoot,
    state: &SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) {
    if state.derived_components.is_empty() && !state.needs_rest_props {
        return;
    }

    let indent = guess_indent(source);
    let derived_lines = state
        .derived_components
        .iter()
        .map(|(expression, alias)| format!("{indent}const {alias} = $derived({expression});"))
        .collect::<Vec<_>>()
        .join("\n");

    if let Some(instance) = root.instance.as_ref() {
        if state.needs_rest_props {
            edits.push(Edit {
                start: instance.content_start,
                end: instance.content_start,
                replacement: format!(
                    "\n{indent}/** @type {{{{ [key: string]: any }}}} */\n{indent}let {{ ...rest }} = $props();"
                ),
            });
        }
        if !derived_lines.is_empty() {
            edits.push(Edit {
                start: instance.content_end,
                end: instance.content_end,
                replacement: format!("\n{derived_lines}\n"),
            });
        }
        return;
    }

    let mut script_body = String::new();
    if state.needs_rest_props {
        script_body.push_str(&format!(
            "{indent}/** @type {{{{ [key: string]: any }}}} */\n{indent}let {{ ...rest }} = $props();"
        ));
    }
    if !derived_lines.is_empty() {
        if !script_body.is_empty() {
            script_body.push_str("\n\n");
        }
        script_body.push_str(&derived_lines);
    }
    edits.push(Edit {
        start: root
            .module
            .as_ref()
            .map(|script| line_end_including_newline(source, script.end))
            .unwrap_or(0),
        end: root
            .module
            .as_ref()
            .map(|script| line_end_including_newline(source, script.end))
            .unwrap_or(0),
        replacement: format!("<script>\n{script_body}\n</script>\n\n"),
    });
}

fn collect_slot_usage_edits(source: &str, root: &ModernRoot, use_ts: bool, edits: &mut Vec<Edit>) {
    if root
        .options
        .as_ref()
        .and_then(|options| options.custom_element.as_ref())
        .is_some()
    {
        return;
    }
    let use_rest_props = source.contains("$$props")
        && !root
            .instance
            .as_ref()
            .is_some_and(|instance| program_has_export_let(&instance.content));
    let has_script_slot_bindings = root.instance.as_ref().is_some_and(|instance| {
        !collect_script_slot_bindings(&instance.content, source, instance.content_start).is_empty()
    });
    let mut slot_props = HashMap::new();
    let mut derived_aliases = HashMap::new();
    let mut slot_usage_edit_state = SlotUsageEditState {
        slot_props: &mut slot_props,
        derived_aliases: &mut derived_aliases,
        edits,
    };
    collect_slot_usage_fragment_edits(
        source,
        &root.fragment,
        false,
        use_rest_props,
        &SlotUsageContext::default(),
        &mut slot_usage_edit_state,
    );
    collect_slot_reference_fragment_edits(source, &root.fragment, use_rest_props, edits);
    if use_rest_props {
        for (start, _) in source.match_indices("$$props") {
            edits.push(Edit {
                start,
                end: start + "$$props".len(),
                replacement: String::from("props"),
            });
        }
    }
    if slot_props.is_empty() {
        return;
    }
    if use_rest_props && has_script_slot_bindings {
        return;
    }

    collect_slot_prop_prelude_edits(
        source,
        root,
        use_ts,
        use_rest_props,
        &slot_props,
        &derived_aliases,
        edits,
    );
}

fn collect_slot_placeholder_requirements(
    source: &str,
    fragment: &Fragment,
) -> Vec<SlotPropRequirement> {
    let mut slot_props = HashMap::new();
    collect_slot_placeholder_requirement_fragment(source, fragment, &mut slot_props);
    let mut slot_props = slot_props.into_values().collect::<Vec<_>>();
    slot_props.sort_by_key(|prop| prop.order);
    slot_props
}

fn collect_slot_placeholder_requirement_fragment(
    source: &str,
    fragment: &Fragment,
    slot_props: &mut HashMap<String, SlotPropRequirement>,
) {
    for node in fragment.nodes.iter() {
        collect_slot_placeholder_requirement_node(source, node, slot_props);
    }
}

fn collect_slot_placeholder_requirement_node(
    source: &str,
    node: &Node,
    slot_props: &mut HashMap<String, SlotPropRequirement>,
) {
    match node {
        Node::SlotElement(slot) => {
            if let Some(slot_name) = Some(normalize_slot_identifier(
                slot_element_name(slot).unwrap_or("default"),
            )) {
                let args = slot_render_argument_source(source, &slot.attributes)
                    .or_else(|| Some(String::new()))
                    .unwrap_or_default();
                let accepts_args = slot_name != "children" && !args.is_empty();
                let next_order = slot_props.len();
                match slot_props.entry(slot_name.clone()) {
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        entry.get_mut().accepts_args |= accepts_args;
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(SlotPropRequirement {
                            name: slot_name,
                            accepts_args,
                            order: next_order,
                        });
                    }
                }
            }
        }
        Node::TitleElement(title) => {
            collect_slot_placeholder_requirement_fragment(source, &title.fragment, slot_props);
        }
        _ => node.for_each_child_fragment(|fragment| {
            collect_slot_placeholder_requirement_fragment(source, fragment, slot_props);
        }),
    }
}

fn collect_slot_reference_fragment_edits(
    source: &str,
    fragment: &Fragment,
    use_rest_props: bool,
    edits: &mut Vec<Edit>,
) {
    for node in fragment.nodes.iter() {
        collect_slot_reference_node_edits(source, node, use_rest_props, edits);
    }
}

fn collect_slot_reference_node_edits(
    source: &str,
    node: &Node,
    use_rest_props: bool,
    edits: &mut Vec<Edit>,
) {
    if let Some(element) = node.as_element() {
        collect_slot_reference_attribute_edits(source, element.attributes(), use_rest_props, edits);
    }

    match node {
        Node::IfBlock(block) => {
            collect_slot_reference_expression_edit(&block.test, use_rest_props, edits);
        }
        Node::EachBlock(block) => {
            collect_slot_reference_expression_edit(&block.expression, use_rest_props, edits);
            if let Some(context) = block.context.as_ref() {
                collect_slot_reference_expression_edit(context, use_rest_props, edits);
            }
            if let Some(key) = block.key.as_ref() {
                collect_slot_reference_expression_edit(key, use_rest_props, edits);
            }
        }
        Node::KeyBlock(block) => {
            collect_slot_reference_expression_edit(&block.expression, use_rest_props, edits);
        }
        Node::AwaitBlock(block) => {
            collect_slot_reference_expression_edit(&block.expression, use_rest_props, edits);
            if let Some(value) = block.value.as_ref() {
                collect_slot_reference_expression_edit(value, use_rest_props, edits);
            }
            if let Some(error) = block.error.as_ref() {
                collect_slot_reference_expression_edit(error, use_rest_props, edits);
            }
        }
        Node::SnippetBlock(block) => {
            collect_slot_reference_expression_edit(&block.expression, use_rest_props, edits);
            for parameter in block.parameters.iter() {
                collect_slot_reference_expression_edit(parameter, use_rest_props, edits);
            }
        }
        Node::SvelteComponent(component) => {
            if let Some(expression) = component.expression.as_ref() {
                collect_slot_reference_expression_edit(expression, use_rest_props, edits);
            }
        }
        Node::SvelteElement(element) => {
            if let Some(expression) = element.expression.as_ref() {
                collect_slot_reference_expression_edit(expression, use_rest_props, edits);
            }
        }
        Node::ExpressionTag(tag) => {
            collect_slot_reference_expression_edit(&tag.expression, use_rest_props, edits)
        }
        Node::HtmlTag(tag) => {
            collect_slot_reference_expression_edit(&tag.expression, use_rest_props, edits)
        }
        Node::RenderTag(tag) => {
            collect_slot_reference_expression_edit(&tag.expression, use_rest_props, edits)
        }
        Node::ConstTag(tag) => {
            collect_slot_reference_expression_edit(&tag.declaration, use_rest_props, edits)
        }
        Node::Comment(_) | Node::Text(_) | Node::DebugTag(_) => {}
        _ => {}
    }

    node.for_each_child_fragment(|fragment| {
        collect_slot_reference_fragment_edits(source, fragment, use_rest_props, edits);
    });
}

fn collect_slot_reference_attribute_edits(
    _source: &str,
    attributes: &[Attribute],
    use_rest_props: bool,
    edits: &mut Vec<Edit>,
) {
    for attribute in attributes {
        match attribute {
            Attribute::Attribute(attribute) => match &attribute.value {
                AttributeValueList::ExpressionTag(tag) => {
                    collect_slot_reference_expression_edit(&tag.expression, use_rest_props, edits);
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_slot_reference_expression_edit(
                                &tag.expression,
                                use_rest_props,
                                edits,
                            );
                        }
                    }
                }
                AttributeValueList::Boolean(_) => {}
            },
            Attribute::BindDirective(directive)
            | Attribute::OnDirective(directive)
            | Attribute::ClassDirective(directive)
            | Attribute::LetDirective(directive)
            | Attribute::AnimateDirective(directive)
            | Attribute::UseDirective(directive) => {
                collect_slot_reference_expression_edit(
                    &directive.expression,
                    use_rest_props,
                    edits,
                );
            }
            Attribute::StyleDirective(directive) => match &directive.value {
                AttributeValueList::ExpressionTag(tag) => {
                    collect_slot_reference_expression_edit(&tag.expression, use_rest_props, edits);
                }
                AttributeValueList::Values(values) => {
                    for value in values.iter() {
                        if let AttributeValue::ExpressionTag(tag) = value {
                            collect_slot_reference_expression_edit(
                                &tag.expression,
                                use_rest_props,
                                edits,
                            );
                        }
                    }
                }
                AttributeValueList::Boolean(_) => {}
            },
            Attribute::TransitionDirective(directive) => {
                collect_slot_reference_expression_edit(
                    &directive.expression,
                    use_rest_props,
                    edits,
                );
            }
            Attribute::SpreadAttribute(attribute) => {
                collect_slot_reference_expression_edit(
                    &attribute.expression,
                    use_rest_props,
                    edits,
                );
            }
            Attribute::AttachTag(tag) => {
                collect_slot_reference_expression_edit(&tag.expression, use_rest_props, edits)
            }
        }
    }
}

fn collect_slot_reference_expression_edit(
    expression: &crate::ast::modern::Expression,
    use_rest_props: bool,
    edits: &mut Vec<Edit>,
) {
    if let Some(edit) = migrate_slot_reference_expression(expression, use_rest_props) {
        edits.push(edit);
    }
}

fn migrate_slot_reference_expression(
    expression: &crate::ast::modern::Expression,
    use_rest_props: bool,
) -> Option<Edit> {
    let slot_name = script_slot_reference_name(expression.oxc_expression()?)?;

    Some(Edit {
        start: expression_start(expression)?,
        end: expression_end(expression)?,
        replacement: slot_prop_reference(&slot_name, use_rest_props),
    })
}

fn slot_prop_reference(slot_name: &str, use_rest_props: bool) -> String {
    let normalized = normalize_slot_identifier(slot_name);
    if use_rest_props {
        format!("props.{normalized}")
    } else {
        normalized
    }
}

#[derive(Debug)]
struct SlotRenderedSegment {
    prelude: String,
    rendered: String,
    is_named_slot: bool,
}

#[derive(Debug, Clone, Default)]
struct SlotUsageContext {
    ancestor_slot_names: Vec<String>,
    has_let_ancestor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlotDerivedAlias {
    alias: String,
    target: String,
    order: usize,
}

fn extend_slot_usage_context(
    context: &SlotUsageContext,
    attributes: &[Attribute],
) -> SlotUsageContext {
    let mut next = context.clone();
    if let Some(slot_name) = slot_usage_attribute_name(attributes) {
        next.ancestor_slot_names.push(slot_name.to_string());
    }
    if attributes
        .iter()
        .any(|attribute| matches!(attribute, Attribute::LetDirective(_)))
    {
        next.has_let_ancestor = true;
    }
    next
}

fn slot_alias_name(
    slot: &crate::ast::modern::SlotElement,
    context: &SlotUsageContext,
) -> Option<String> {
    let slot_name = slot_element_name(slot).unwrap_or("default");
    if slot_name == "default" {
        context
            .has_let_ancestor
            .then_some(String::from("children_render"))
    } else if context
        .ancestor_slot_names
        .iter()
        .any(|name| name == slot_name)
        || slot_usage_attribute_name(&slot.attributes).is_some_and(|name| name == slot_name)
    {
        Some(format!("{}_render", normalize_slot_identifier(slot_name)))
    } else {
        None
    }
}

fn record_slot_requirement(
    slot_props: &mut HashMap<String, SlotPropRequirement>,
    requirement: SlotPropRequirement,
) {
    let next_order = slot_props.len();
    match slot_props.entry(requirement.name.clone()) {
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            entry.get_mut().accepts_args |= requirement.accepts_args;
        }
        std::collections::hash_map::Entry::Vacant(entry) => {
            let mut requirement = requirement;
            requirement.order = next_order;
            entry.insert(requirement);
        }
    }
}

fn record_slot_alias(
    derived_aliases: &mut HashMap<String, SlotDerivedAlias>,
    alias: String,
    target: String,
) {
    let next_order = derived_aliases.len();
    derived_aliases
        .entry(alias.clone())
        .or_insert(SlotDerivedAlias {
            alias,
            target,
            order: next_order,
        });
}

fn component_slot_parent_props(attributes: &[Attribute]) -> HashSet<String> {
    let mut names = HashSet::new();
    for attribute in attributes {
        match attribute {
            Attribute::Attribute(attribute) => {
                names.insert(attribute.name.to_string());
            }
            Attribute::BindDirective(directive) => {
                names.insert(directive.name.to_string());
            }
            _ => {}
        }
    }
    names
}

fn unmigrated_slot_segment(
    source: &str,
    start: usize,
    end: usize,
    reason: &str,
) -> Option<SlotRenderedSegment> {
    let indent = line_indent_at(source, start).unwrap_or("");
    let original = source.get(start..end)?;
    Some(SlotRenderedSegment {
        prelude: String::new(),
        rendered: format!(
            "<!-- @migration-task: migrate this slot by hand, {reason} -->\n{indent}{original}"
        ),
        is_named_slot: false,
    })
}

fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn attributes_have_slot_usage(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        matches!(
            attribute,
            Attribute::Attribute(attribute) if attribute.name.as_ref() == "slot"
        ) || matches!(attribute, Attribute::LetDirective(_))
    })
}

fn fragment_has_slot_usage(fragment: &Fragment) -> bool {
    fragment.nodes.iter().any(node_has_slot_usage)
}

fn node_has_slot_usage(node: &Node) -> bool {
    if matches!(node, Node::SlotElement(_)) {
        return true;
    }

    if let Some(element) = node.as_element() {
        return attributes_have_slot_usage(element.attributes())
            || fragment_has_slot_usage(element.fragment());
    }

    if let Node::SvelteFragment(fragment) = node {
        return attributes_have_slot_usage(&fragment.attributes)
            || fragment_has_slot_usage(&fragment.fragment);
    }

    node.try_for_each_child_fragment(|fragment| {
        if fragment_has_slot_usage(fragment) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

fn collect_component_slot_usage_structure_edits(
    source: &str,
    fragment: &Fragment,
    svelte_component_state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) {
    for node in fragment.nodes.iter() {
        collect_component_slot_usage_structure_node_edits(
            source,
            node,
            svelte_component_state,
            edits,
        );
    }
}

fn collect_component_slot_usage_structure_node_edits(
    source: &str,
    node: &Node,
    svelte_component_state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) {
    match node {
        Node::Component(component) => {
            if attributes_have_slot_usage(&component.attributes)
                || fragment_has_slot_usage(&component.fragment)
            {
                let mut local_state = svelte_component_state.clone();
                if push_component_slot_usage_structure_edit(
                    source,
                    node,
                    None,
                    &mut local_state,
                    edits,
                ) {
                    *svelte_component_state = local_state;
                    return;
                }
            }
        }
        Node::SvelteComponent(component) => {
            if attributes_have_slot_usage(&component.attributes)
                || fragment_has_slot_usage(&component.fragment)
            {
                let mut local_state = svelte_component_state.clone();
                let rendered_tag = render_svelte_component_tag(source, component, &mut local_state);
                let static_name = rendered_tag
                    .as_ref()
                    .filter(|tag| tag.prelude.is_none())
                    .map(|tag| tag.name.as_str());
                if push_component_slot_usage_structure_edit(
                    source,
                    node,
                    static_name,
                    &mut local_state,
                    edits,
                ) {
                    *svelte_component_state = local_state;
                    return;
                }
            }
        }
        Node::Comment(_)
        | Node::Text(_)
        | Node::ExpressionTag(_)
        | Node::HtmlTag(_)
        | Node::ConstTag(_)
        | Node::RenderTag(_)
        | Node::DebugTag(_) => {}
        _ => {}
    }

    node.for_each_child_fragment(|fragment| {
        collect_component_slot_usage_structure_edits(
            source,
            fragment,
            svelte_component_state,
            edits,
        );
    });
}

fn push_component_slot_usage_structure_edit(
    source: &str,
    node: &Node,
    static_name: Option<&str>,
    svelte_component_state: &mut SvelteComponentMigrationState,
    edits: &mut Vec<Edit>,
) -> bool {
    let Some(render_args) = slot_usage_render_args(node, static_name) else {
        return false;
    };
    let Some(replacement) =
        render_component_slot_usage_node(source, render_args, svelte_component_state)
    else {
        return false;
    };

    edits.push(Edit {
        start: node.start(),
        end: node.end(),
        replacement,
    });
    true
}

struct SlotUsageRenderArgs<'a> {
    start: usize,
    end: usize,
    original_name: &'a str,
    attributes: &'a [Attribute],
    fragment: &'a Fragment,
    static_name: Option<&'a str>,
    tag_prelude: Option<&'a str>,
}

fn slot_usage_render_args<'a>(
    node: &'a Node,
    static_name: Option<&'a str>,
) -> Option<SlotUsageRenderArgs<'a>> {
    let element = node.as_element()?;
    Some(SlotUsageRenderArgs {
        start: node.start(),
        end: node.end(),
        original_name: element.name(),
        attributes: element.attributes(),
        fragment: element.fragment(),
        static_name,
        tag_prelude: None,
    })
}

fn render_component_slot_usage_node(
    source: &str,
    render_args: SlotUsageRenderArgs<'_>,
    svelte_component_state: &mut SvelteComponentMigrationState,
) -> Option<String> {
    let SlotUsageRenderArgs {
        start,
        end,
        original_name,
        attributes,
        fragment,
        static_name,
        tag_prelude,
    } = render_args;
    let let_props = let_directive_props(source, start, end, attributes);
    let parent_props = component_slot_parent_props(attributes);
    let rendered_fragment = render_component_slot_usage_fragment(
        source,
        fragment,
        &parent_props,
        &extend_slot_usage_context(&SlotUsageContext::default(), attributes),
        svelte_component_state,
    )?;
    let raw_fragment = fragment_source_slice(source, fragment).unwrap_or_default();
    let combined_fragment = rendered_fragment
        .iter()
        .map(|segment| format!("{}{}", segment.prelude, segment.rendered))
        .collect::<String>();
    let has_child_slot_usage = rendered_fragment
        .iter()
        .any(|segment| segment.is_named_slot || !segment.prelude.is_empty())
        || combined_fragment != raw_fragment;
    if let_props.is_empty() && !has_child_slot_usage {
        return None;
    }

    let is_self_closing = source
        .get(start..end)
        .is_some_and(|raw| raw.trim_end().ends_with("/>"));
    let open_end = opening_tag_end(source, start, end)?;
    let open_tag = cleaned_tag_source(
        source,
        TagSourceCleanupArgs {
            start,
            end: if is_self_closing { end } else { open_end + 1 },
            original_name,
            static_name,
            attributes,
            remove_slot: true,
            remove_lets: true,
        },
    )?;
    let close_tag = if is_self_closing {
        if original_name == "svelte:component" && static_name.is_some() {
            open_tag.clone()
        } else {
            expand_self_closing_tag(&open_tag, static_name.unwrap_or(original_name))
        }
    } else {
        let close_start = closing_tag_start(source, start, end)?;
        cleaned_close_tag_source(source, close_start, end, original_name, static_name)?
    };
    let fragment_source = if is_self_closing {
        String::new()
    } else if let_props.is_empty() {
        combined_fragment
    } else {
        render_component_default_slot_snippet(rendered_fragment, &let_props)?
    };
    let separator = if !is_self_closing
        && fragment_source.ends_with("{/snippet}")
        && !close_tag.starts_with('\n')
    {
        "\n"
    } else {
        ""
    };
    let rendered = if is_self_closing {
        open_tag
    } else {
        format!("{open_tag}{fragment_source}{separator}{close_tag}")
    };
    Some(match tag_prelude {
        Some(prelude) => format!("{prelude}{rendered}"),
        None => rendered,
    })
}

fn render_component_slot_usage_fragment(
    source: &str,
    fragment: &Fragment,
    parent_props: &HashSet<String>,
    context: &SlotUsageContext,
    svelte_component_state: &mut SvelteComponentMigrationState,
) -> Option<Vec<SlotRenderedSegment>> {
    let mut segments = Vec::new();
    let mut cursor = fragment.nodes.first().map(Span::start).unwrap_or(0);

    for node in fragment.nodes.iter() {
        if cursor < node.start() {
            segments.push(SlotRenderedSegment {
                prelude: String::new(),
                rendered: source.get(cursor..node.start())?.to_string(),
                is_named_slot: false,
            });
        }
        if let Some(rendered) = render_component_child_slot_usage(
            source,
            node,
            parent_props,
            context,
            svelte_component_state,
        ) {
            segments.push(rendered);
        } else {
            segments.push(SlotRenderedSegment {
                prelude: String::new(),
                rendered: source.get(node.start()..node.end())?.to_string(),
                is_named_slot: false,
            });
        }
        cursor = node.end();
    }

    let fragment_end = fragment.nodes.last().map(Span::end).unwrap_or(cursor);
    if cursor < fragment_end {
        segments.push(SlotRenderedSegment {
            prelude: String::new(),
            rendered: source.get(cursor..fragment_end)?.to_string(),
            is_named_slot: false,
        });
    }

    Some(segments)
}

fn render_component_default_slot_snippet(
    segments: Vec<SlotRenderedSegment>,
    let_props: &[String],
) -> Option<String> {
    let first_default_index = segments
        .iter()
        .position(|segment| !segment.is_named_slot && !segment.rendered.trim().is_empty())?;
    let mut before = String::new();
    let mut current_default_group = String::new();
    let mut default_groups = Vec::new();
    let mut after = String::new();
    let mut named_slot_seen = false;

    for (index, segment) in segments.into_iter().enumerate() {
        let segment_text = format!("{}{}", segment.prelude, segment.rendered);
        if index < first_default_index {
            before.push_str(&segment_text);
        } else if segment.is_named_slot {
            named_slot_seen = true;
            if !current_default_group.is_empty() {
                default_groups.push(std::mem::take(&mut current_default_group));
            }
            after.push_str(&segment_text);
        } else {
            if named_slot_seen && !current_default_group.is_empty() {
                default_groups.push(std::mem::take(&mut current_default_group));
            }
            current_default_group.push_str(&segment_text);
            named_slot_seen = false;
        }
    }
    if !current_default_group.is_empty() {
        default_groups.push(current_default_group);
    }
    let default_content = default_groups.join("");

    let default_content = if after.is_empty() {
        default_content
    } else {
        default_content
            .trim_end_matches(char::is_whitespace)
            .to_string()
    };
    let snippet_indent = if trailing_line_indent(&before).is_empty() {
        leading_non_empty_line_indent(&default_content).unwrap_or("")
    } else {
        trailing_line_indent(&before)
    };
    let child_indent = format!("{snippet_indent}{snippet_indent}");
    let formatted_default =
        if default_content.contains("{@const ") && !default_content.starts_with('\n') {
            if let Some((prelude, remainder)) = default_content.split_once('\n') {
                let remainder = remainder.strip_prefix(snippet_indent).unwrap_or(remainder);
                format!(
                    "{child_indent}{prelude}\n{}",
                    indent_block_with_first_indent(remainder, snippet_indent, snippet_indent)
                )
            } else {
                format!("{child_indent}{default_content}")
            }
        } else if default_content.starts_with('\n') {
            if after.is_empty() {
                default_content.clone()
            } else {
                let lines = default_content.split('\n').collect::<Vec<_>>();
                if lines.len() <= 1 {
                    indent_block_with_first_indent(&default_content, snippet_indent, snippet_indent)
                } else {
                    let last_index = lines.len() - 1;
                    let mut output = String::new();
                    for (index, line) in lines.iter().enumerate() {
                        if index > 0 {
                            output.push('\n');
                        }
                        if index == last_index {
                            output.push_str(line);
                        } else {
                            output.push_str(snippet_indent);
                            output.push_str(line);
                            if index > 0 && line.trim().is_empty() && !line.is_empty() {
                                output.push(' ');
                            }
                        }
                    }
                    output
                }
            }
        } else if default_groups.len() <= 1 {
            indent_block_with_first_indent(&default_content, &child_indent, snippet_indent)
        } else {
            let mut groups = default_groups;
            let first_group = groups.remove(0);
            let first_group = if before.is_empty() || after.is_empty() {
                first_group
            } else {
                first_group
                    .trim_end_matches(char::is_whitespace)
                    .to_string()
            };
            let mut output =
                indent_block_with_first_indent(&first_group, &child_indent, snippet_indent);
            let trailing_groups = groups.join("");
            output.push_str(trailing_groups.trim_end_matches(char::is_whitespace));
            output
        };
    let after = normalize_adjacent_named_snippets(&after, snippet_indent);
    let close = if !after.is_empty() {
        if default_content.contains('\n') {
            format!("{{/snippet}}\n{snippet_indent}")
        } else {
            format!("\n{child_indent}{{/snippet}}\n{snippet_indent}")
        }
    } else if formatted_default.ends_with('\n') {
        format!("{snippet_indent}{{/snippet}}")
    } else {
        String::from("{/snippet}")
    };

    Some(format!(
        "{before}{{#snippet children({})}}\n{formatted_default}{close}{after}",
        render_snippet_props(let_props)
    ))
}

fn render_component_child_slot_usage(
    source: &str,
    node: &Node,
    parent_props: &HashSet<String>,
    context: &SlotUsageContext,
    svelte_component_state: &mut SvelteComponentMigrationState,
) -> Option<SlotRenderedSegment> {
    match node {
        Node::RegularElement(_) => render_slot_usage_element_like(
            source,
            slot_usage_render_args(node, None)?,
            false,
            parent_props,
            context,
            svelte_component_state,
        ),
        Node::Component(_) => render_component_slot_usage_node(
            source,
            slot_usage_render_args(node, None)?,
            svelte_component_state,
        )
        .map(|rendered| SlotRenderedSegment {
            prelude: String::new(),
            rendered,
            is_named_slot: false,
        })
        .or_else(|| {
            render_slot_usage_element_like(
                source,
                slot_usage_render_args(node, None)?,
                false,
                parent_props,
                context,
                svelte_component_state,
            )
        }),
        Node::SvelteElement(_) => render_slot_usage_element_like(
            source,
            slot_usage_render_args(node, None)?,
            false,
            parent_props,
            context,
            svelte_component_state,
        ),
        Node::SvelteComponent(component) => {
            let rendered_tag =
                render_svelte_component_tag(source, component, svelte_component_state);
            let prelude = rendered_tag
                .as_ref()
                .and_then(|tag| tag.prelude.clone())
                .unwrap_or_default();
            let static_name = rendered_tag.as_ref().map(|tag| tag.name.as_str());
            render_component_slot_usage_node(
                source,
                slot_usage_render_args(node, static_name)?,
                svelte_component_state,
            )
            .map(|rendered| SlotRenderedSegment {
                prelude: prelude.clone(),
                rendered,
                is_named_slot: false,
            })
            .or_else(|| {
                render_slot_usage_element_like(
                    source,
                    slot_usage_render_args(node, static_name)?,
                    false,
                    parent_props,
                    context,
                    svelte_component_state,
                )
            })
            .map(|mut segment| {
                if !prelude.is_empty() && segment.prelude.is_empty() {
                    segment.prelude = prelude.clone();
                }
                segment
            })
        }
        Node::SlotElement(slot) => {
            let snippet_name = slot_usage_attribute_name(&slot.attributes)
                .map(normalize_slot_identifier)
                .unwrap_or_else(|| "children".to_string());
            migrate_component_child_slot_element(source, slot, false, context).map(|(edit, _)| {
                SlotRenderedSegment {
                    prelude: String::new(),
                    rendered: edit.replacement,
                    is_named_slot: snippet_name != "children",
                }
            })
        }
        Node::SvelteFragment(_) => render_slot_usage_element_like(
            source,
            slot_usage_render_args(node, None)?,
            true,
            parent_props,
            context,
            svelte_component_state,
        ),
        _ => None,
    }
}

fn render_slot_usage_element_like(
    source: &str,
    render_args: SlotUsageRenderArgs<'_>,
    is_fragment: bool,
    parent_props: &HashSet<String>,
    context: &SlotUsageContext,
    svelte_component_state: &mut SvelteComponentMigrationState,
) -> Option<SlotRenderedSegment> {
    let SlotUsageRenderArgs {
        start,
        end,
        original_name,
        attributes,
        fragment,
        static_name,
        tag_prelude,
    } = render_args;
    let raw_slot_name = slot_usage_attribute_name(attributes);
    if let Some(slot_name) = raw_slot_name
        && slot_name != "default"
    {
        if !is_valid_identifier(slot_name) || is_reserved_identifier(slot_name) {
            return unmigrated_slot_segment(
                source,
                start,
                end,
                &format!("`{slot_name}` is an invalid identifier"),
            );
        }
        if parent_props.contains(slot_name) {
            return unmigrated_slot_segment(
                source,
                start,
                end,
                &format!("`{slot_name}` would shadow a prop on the parent component"),
            );
        }
    }
    let slot_name = raw_slot_name
        .map(normalize_slot_identifier)
        .unwrap_or_else(|| "children".to_string());
    let let_props = let_directive_props(source, start, end, attributes);
    if slot_usage_attribute_name(attributes).is_none()
        && let_props.is_empty()
        && static_name.is_none()
    {
        return None;
    }

    if is_fragment {
        let inner_segments = render_component_slot_usage_fragment(
            source,
            fragment,
            parent_props,
            &extend_slot_usage_context(context, attributes),
            svelte_component_state,
        )?;
        let child_prelude = inner_segments
            .iter()
            .map(|segment| segment.prelude.as_str())
            .collect::<String>();
        let inner = inner_segments
            .into_iter()
            .map(|segment| segment.rendered)
            .collect::<String>();
        let inner = format!("{child_prelude}{inner}");
        let snippet_indent = line_indent_at(source, start).unwrap_or("");
        let child_indent = format!("{snippet_indent}{snippet_indent}");
        let formatted_inner = if !child_prelude.is_empty() && inner.contains('\n') {
            if let Some((prelude, remainder)) = inner.split_once('\n') {
                let remainder = remainder
                    .strip_prefix(&child_indent)
                    .or_else(|| remainder.strip_prefix(snippet_indent))
                    .unwrap_or(remainder);
                format!(
                    "{snippet_indent}{prelude}\n{}",
                    indent_block_with_first_indent(remainder, snippet_indent, snippet_indent)
                )
            } else {
                format!("{snippet_indent}{inner}")
            }
        } else {
            let first_indent = if inner.starts_with('\n') {
                snippet_indent
            } else {
                &child_indent
            };
            indent_block_with_first_indent(&inner, first_indent, snippet_indent)
        };
        return Some(SlotRenderedSegment {
            prelude: String::new(),
            rendered: format!(
                "{{#snippet {slot_name}({})}}\n{formatted_inner}\n{snippet_indent}{{/snippet}}",
                render_snippet_props(&let_props)
            ),
            is_named_slot: slot_name != "children",
        });
    }

    let is_self_closing = source
        .get(start..end)
        .is_some_and(|raw| raw.trim_end().ends_with("/>"));
    let open_end = opening_tag_end(source, start, end)?;
    let open_tag = cleaned_tag_source(
        source,
        TagSourceCleanupArgs {
            start,
            end: if is_self_closing { end } else { open_end + 1 },
            original_name,
            static_name,
            attributes,
            remove_slot: true,
            remove_lets: true,
        },
    )?;
    let nested_context = extend_slot_usage_context(context, attributes);
    let (inner_segments, close_tag) = if is_self_closing {
        (
            Vec::new(),
            if original_name == "svelte:component" && static_name.is_some() {
                open_tag.clone()
            } else {
                expand_self_closing_tag(&open_tag, static_name.unwrap_or(original_name))
            },
        )
    } else {
        let close_start = closing_tag_start(source, start, end)?;
        (
            render_component_slot_usage_fragment(
                source,
                fragment,
                parent_props,
                &nested_context,
                svelte_component_state,
            )?,
            cleaned_close_tag_source(source, close_start, end, original_name, static_name)?,
        )
    };
    let hoist_child_prelude = slot_name == "children";
    let child_prelude = if hoist_child_prelude || slot_name != "children" {
        inner_segments
            .iter()
            .map(|segment| segment.prelude.as_str())
            .collect::<String>()
    } else {
        String::new()
    };
    let inner = inner_segments
        .into_iter()
        .map(|segment| {
            if hoist_child_prelude || slot_name != "children" {
                segment.rendered
            } else {
                format!("{}{}", segment.prelude, segment.rendered)
            }
        })
        .collect::<String>();

    let rendered = if slot_name == "children" {
        if let_props.is_empty() {
            if is_self_closing {
                close_tag
            } else {
                format!("{open_tag}{inner}{close_tag}")
            }
        } else {
            let snippet_indent = line_indent_at(source, start).unwrap_or("");
            let child_indent = format!("{snippet_indent}{snippet_indent}");
            let content = if is_self_closing { &close_tag } else { &inner };
            let content =
                if content.starts_with('\n') && content.trim_start().starts_with("{@render") {
                    &content[1..]
                } else {
                    content
                };
            let formatted_inner = indent_block_with_first_indent(
                content,
                if content.starts_with('\n') {
                    snippet_indent
                } else {
                    &child_indent
                },
                snippet_indent,
            );
            let render_only_slot_child = raw_inner_contains_slot(source, open_end, start, end)
                && content.trim_start().starts_with("{@render");
            let close = if render_only_slot_child {
                format!("{child_indent}{snippet_indent}{{/snippet}}")
            } else if formatted_inner.ends_with('\n') {
                format!("{snippet_indent}{{/snippet}}")
            } else {
                String::from("{/snippet}")
            };
            let snippet = format!(
                "{{#snippet children({})}}\n{formatted_inner}{close}",
                render_snippet_props(&let_props),
            );
            if is_self_closing {
                snippet
            } else {
                let close_start = closing_tag_start(source, start, end).unwrap_or(end);
                let raw_inner = source.get(open_end + 1..close_start).unwrap_or("");
                if child_prelude.is_empty() && inner == raw_inner && raw_inner.contains('\n') {
                    let snippet = format!(
                        "{{#snippet children({})}}{raw_inner}{{/snippet}}",
                        render_snippet_props(&let_props),
                    );
                    format!("{open_tag}{snippet}{close_tag}")
                } else if child_prelude.is_empty() && inner == raw_inner {
                    let indent_unit = guess_indent(source);
                    let body_indent = indent_unit.repeat(4);
                    let trailing_indent = indent_unit.repeat(3);
                    let close_indent = indent_unit.repeat(2);
                    let snippet = format!(
                        "{{#snippet children({})}}\n{body_indent}{raw_inner}{trailing_indent}{{/snippet}}\n{close_indent}",
                        render_snippet_props(&let_props),
                    );
                    format!("{open_tag}{snippet}{close_tag}")
                } else {
                    let prefix = raw_inner
                        .chars()
                        .take_while(|ch| ch.is_whitespace())
                        .collect::<String>();
                    let mut suffix = raw_inner
                        .chars()
                        .rev()
                        .take_while(|ch| ch.is_whitespace())
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect::<String>();
                    if render_only_slot_child {
                        suffix = format!("\n{child_indent}");
                    }
                    if suffix.is_empty() && snippet.contains('\n') {
                        suffix = format!("\n{snippet_indent}");
                    }
                    format!("{open_tag}{prefix}{snippet}{suffix}{close_tag}")
                }
            }
        }
    } else {
        let snippet_indent = line_indent_at(source, start).unwrap_or("");
        let child_indent = format!("{snippet_indent}{snippet_indent}");
        let node_source = if is_self_closing {
            format!("{child_prelude}{close_tag}")
        } else {
            format!("{child_prelude}{open_tag}{inner}{close_tag}")
        };
        let render_only_named_slot =
            node_source.contains("{@render ") && !node_source.contains("{#snippet ");
        let formatted_node = if !child_prelude.is_empty() && node_source.contains('\n') {
            if let Some((prelude, remainder)) = node_source.split_once('\n') {
                let remainder = remainder
                    .strip_prefix(&child_indent)
                    .or_else(|| remainder.strip_prefix(snippet_indent))
                    .unwrap_or(remainder);
                format!(
                    "{child_indent}{prelude}\n{}",
                    indent_block_with_first_indent(remainder, snippet_indent, snippet_indent)
                )
            } else {
                format!("{child_indent}{node_source}")
            }
        } else if render_only_named_slot && node_source.contains('\n') {
            if let Some((first_line, remainder)) = node_source.split_once('\n') {
                let indent_unit = guess_indent(source);
                let first_indent = child_indent
                    .strip_suffix(&indent_unit)
                    .unwrap_or(&child_indent);
                let rest_indent = format!("{snippet_indent}{indent_unit}");
                let dedented_remainder = dedent_all_lines(remainder);
                format!(
                    "{first_indent}{first_line}\n{}",
                    indent_block_with_first_indent(&dedented_remainder, &rest_indent, &rest_indent)
                )
            } else {
                indent_block_with_first_indent(&node_source, &child_indent, snippet_indent)
            }
        } else {
            indent_block_with_first_indent(&node_source, &child_indent, snippet_indent)
        };
        format!(
            "{{#snippet {slot_name}({})}}\n{formatted_node}\n{}{{/snippet}}",
            render_snippet_props(&let_props),
            if render_only_named_slot {
                format!("{snippet_indent}{}", guess_indent(source))
            } else {
                snippet_indent.to_string()
            }
        )
    };

    let mut prelude = if slot_name == "children" {
        child_prelude
    } else {
        String::new()
    };
    if let Some(tag_prelude) = tag_prelude {
        if slot_name == "children" {
            prelude.push_str(tag_prelude);
        } else {
            return Some(SlotRenderedSegment {
                prelude,
                rendered: format!("{tag_prelude}{rendered}"),
                is_named_slot: slot_name != "children",
            });
        }
    }

    Some(SlotRenderedSegment {
        prelude,
        rendered,
        is_named_slot: slot_name != "children",
    })
}

fn raw_inner_contains_slot(source: &str, open_end: usize, start: usize, end: usize) -> bool {
    let Some(close_start) = closing_tag_start(source, start, end) else {
        return false;
    };
    source
        .get(open_end + 1..close_start)
        .is_some_and(|raw_inner| raw_inner.contains("<slot"))
}

fn expand_self_closing_tag(tag_source: &str, name: &str) -> String {
    let expanded = tag_source.replace("/>", &format!("></{name}>"));
    expanded.replace("  >", " >")
}

fn fragment_source_slice<'a>(source: &'a str, fragment: &Fragment) -> Option<&'a str> {
    let start = fragment.nodes.first().map(Span::start)?;
    let end = fragment.nodes.last().map(Span::end)?;
    source.get(start..end)
}

fn let_directive_props(
    source: &str,
    start: usize,
    end: usize,
    attributes: &[Attribute],
) -> Vec<String> {
    let props = attributes
        .iter()
        .filter_map(|attribute| {
            let Attribute::LetDirective(directive) = attribute else {
                return None;
            };
            let value = expression_source(source, &directive.expression)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && value != directive.name.as_ref())
                .map(|value| format!("{}: {value}", directive.name))
                .unwrap_or_else(|| directive.name.to_string());
            Some(value)
        })
        .collect::<Vec<_>>();
    if !props.is_empty() {
        return props;
    }

    scan_open_tag_let_directive_props(source, start, end)
}

fn render_snippet_props(let_props: &[String]) -> String {
    if let_props.is_empty() {
        String::new()
    } else {
        format!("{{ {} }}", let_props.join(", "))
    }
}

struct TagSourceCleanupArgs<'a> {
    start: usize,
    end: usize,
    original_name: &'a str,
    static_name: Option<&'a str>,
    attributes: &'a [Attribute],
    remove_slot: bool,
    remove_lets: bool,
}

fn cleaned_tag_source(source: &str, args: TagSourceCleanupArgs<'_>) -> Option<String> {
    let TagSourceCleanupArgs {
        start,
        end,
        original_name,
        static_name,
        attributes,
        remove_slot,
        remove_lets,
    } = args;
    let mut edits = Vec::new();
    let has_slot_attribute = attributes.iter().any(|attribute| {
        matches!(attribute, Attribute::Attribute(attribute) if attribute.name.as_ref() == "slot")
    });
    let has_let_directive = attributes
        .iter()
        .any(|attribute| matches!(attribute, Attribute::LetDirective(_)));
    if let Some(static_name) = static_name {
        edits.push(Edit {
            start: start + 1,
            end: start + 1 + original_name.len(),
            replacement: static_name.to_string(),
        });
        if let Some((attr_start, attr_end)) =
            svelte_component_this_attribute_range(source, start, end)
        {
            edits.push(Edit {
                start: attr_start,
                end: attr_end,
                replacement: String::new(),
            });
        }
    }
    for attribute in attributes {
        match attribute {
            Attribute::Attribute(attribute) if remove_slot && attribute.name.as_ref() == "slot" => {
                edits.push(Edit {
                    start: attribute.start,
                    end: attribute.end,
                    replacement: String::new(),
                });
            }
            Attribute::LetDirective(directive) if remove_lets => {
                edits.push(Edit {
                    start: directive.start,
                    end: directive.end,
                    replacement: String::new(),
                });
            }
            _ => {}
        }
    }
    let local = source.get(start..end)?;
    if remove_slot
        && !edits
            .iter()
            .any(|edit| edit.start >= start && edit.end <= end)
    {
        for (attr_start, attr_end) in scan_open_tag_attribute_ranges(local, "slot=") {
            edits.push(Edit {
                start: start + attr_start,
                end: start + attr_end,
                replacement: String::new(),
            });
        }
    }
    if remove_lets {
        for (attr_start, attr_end) in scan_open_tag_let_directive_ranges(local) {
            if !edits
                .iter()
                .any(|edit| edit.start == start + attr_start && edit.end == start + attr_end)
            {
                edits.push(Edit {
                    start: start + attr_start,
                    end: start + attr_end,
                    replacement: String::new(),
                });
            }
        }
    }
    let mut local_edits = edits
        .into_iter()
        .map(|edit| Edit {
            start: edit.start - start,
            end: edit.end - start,
            replacement: edit.replacement,
        })
        .collect::<Vec<_>>();
    let mut output = apply_edits(local, &mut local_edits);
    if static_name.is_some() {
        while output.contains("  >") {
            output = output.replace("  >", " >");
        }
        while output.contains("  />") {
            output = output.replace("  />", " />");
        }
        if !has_slot_attribute && !has_let_directive {
            output = output.replace(" >", ">");
        }
    }
    Some(output)
}

fn cleaned_close_tag_source(
    source: &str,
    start: usize,
    end: usize,
    original_name: &str,
    static_name: Option<&str>,
) -> Option<String> {
    let local = source.get(start..end)?;
    let mut edits = Vec::new();
    if let Some(static_name) = static_name {
        edits.push(Edit {
            start: 2,
            end: 2 + original_name.len(),
            replacement: static_name.to_string(),
        });
    }
    Some(apply_edits(local, &mut edits))
}

fn opening_tag_end(source: &str, start: usize, end: usize) -> Option<usize> {
    source
        .get(start..end)?
        .find('>')
        .map(|offset| start + offset)
}

fn closing_tag_start(source: &str, start: usize, end: usize) -> Option<usize> {
    source
        .get(start..end)?
        .rfind("</")
        .map(|offset| start + offset)
}

fn svelte_component_this_attribute_range(
    source: &str,
    start: usize,
    end: usize,
) -> Option<(usize, usize)> {
    let raw = source.get(start..end)?;
    let attr_offset = raw.find("this={")?;
    let attr_start = start + attr_offset;
    let after = raw.get(attr_offset + "this={".len()..)?;
    let close_offset = after.find('}')?;
    Some((attr_start, attr_start + "this={".len() + close_offset + 1))
}

fn trailing_line_indent(source: &str) -> &str {
    source
        .rsplit_once('\n')
        .map(|(_, tail)| tail)
        .unwrap_or(source)
}

fn leading_non_empty_line_indent(source: &str) -> Option<&str> {
    for line in source.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indent_len = line
            .char_indices()
            .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
            .unwrap_or(line.len());
        return Some(&line[..indent_len]);
    }
    None
}

fn indent_block_with_first_indent(content: &str, first_indent: &str, rest_indent: &str) -> String {
    let mut output = String::new();
    if content.is_empty() {
        return output;
    }

    output.push_str(first_indent);
    for (index, ch) in content.char_indices() {
        output.push(ch);
        if ch == '\n' && index + ch.len_utf8() < content.len() {
            output.push_str(rest_indent);
        }
    }
    output
}

fn dedent_all_lines(content: &str) -> String {
    let lines = content.split('\n').collect::<Vec<_>>();
    let common_indent = lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(
                    line.char_indices()
                        .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
                        .unwrap_or(line.len()),
                )
            }
        })
        .min()
        .unwrap_or(0);

    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        if common_indent == 0 {
            output.push_str(line);
        } else if line.len() >= common_indent {
            output.push_str(&line[common_indent..]);
        } else {
            output.push_str(line.trim_start());
        }
    }
    output
}

fn normalize_adjacent_named_snippets(content: &str, indent: &str) -> String {
    if content.is_empty() {
        return String::new();
    }

    let normalized = content.replace(
        "{/snippet}{#snippet",
        &format!("{{/snippet}}\n{indent}{{#snippet"),
    );
    normalized.replace(
        "{/snippet}\n{#snippet",
        &format!("{{/snippet}}\n{indent}{{#snippet"),
    )
}

fn scan_open_tag_let_directive_props(source: &str, start: usize, end: usize) -> Vec<String> {
    let Some(tag_end) = opening_tag_end(source, start, end).map(|index| index + 1) else {
        return Vec::new();
    };
    let Some(raw) = source.get(start..tag_end) else {
        return Vec::new();
    };

    scan_open_tag_let_directive_ranges(raw)
        .into_iter()
        .filter_map(|(attr_start, attr_end)| {
            let raw_attr = raw.get(attr_start..attr_end)?.trim();
            let body = raw_attr.strip_prefix("let:")?;
            let (name, remainder) = body
                .split_once('=')
                .map(|(name, remainder)| (name.trim(), remainder.trim()))
                .unwrap_or((body.trim(), ""));
            if name.is_empty() {
                return None;
            }
            if remainder.is_empty() {
                Some(name.to_string())
            } else {
                let expression = remainder
                    .strip_prefix('{')
                    .and_then(|value| value.strip_suffix('}'))
                    .unwrap_or(remainder)
                    .trim();
                if expression.is_empty() || expression == name {
                    Some(name.to_string())
                } else {
                    Some(format!("{name}: {expression}"))
                }
            }
        })
        .collect()
}

fn scan_open_tag_let_directive_ranges(raw: &str) -> Vec<(usize, usize)> {
    let bytes = raw.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while let Some(offset) = raw[index..].find("let:") {
        let attr_start = index + offset;
        let mut attr_end = attr_start + "let:".len();
        while attr_end < bytes.len() {
            let byte = bytes[attr_end];
            if byte.is_ascii_whitespace() || byte == b'>' {
                break;
            }
            if byte == b'=' && bytes.get(attr_end + 1).copied() == Some(b'{') {
                attr_end += 2;
                while attr_end < bytes.len() && bytes[attr_end] != b'}' {
                    attr_end += 1;
                }
                if attr_end < bytes.len() {
                    attr_end += 1;
                }
                break;
            }
            attr_end += 1;
        }
        ranges.push((attr_start, attr_end));
        index = attr_end;
    }
    ranges
}

fn scan_open_tag_attribute_ranges(raw: &str, needle: &str) -> Vec<(usize, usize)> {
    let bytes = raw.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while let Some(offset) = raw[index..].find(needle) {
        let attr_key_start = index + offset;
        let attr_start = raw[..attr_key_start]
            .rfind(char::is_whitespace)
            .map(|position| position + 1)
            .unwrap_or(attr_key_start);
        let mut attr_end = attr_key_start + needle.len();
        if bytes.get(attr_end).copied() == Some(b'"') {
            attr_end += 1;
            while attr_end < bytes.len() && bytes[attr_end] != b'"' {
                attr_end += 1;
            }
            if attr_end < bytes.len() {
                attr_end += 1;
            }
        } else if bytes.get(attr_end).copied() == Some(b'\'') {
            attr_end += 1;
            while attr_end < bytes.len() && bytes[attr_end] != b'\'' {
                attr_end += 1;
            }
            if attr_end < bytes.len() {
                attr_end += 1;
            }
        } else {
            while attr_end < bytes.len()
                && !bytes[attr_end].is_ascii_whitespace()
                && bytes[attr_end] != b'>'
            {
                attr_end += 1;
            }
        }
        ranges.push((attr_start, attr_end));
        index = attr_end;
    }
    ranges
}

fn collect_slot_usage_fragment_edits(
    source: &str,
    fragment: &Fragment,
    parent_is_component: bool,
    use_rest_props: bool,
    context: &SlotUsageContext,
    state: &mut SlotUsageEditState<'_>,
) {
    for node in fragment.nodes.iter() {
        collect_slot_usage_node_edits(
            source,
            node,
            parent_is_component,
            use_rest_props,
            context,
            state,
        );
    }
}

struct SlotUsageEditState<'a> {
    slot_props: &'a mut HashMap<String, SlotPropRequirement>,
    derived_aliases: &'a mut HashMap<String, SlotDerivedAlias>,
    edits: &'a mut Vec<Edit>,
}

fn collect_slot_usage_node_edits(
    source: &str,
    node: &Node,
    parent_is_component: bool,
    use_rest_props: bool,
    context: &SlotUsageContext,
    state: &mut SlotUsageEditState<'_>,
) {
    match node {
        Node::SlotElement(slot) => {
            if parent_is_component
                && let Some((edit, requirement)) =
                    migrate_component_child_slot_element(source, slot, use_rest_props, context)
            {
                if let Some(alias) = slot_alias_name(slot, context) {
                    record_slot_alias(state.derived_aliases, alias, requirement.name.clone());
                }
                record_slot_requirement(state.slot_props, requirement);
                state.edits.push(edit);
                return;
            }
            if let Some((edit, requirement)) =
                migrate_slot_element_placeholder(source, slot, use_rest_props, context)
            {
                if let Some(alias) = slot_alias_name(slot, context) {
                    record_slot_alias(state.derived_aliases, alias, requirement.name.clone());
                }
                record_slot_requirement(state.slot_props, requirement);
                state.edits.push(edit);
                return;
            }
            collect_slot_usage_descendant_edits(
                source,
                node,
                parent_is_component,
                use_rest_props,
                context,
                state,
            );
        }
        Node::Comment(_)
        | Node::Text(_)
        | Node::ExpressionTag(_)
        | Node::HtmlTag(_)
        | Node::ConstTag(_)
        | Node::RenderTag(_)
        | Node::DebugTag(_) => {}
        _ => collect_slot_usage_descendant_edits(
            source,
            node,
            parent_is_component,
            use_rest_props,
            context,
            state,
        ),
    }
}

fn collect_slot_usage_descendant_edits(
    source: &str,
    node: &Node,
    parent_is_component: bool,
    use_rest_props: bool,
    context: &SlotUsageContext,
    state: &mut SlotUsageEditState<'_>,
) {
    let next_context = node
        .as_element()
        .map(|element| extend_slot_usage_context(context, element.attributes()));
    let next_parent_is_component = slot_usage_child_parent_is_component(node, parent_is_component);
    let next_context = next_context.as_ref().unwrap_or(context);

    node.for_each_child_fragment(|fragment| {
        collect_slot_usage_fragment_edits(
            source,
            fragment,
            next_parent_is_component,
            use_rest_props,
            next_context,
            state,
        );
    });
}

fn slot_usage_child_parent_is_component(node: &Node, parent_is_component: bool) -> bool {
    match node {
        Node::Component(_) | Node::SvelteComponent(_) => true,
        Node::SvelteFragment(_) => parent_is_component,
        _ if node.as_element().is_some() => false,
        _ => parent_is_component,
    }
}

fn migrate_component_child_slot_element(
    source: &str,
    slot: &crate::ast::modern::SlotElement,
    use_rest_props: bool,
    context: &SlotUsageContext,
) -> Option<(Edit, SlotPropRequirement)> {
    let snippet_name = slot_usage_attribute_name(&slot.attributes)
        .map(normalize_slot_identifier)
        .unwrap_or_else(|| "children".to_string());
    let slot_name = slot_element_name(slot).unwrap_or("default");
    let prop_name = normalize_slot_identifier(slot_name);
    let args = slot_render_argument_source(source, &slot.attributes)?;
    let args = if use_rest_props {
        args.replace("$$props", "props")
    } else {
        args
    };
    let accepts_args = prop_name != "children" && !args.is_empty();
    let prop_reference = slot_alias_name(slot, context)
        .unwrap_or_else(|| slot_prop_reference(slot_name, use_rest_props));
    let render_expression = if args.is_empty() {
        format!("{prop_reference}?.()")
    } else {
        format!("{prop_reference}?.({args})")
    };
    let body = if slot.fragment.nodes.is_empty() {
        format!("{{@render {render_expression}}}")
    } else {
        let fallback_start = slot.fragment.nodes.first()?.start();
        let fallback_end = slot.fragment.nodes.last()?.end();
        let fallback = source.get(fallback_start..fallback_end)?;
        format!("{{#if {prop_name}}}{{@render {render_expression}}}{{:else}}{fallback}{{/if}}")
    };

    let replacement = if snippet_name == "children" {
        body
    } else {
        let indent = line_indent_at(source, slot.start).unwrap_or("");
        let child_indent = format!("{indent}{}", guess_indent(source));
        format!("{{#snippet {snippet_name}()}}\n{child_indent}{body}\n{indent}{{/snippet}}")
    };

    Some((
        Edit {
            start: slot.start,
            end: slot.end,
            replacement,
        },
        SlotPropRequirement {
            name: prop_name,
            accepts_args,
            order: 0,
        },
    ))
}

fn migrate_slot_element_placeholder(
    source: &str,
    slot: &crate::ast::modern::SlotElement,
    use_rest_props: bool,
    context: &SlotUsageContext,
) -> Option<(Edit, SlotPropRequirement)> {
    let slot_name = slot_element_name(slot).unwrap_or("default");
    let prop_name = normalize_slot_identifier(slot_name);
    let args = slot_render_argument_source(source, &slot.attributes)?;
    let args = if use_rest_props {
        args.replace("$$props", "props")
    } else {
        args
    };
    let accepts_args = prop_name != "children" && !args.is_empty();
    let prop_reference = slot_alias_name(slot, context)
        .unwrap_or_else(|| slot_prop_reference(slot_name, use_rest_props));
    let render_expression = if args.is_empty() {
        format!("{{@render {prop_reference}?.()}}")
    } else {
        format!("{{@render {prop_reference}?.({args})}}")
    };
    let replacement = if slot.fragment.nodes.is_empty() {
        render_expression
    } else {
        let fallback_start = slot.fragment.nodes.first()?.start();
        let fallback_end = slot.fragment.nodes.last()?.end();
        let fallback = source.get(fallback_start..fallback_end)?;
        format!("{{#if {prop_name}}}{render_expression}{{:else}}{fallback}{{/if}}")
    };

    Some((
        Edit {
            start: slot.start,
            end: slot.end,
            replacement,
        },
        SlotPropRequirement {
            name: prop_name,
            accepts_args,
            order: 0,
        },
    ))
}

fn collect_slot_prop_prelude_edits(
    source: &str,
    root: &ModernRoot,
    use_ts: bool,
    use_rest_props: bool,
    slot_props: &HashMap<String, SlotPropRequirement>,
    derived_aliases: &HashMap<String, SlotDerivedAlias>,
    edits: &mut Vec<Edit>,
) {
    if slot_props.is_empty() && derived_aliases.is_empty() {
        return;
    }

    if let Some(instance) = root.instance.as_ref() {
        if program_has_export_let(&instance.content) {
            return;
        }
        let indent = guess_indent(source);
        let is_typescript = script_is_typescript(source, instance);
        let insertion = render_slot_prop_prelude(
            slot_props,
            derived_aliases,
            is_typescript || use_ts,
            use_rest_props,
            indent,
        );

        if use_ts && !is_typescript && !program_has_export_let(&instance.content) {
            let newline_end = if source
                .get(instance.content_start..)
                .is_some_and(|content| content.starts_with('\n'))
            {
                instance.content_start + 1
            } else {
                instance.content_start
            };
            edits.push(Edit {
                start: instance.content_start.saturating_sub(1),
                end: instance.content_start.saturating_sub(1),
                replacement: " lang=\"ts\"".to_string(),
            });
            edits.push(Edit {
                start: instance.content_start,
                end: newline_end,
                replacement: format!("\n{insertion}"),
            });
            return;
        }

        let needs_leading_newline = source
            .get(instance.content_start..instance.content_end)
            .is_some_and(|content| !content.is_empty() && !content.ends_with('\n'));

        edits.push(Edit {
            start: instance.content_end,
            end: instance.content_end,
            replacement: if needs_leading_newline {
                format!("\n{insertion}")
            } else {
                insertion
            },
        });
        return;
    }

    let indent = guess_indent(source);
    let script_tag = if use_ts {
        "<script lang=\"ts\">\n"
    } else {
        "<script>\n"
    };
    let insertion = format!(
        "{script_tag}{}</script>\n\n",
        render_slot_prop_prelude(slot_props, derived_aliases, use_ts, use_rest_props, indent)
    );
    let start = root
        .module
        .as_ref()
        .map(|script| line_end_including_newline(source, script.end))
        .unwrap_or(0);
    edits.push(Edit {
        start,
        end: start,
        replacement: insertion,
    });
}

fn render_slot_prop_prelude(
    slot_props: &HashMap<String, SlotPropRequirement>,
    derived_aliases: &HashMap<String, SlotDerivedAlias>,
    is_typescript: bool,
    use_rest_props: bool,
    indent: &str,
) -> String {
    let mut slot_props = slot_props.values().cloned().collect::<Vec<_>>();
    slot_props.sort_by_key(|prop| prop.order);
    let mut derived_aliases = derived_aliases.values().cloned().collect::<Vec<_>>();
    derived_aliases.sort_by_key(|alias| alias.order);

    let destructured = slot_props
        .iter()
        .map(|prop| prop.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    let derived_lines = derived_aliases
        .iter()
        .map(|alias| {
            format!(
                "{indent}const {} = $derived({});\n",
                alias.alias, alias.target
            )
        })
        .collect::<String>();
    let derived_spacing = if derived_lines.is_empty() { "" } else { "\n" };

    if is_typescript {
        let members = slot_props
            .iter()
            .map(|prop| {
                let snippet_type = if prop.accepts_args {
                    "Snippet<[any]>"
                } else {
                    "Snippet"
                };
                format!(
                    "{indent}{indent}{}?: import('svelte').{snippet_type};\n",
                    prop.name
                )
            })
            .collect::<String>();
        if use_rest_props {
            format!(
                "{indent}interface Props {{\n{members}{indent}}}\n\n{indent}let {{ ...props }}: Props & {{ [key: string]: any }} = $props();\n{derived_lines}{derived_spacing}"
            )
        } else {
            format!(
                "{indent}interface Props {{\n{members}{indent}}}\n\n{indent}let {{ {destructured} }}: Props = $props();\n{derived_lines}{derived_spacing}"
            )
        }
    } else {
        let property_lines = slot_props
            .iter()
            .map(|prop| {
                let snippet_type = if prop.accepts_args {
                    "Snippet<[any]>"
                } else {
                    "Snippet"
                };
                format!(
                    "{indent} * @property {{import('svelte').{snippet_type}}} [{}]\n",
                    prop.name
                )
            })
            .collect::<String>();
        if use_rest_props {
            format!(
                "{indent}/**\n{indent} * @typedef {{Object}} Props\n{property_lines}{indent} */\n\n{indent}/** @type {{Props & {{ [key: string]: any }}}} */\n{indent}let {{ ...props }} = $props();\n{derived_lines}{derived_spacing}"
            )
        } else {
            format!(
                "{indent}/**\n{indent} * @typedef {{Object}} Props\n{property_lines}{indent} */\n\n{indent}/** @type {{Props}} */\n{indent}let {{ {destructured} }} = $props();\n{derived_lines}{derived_spacing}"
            )
        }
    }
}

fn template_updated_names(fragment: &Fragment) -> HashSet<String> {
    let mut names = HashSet::new();
    fragment.walk(
        &mut names,
        |entry, names| {
            if let crate::ast::modern::Entry::Node(node) = entry
                && let Some(element) = node.as_element()
            {
                for attribute in element.attributes() {
                    if let Some(expression) = event_attribute_expression(attribute) {
                        struct UpdatedNamesVisitor<'a> {
                            names: &'a mut HashSet<String>,
                        }

                        impl<'a> Visit<'a> for UpdatedNamesVisitor<'_> {
                            fn visit_assignment_target(&mut self, target: &AssignmentTarget<'a>) {
                                collect_assignment_target_binding_names(target, self.names);
                                walk::walk_assignment_target(self, target);
                            }

                            fn visit_update_expression(
                                &mut self,
                                expression: &oxc_ast::ast::UpdateExpression<'a>,
                            ) {
                                if let Some(name) = expression.argument.get_identifier_name() {
                                    self.names.insert(name.to_string());
                                }
                                walk::walk_update_expression(self, expression);
                            }
                        }

                        if let Some(expression) = expression.oxc_expression() {
                            let mut visitor = UpdatedNamesVisitor { names };
                            visitor.visit_expression(expression);
                        }
                    }
                }
            }
            crate::ast::modern::Search::<()>::Continue
        },
        |_, _| {},
    );
    names
}

fn event_attribute_expression(attribute: &Attribute) -> Option<&crate::ast::modern::Expression> {
    match attribute {
        Attribute::OnDirective(directive) => Some(&directive.expression),
        Attribute::Attribute(attribute) if attribute.name.starts_with("on") => {
            match &attribute.value {
                AttributeValueList::ExpressionTag(tag) => Some(&tag.expression),
                AttributeValueList::Values(values) if values.len() == 1 => match &values[0] {
                    AttributeValue::ExpressionTag(tag) => Some(&tag.expression),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

fn stateful_names(root: &ModernRoot) -> HashSet<String> {
    let mut names = fragment_bind_targets(&root.fragment);
    names.extend(template_updated_names(&root.fragment));
    if let Some(instance) = root.instance.as_ref() {
        names.extend(script_updated_names(&instance.content));
    }
    names
}

fn state_binding_head(statement_source: &str) -> Option<String> {
    let trimmed = statement_source.trim();
    let remainder = trimmed.strip_prefix("let ")?;
    let end = remainder.find(['=', ';']).unwrap_or(remainder.len());
    Some(remainder[..end].trim().to_string())
}

#[derive(Debug, Clone)]
enum ReactiveBindingRewrite {
    Derived {
        rhs: String,
        statement_start: usize,
        statement_end: usize,
        depends_on_later: bool,
    },
    StateInit {
        rhs: String,
        statement_start: usize,
        statement_end: usize,
    },
}

#[derive(Debug, Clone)]
struct TopLevelLetBinding {
    start: usize,
    end: usize,
    head: String,
    init: Option<String>,
}

fn top_level_let_statement_for_name(
    program: &ParsedJsProgram,
    name: &str,
    source: &str,
    source_offset: usize,
) -> Option<TopLevelLetBinding> {
    for statement in &program.program().body {
        let Statement::VariableDeclaration(statement) = statement else {
            continue;
        };
        if statement.kind.as_str() != "let" {
            continue;
        }
        let declarations = &statement.declarations;
        if declarations.len() != 1 {
            continue;
        }
        let declarator = &declarations[0];
        let Some(id) = declarator.id.get_binding_identifier() else {
            continue;
        };
        if id.name.as_str() != name {
            continue;
        }
        let (start, end) = span_range_with_offset(source_offset, statement.span);
        let statement_source = source.get(start..end)?;
        return Some(TopLevelLetBinding {
            start,
            end,
            head: state_binding_head(statement_source)?,
            init: declarator
                .init
                .as_ref()
                .and_then(|init| {
                    expression_source_from_span_with_offset(source, source_offset, init.span())
                }),
        });
    }
    None
}

struct ReactiveSingleAssignment<'a> {
    name: &'a str,
    right: &'a OxcExpression<'a>,
    rhs: String,
    rhs_is_literal: bool,
    statement_start: usize,
    statement_end: usize,
    has_semicolon: bool,
    /// Comments inside a block before the assignment (e.g., `$: { /* here */ x = 1; }`)
    block_leading_comments: Vec<String>,
    /// Comments inside a block after the assignment (e.g., `$: { x = 1; /* here */ }`)
    block_trailing_comments: Vec<String>,
}

struct ReactiveDestructuringAssignment {
    pattern: String,
    rhs: String,
    statement_start: usize,
    statement_end: usize,
    has_semicolon: bool,
}

fn reactive_binding_rewrites(
    source: &str,
    root: &ModernRoot,
) -> HashMap<String, ReactiveBindingRewrite> {
    let Some(instance) = root.instance.as_ref() else {
        return HashMap::new();
    };
    let declaration_starts = top_level_declaration_starts(&instance.content, instance.content_start);
    let reactive_counts =
        top_level_reactive_assignment_counts(&instance.content, source, instance.content_start);
    let non_reactive_updates = non_reactive_script_updated_names(&instance.content);
    let template_updates = template_updated_names(&root.fragment);
    let mut rewrites = HashMap::new();

    for statement in &instance.content.program().body {
        let Some(reactive_assignment) =
            reactive_single_assignment(statement, source, instance.content_start)
        else {
            continue;
        };
        let Some(binding) =
            top_level_let_statement_for_name(&instance.content, reactive_assignment.name, source, instance.content_start)
        else {
            continue;
        };
        if binding.init.is_some()
            || reactive_counts
                .get(reactive_assignment.name)
                .copied()
                .unwrap_or(0)
                != 1
        {
            continue;
        }
        if reactive_assignment.rhs_is_literal {
            rewrites.insert(
                reactive_assignment.name.to_string(),
                ReactiveBindingRewrite::StateInit {
                    rhs: reactive_assignment.rhs.clone(),
                    statement_start: reactive_assignment.statement_start,
                    statement_end: reactive_assignment.statement_end,
                },
            );
            continue;
        }
        if non_reactive_updates.contains(reactive_assignment.name)
            || template_updates.contains(reactive_assignment.name)
        {
            continue;
        }
        let depends_on_later = rhs_identifier_names(reactive_assignment.right)
            .into_iter()
            .filter_map(|identifier| declaration_starts.get(identifier.as_str()))
            .any(|start| *start > reactive_assignment.statement_start);
        rewrites.insert(
            reactive_assignment.name.to_string(),
            ReactiveBindingRewrite::Derived {
                rhs: reactive_assignment.rhs,
                statement_start: reactive_assignment.statement_start,
                statement_end: reactive_assignment.statement_end,
                depends_on_later,
            },
        );
    }

    rewrites
}

fn reactive_single_assignment<'a>(
    statement: &'a Statement<'a>,
    source: &'a str,
    source_offset: usize,
) -> Option<ReactiveSingleAssignment<'a>> {
    let Statement::LabeledStatement(statement) = statement else {
        return None;
    };
    if statement.label.name.as_str() != "$" {
        return None;
    }
    let (statement_start, statement_end) = span_range_with_offset(source_offset, statement.span);
    let statement_source = source.get(statement_start..statement_end)?;
    if !statement_source.trim_start().starts_with("$:") {
        return None;
    }
    let (expression, has_semicolon, block_leading_comments, block_trailing_comments) = match &statement.body {
        Statement::ExpressionStatement(body) => {
            (&body.expression, statement_source.trim_end().ends_with(';'), Vec::new(), Vec::new())
        }
        Statement::BlockStatement(body) => {
            if body.body.len() != 1 {
                return None;
            }
            let Statement::ExpressionStatement(inner_statement) = &body.body[0] else {
                return None;
            };
            let (inner_start, inner_end) =
                span_range_with_offset(source_offset, inner_statement.span);
            let (block_start, block_end) =
                span_range_with_offset(source_offset, body.span);
            // Extract comments between `{` and the inner statement
            let leading = extract_block_comment_lines(source, block_start + 1, inner_start);
            // Extract comments between the inner statement end and `}`
            let trailing = extract_block_comment_lines(source, inner_end, block_end.saturating_sub(1));
            (
                &inner_statement.expression,
                source
                    .get(inner_start..inner_end)
                    .is_some_and(|inner| inner.trim_end().ends_with(';')),
                leading,
                trailing,
            )
        }
        _ => return None,
    };
    let expression = unwrap_oxc_parenthesized_expression(expression);
    let OxcExpression::AssignmentExpression(expression) = expression else {
        return None;
    };
    let AssignmentTarget::AssignmentTargetIdentifier(left) = &expression.left else {
        return None;
    };
    let right = expression.right.get_inner_expression();

    let rhs_text = expression_source_from_span_with_offset(source, source_offset, right.span())?;
    let rhs = if matches!(right, OxcExpression::SequenceExpression(_)) {
        format!("({rhs_text})")
    } else {
        rhs_text
    };

    Some(ReactiveSingleAssignment {
        name: left.name.as_str(),
        right,
        rhs,
        rhs_is_literal: is_literal_expression(right),
        statement_start,
        statement_end,
        has_semicolon,
        block_leading_comments,
        block_trailing_comments,
    })
}

/// Extract comment lines from a source region (between two positions in a block).
/// Returns each comment line as a string like `//  comment text`.
/// Strips one level of indentation (the block's indentation) but preserves
/// any additional relative indentation as a space prefix.
fn extract_block_comment_lines(source: &str, start: usize, end: usize) -> Vec<String> {
    let Some(region) = source.get(start..end) else {
        return Vec::new();
    };
    let mut comments = Vec::new();
    for line in region.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            // Preserve the text after `//` exactly as-is, but add one leading space
            // to represent the removed block indentation
            let after_slashes = &trimmed[2..];
            comments.push(format!("// {}", after_slashes));
        }
    }
    comments
}

fn span_range_with_offset(offset: usize, span: OxcSpan) -> (usize, usize) {
    (
        offset + span.start as usize,
        offset + span.end as usize,
    )
}

fn expression_source_from_span_with_offset(
    source: &str,
    offset: usize,
    span: OxcSpan,
) -> Option<String> {
    let (start, end) = span_range_with_offset(offset, span);
    source.get(start..end).map(ToString::to_string)
}

fn reactive_destructuring_assignment(
    statement: &Statement<'_>,
    source: &str,
    source_offset: usize,
    declared_names: &HashSet<String>,
) -> Option<ReactiveDestructuringAssignment> {
    let Statement::LabeledStatement(statement) = statement else {
        return None;
    };
    if statement.label.name.as_str() != "$" {
        return None;
    }
    let (statement_start, statement_end) = span_range_with_offset(source_offset, statement.span);
    let statement_source = source.get(statement_start..statement_end)?;
    if !statement_source.trim_start().starts_with("$:") {
        return None;
    }
    let Statement::ExpressionStatement(body) = &statement.body else {
        return None;
    };
    let expression = unwrap_oxc_parenthesized_expression(&body.expression);
    let OxcExpression::AssignmentExpression(expression) = expression else {
        return None;
    };
    if matches!(
        &expression.left,
        AssignmentTarget::AssignmentTargetIdentifier(_)
    ) {
        return None;
    }
    let mut names = HashSet::new();
    collect_assignment_target_binding_names(&expression.left, &mut names);
    if names.is_empty() || names.iter().any(|name| declared_names.contains(name)) {
        return None;
    }
    let right = expression.right.get_inner_expression();
    if is_literal_expression(right) {
        return None;
    }

    let rhs_text = expression_source_from_span_with_offset(source, source_offset, right.span())?;
    let rhs = if matches!(right, OxcExpression::SequenceExpression(_)) {
        format!("({rhs_text})")
    } else {
        rhs_text
    };

    Some(ReactiveDestructuringAssignment {
        pattern: assignment_target_source(source, source_offset, &expression.left)?,
        rhs,
        statement_start,
        statement_end,
        has_semicolon: statement_source.trim_end().ends_with(';'),
    })
}

fn normalize_svelte_ignore_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0usize;

    while cursor < source.len() {
        let tail = &source[cursor..];
        if let Some(rest) = tail.strip_prefix("<!--")
            && let Some(end) = rest.find("-->")
        {
            output.push_str("<!--");
            let comment = &rest[..end];
            output.push_str(
                migrate_svelte_ignore(comment)
                    .as_deref()
                    .unwrap_or(comment),
            );
            output.push_str("-->");
            cursor += 4 + end + 3;
            continue;
        }

        if let Some(rest) = tail.strip_prefix("/*")
            && let Some(end) = rest.find("*/")
        {
            output.push_str("/*");
            let comment = &rest[..end];
            output.push_str(
                migrate_svelte_ignore(comment)
                    .as_deref()
                    .unwrap_or(comment),
            );
            output.push_str("*/");
            cursor += 2 + end + 2;
            continue;
        }

        if let Some(rest) = tail.strip_prefix("//") {
            let line_end = rest.find('\n').unwrap_or(rest.len());
            let comment = &rest[..line_end];
            output.push_str("//");
            output.push_str(
                migrate_svelte_ignore(comment)
                    .as_deref()
                    .unwrap_or(comment),
            );
            cursor += 2 + line_end;
            continue;
        }

        let next = source[cursor..]
            .char_indices()
            .nth(1)
            .map_or(source.len(), |(offset, _)| cursor + offset);
        output.push_str(&source[cursor..next]);
        cursor = next;
    }

    output
}

fn top_level_reactive_assignment_counts(
    program: &ParsedJsProgram,
    source: &str,
    source_offset: usize,
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();

    for statement in &program.program().body {
        let Some(reactive_assignment) =
            reactive_single_assignment(statement, source, source_offset)
        else {
            continue;
        };
        *counts
            .entry(reactive_assignment.name.to_string())
            .or_insert(0) += 1;
    }

    counts
}

fn non_reactive_script_updated_names(program: &ParsedJsProgram) -> HashSet<String> {
    struct UpdatedNamesVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<'a> Visit<'a> for UpdatedNamesVisitor<'_> {
        fn visit_assignment_target(&mut self, target: &AssignmentTarget<'a>) {
            collect_assignment_target_binding_names(target, self.names);
            walk::walk_assignment_target(self, target);
        }

        fn visit_update_expression(&mut self, expression: &oxc_ast::ast::UpdateExpression<'a>) {
            if let Some(name) = expression.argument.get_identifier_name() {
                self.names.insert(name.to_string());
            }
            walk::walk_update_expression(self, expression);
        }
    }

    let mut names = HashSet::new();
    for statement in &program.program().body {
        if matches!(
            statement,
            Statement::LabeledStatement(statement) if statement.label.name.as_str() == "$"
        ) {
            continue;
        }
        let mut visitor = UpdatedNamesVisitor { names: &mut names };
        visitor.visit_statement(statement);
    }

    names
}

fn collect_assignment_target_binding_names(
    node: &AssignmentTarget<'_>,
    names: &mut HashSet<String>,
) {
    match node {
        AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
            names.insert(identifier.name.to_string());
        }
        AssignmentTarget::ArrayAssignmentTarget(target) => {
            for element in target.elements.iter().flatten() {
                collect_assignment_target_maybe_default_names(element, names);
            }
            if let Some(rest) = target.rest.as_ref() {
                collect_assignment_target_binding_names(&rest.target, names);
            }
        }
        AssignmentTarget::ObjectAssignmentTarget(target) => {
            for property in &target.properties {
                match property {
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(
                        property,
                    ) => {
                        names.insert(property.binding.name.to_string());
                    }
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(
                        property,
                    ) => {
                        collect_assignment_target_maybe_default_names(&property.binding, names);
                    }
                }
            }
            if let Some(rest) = target.rest.as_ref() {
                collect_assignment_target_binding_names(&rest.target, names);
            }
        }
        AssignmentTarget::StaticMemberExpression(_)
        | AssignmentTarget::ComputedMemberExpression(_)
        | AssignmentTarget::PrivateFieldExpression(_)
        | AssignmentTarget::TSAsExpression(_)
        | AssignmentTarget::TSSatisfiesExpression(_)
        | AssignmentTarget::TSNonNullExpression(_)
        | AssignmentTarget::TSTypeAssertion(_) => {}
    }
}

fn collect_assignment_target_maybe_default_names(
    node: &oxc_ast::ast::AssignmentTargetMaybeDefault<'_>,
    names: &mut HashSet<String>,
) {
    match node {
        oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(target) => {
            collect_assignment_target_binding_names(&target.binding, names);
        }
        oxc_ast::ast::AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(identifier) => {
            names.insert(identifier.name.to_string());
        }
        oxc_ast::ast::AssignmentTargetMaybeDefault::ArrayAssignmentTarget(target) => {
            for element in target.elements.iter().flatten() {
                collect_assignment_target_maybe_default_names(element, names);
            }
            if let Some(rest) = target.rest.as_ref() {
                collect_assignment_target_binding_names(&rest.target, names);
            }
        }
        oxc_ast::ast::AssignmentTargetMaybeDefault::ObjectAssignmentTarget(target) => {
            for property in &target.properties {
                match property {
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(
                        property,
                    ) => {
                        names.insert(property.binding.name.to_string());
                    }
                    oxc_ast::ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(
                        property,
                    ) => {
                        collect_assignment_target_maybe_default_names(&property.binding, names);
                    }
                }
            }
            if let Some(rest) = target.rest.as_ref() {
                collect_assignment_target_binding_names(&rest.target, names);
            }
        }
        oxc_ast::ast::AssignmentTargetMaybeDefault::StaticMemberExpression(_)
        | oxc_ast::ast::AssignmentTargetMaybeDefault::ComputedMemberExpression(_)
        | oxc_ast::ast::AssignmentTargetMaybeDefault::PrivateFieldExpression(_)
        | oxc_ast::ast::AssignmentTargetMaybeDefault::TSAsExpression(_)
        | oxc_ast::ast::AssignmentTargetMaybeDefault::TSSatisfiesExpression(_)
        | oxc_ast::ast::AssignmentTargetMaybeDefault::TSNonNullExpression(_)
        | oxc_ast::ast::AssignmentTargetMaybeDefault::TSTypeAssertion(_) => {}
    }
}

fn unwrap_oxc_parenthesized_expression<'a>(
    mut node: &'a OxcExpression<'a>,
) -> &'a OxcExpression<'a> {
    while let OxcExpression::ParenthesizedExpression(expression) = node {
        node = &expression.expression;
    }
    node
}

fn top_level_declaration_starts(program: &ParsedJsProgram, source_offset: usize) -> HashMap<String, usize> {
    let mut starts = HashMap::new();
    for statement in &program.program().body {
        let (start, _) = span_range_with_offset(source_offset, statement.span());
        match statement {
            Statement::VariableDeclaration(statement) => {
                for declaration in &statement.declarations {
                    if let Some(id) = declaration.id.get_binding_identifier() {
                        starts.insert(id.name.to_string(), start);
                    }
                }
            }
            Statement::FunctionDeclaration(statement) => {
                if let Some(id) = statement.id.as_ref() {
                    starts.insert(id.name.to_string(), start);
                }
            }
            Statement::ClassDeclaration(statement) => {
                if let Some(id) = statement.id.as_ref() {
                    starts.insert(id.name.to_string(), start);
                }
            }
            _ => {}
        }
    }
    starts
}

fn rhs_identifier_names(node: &OxcExpression<'_>) -> HashSet<String> {
    struct IdentifierVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<'a> Visit<'a> for IdentifierVisitor<'_> {
        fn visit_identifier_reference(
            &mut self,
            identifier: &oxc_ast::ast::IdentifierReference<'a>,
        ) {
            self.names.insert(identifier.name.to_string());
        }
    }

    let mut names = HashSet::new();
    let mut visitor = IdentifierVisitor { names: &mut names };
    visitor.visit_expression(node);
    names
}

fn body_identifier_names(statement: &Statement<'_>) -> HashSet<String> {
    struct IdentifierVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<'a> Visit<'a> for IdentifierVisitor<'_> {
        fn visit_identifier_reference(
            &mut self,
            identifier: &oxc_ast::ast::IdentifierReference<'a>,
        ) {
            self.names.insert(identifier.name.to_string());
        }
    }

    let mut names = HashSet::new();
    let mut visitor = IdentifierVisitor { names: &mut names };
    visitor.visit_statement(statement);
    names
}

fn script_is_typescript(source: &str, script: &Script) -> bool {
    let Some(open_tag) = source.get(script.start..script.content_start) else {
        return false;
    };
    open_tag.contains("lang=\"ts\"") || open_tag.contains("lang='ts'")
}

fn line_start(source: &str, index: usize) -> usize {
    source[..index].rfind('\n').map(|pos| pos + 1).unwrap_or(0)
}

fn line_end_including_newline(source: &str, index: usize) -> usize {
    source[index..]
        .find('\n')
        .map(|offset| index + offset + 1)
        .unwrap_or(source.len())
}

fn statement_has_trailing_blank_line(source: &str, statement_end: usize) -> bool {
    let line_end = line_end_including_newline(source, statement_end);
    let next_non_whitespace = source[line_end..]
        .char_indices()
        .find_map(|(offset, ch)| (!ch.is_whitespace()).then_some(line_end + offset))
        .unwrap_or(source.len());
    source
        .get(line_end..next_non_whitespace)
        .is_some_and(|between| between.contains('\n'))
}

fn statement_blank_line_end(source: &str, statement_end: usize) -> usize {
    let line_end = line_end_including_newline(source, statement_end);
    let next_non_whitespace = source[line_end..]
        .char_indices()
        .find_map(|(offset, ch)| (!ch.is_whitespace()).then_some(line_end + offset))
        .unwrap_or(source.len());
    if next_non_whitespace == source.len() {
        source.len()
    } else {
        line_start(source, next_non_whitespace)
    }
}

fn first_impossible_slot_name_change(
    fragment: &Fragment,
    declared_names: &HashSet<String>,
) -> Option<(String, String)> {
    for node in fragment.nodes.iter() {
        if let Some(change) = node_impossible_slot_name_change(node, declared_names) {
            return Some(change);
        }
    }

    None
}

fn node_impossible_slot_name_change(
    node: &Node,
    declared_names: &HashSet<String>,
) -> Option<(String, String)> {
    if let Node::SlotElement(slot) = node {
        return impossible_slot_element_name_change(slot, declared_names)
            .or_else(|| first_impossible_slot_name_change(&slot.fragment, declared_names));
    }

    node.try_for_each_child_fragment(|fragment| {
        if let Some(change) = first_impossible_slot_name_change(fragment, declared_names) {
            ControlFlow::Break(change)
        } else {
            ControlFlow::Continue(())
        }
    })
    .break_value()
}

fn impossible_slot_element_name_change(
    slot: &crate::ast::modern::SlotElement,
    declared_names: &HashSet<String>,
) -> Option<(String, String)> {
    let slot_name = slot_element_name(slot)?;
    if slot_name == "default" {
        return None;
    }

    let migrated_name = generate_migrated_slot_name(slot_name, declared_names);
    (migrated_name != slot_name).then(|| (slot_name.to_string(), migrated_name))
}

fn slot_element_name(slot: &crate::ast::modern::SlotElement) -> Option<&str> {
    slot.attributes.iter().find_map(|attribute| {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        (attribute.name.as_ref() == "name")
            .then_some(attribute)
            .and_then(static_text_attribute_value)
    })
}

fn static_text_attribute_value(attribute: &crate::ast::modern::NamedAttribute) -> Option<&str> {
    let AttributeValueList::Values(values) = &attribute.value else {
        return None;
    };
    if values.len() != 1 {
        return None;
    }

    match &values[0] {
        AttributeValue::Text(text) => Some(text.data.as_ref()),
        AttributeValue::ExpressionTag(_) => None,
    }
}

fn generate_migrated_slot_name(slot_name: &str, declared_names: &HashSet<String>) -> String {
    let mut preferred_name: String = slot_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if preferred_name.is_empty() {
        preferred_name.push('_');
    } else if preferred_name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
    {
        preferred_name.replace_range(0..1, "_");
    }

    if !declared_names.contains(&preferred_name) && !is_reserved_identifier(&preferred_name) {
        return preferred_name;
    }

    let mut counter = 1usize;
    loop {
        let candidate = format!("{preferred_name}_{counter}");
        if !declared_names.contains(&candidate) && !is_reserved_identifier(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

fn is_reserved_identifier(name: &str) -> bool {
    matches!(
        name,
        "arguments"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "eval"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

#[derive(Debug, Clone, Copy)]
struct TopLevelLetStatement<'a> {
    name: &'a str,
    source: &'a str,
}

#[derive(Debug, Clone, Copy)]
struct TopLevelReactiveAssignment<'a> {
    name: &'a str,
    source: &'a str,
    rhs_is_literal: bool,
}

fn migrate_impossible_rune_binding_conflict(document: &Document, source: &str) -> Option<Arc<str>> {
    let Root::Modern(root) = &document.root else {
        return None;
    };
    let script = root.instance.as_ref()?;
    let declared_names = declared_names_in_program(&script.content);
    let export_let_names = export_let_names(&script.content);
    let bind_targets = prop_bind_targets(root);
    let top_level_lets = top_level_let_statements(&script.content, source, script.content_start);
    let reactive_assignments =
        top_level_reactive_assignments(&script.content, source, script.content_start);

    if declared_names.contains("props") && !export_let_names.is_empty() {
        return Some(migration_task_result(
            source,
            "migrating this component would require adding a `$props` rune but there's already a variable named props.\n     Rename the variable and try again or migrate by hand.",
        ));
    }

    if declared_names.contains("bindable")
        && export_let_names
            .iter()
            .any(|name| bind_targets.contains(name.as_str()))
    {
        return Some(migration_task_result(
            source,
            "migrating this component would require adding a `$bindable` rune but there's already a variable named bindable.\n     Rename the variable and try again or migrate by hand.",
        ));
    }

    if declared_names.contains("derived") {
        if fragment_has_svelte_component(&root.fragment) {
            return Some(migration_task_result(
                source,
                "migrating this component would require adding a `$derived` rune but there's already a variable named derived.\n     Rename the variable and try again or migrate by hand.",
            ));
        }

        for statement in &top_level_lets {
            if reactive_assignments
                .iter()
                .any(|assignment| assignment.name == statement.name && !assignment.rhs_is_literal)
            {
                return Some(migration_task_result(
                    source,
                    &format!(
                        "can't migrate `{}` to `$derived` because there's a variable named derived.\n     Rename the variable and try again or migrate by hand.",
                        statement.source
                    ),
                ));
            }
        }

        if let Some(assignment) = reactive_assignments
            .iter()
            .find(|assignment| !assignment.rhs_is_literal)
        {
            return Some(migration_task_result(
                source,
                &format!(
                    "can't migrate `{}` to `$derived` because there's a variable named derived.\n     Rename the variable and try again or migrate by hand.",
                    assignment.source
                ),
            ));
        }
    }

    if declared_names.contains("state") {
        if let Some(statement) = top_level_lets
            .iter()
            .find(|statement| bind_targets.contains(statement.name))
        {
            return Some(migration_task_result(
                source,
                &format!(
                    "can't migrate `{}` to `$state` because there's a variable named state.\n     Rename the variable and try again or migrate by hand.",
                    statement.source
                ),
            ));
        }

        if let Some(assignment) = reactive_assignments
            .iter()
            .find(|assignment| bind_targets.contains(assignment.name))
        {
            return Some(migration_task_result(
                source,
                &format!(
                    "can't migrate `{}` to `$state` because there's a variable named state.\n     Rename the variable and try again or migrate by hand.",
                    assignment.source
                ),
            ));
        }
    }

    None
}

fn migration_task_result(source: &str, message: &str) -> Arc<str> {
    Arc::from(format!(
        "<!-- @migration-task Error while migrating Svelte code: {message} -->\n{source}"
    ))
}

fn export_let_names(program: &ParsedJsProgram) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in &program.program().body {
        let Some(declaration) = export_let_declaration(statement) else {
            continue;
        };
        for declaration in &declaration.declarations {
            if let Some(id) = declaration.id.get_binding_identifier() {
                names.insert(id.name.to_string());
            }
        }
    }

    names
}

fn top_level_let_statements<'a>(
    program: &'a ParsedJsProgram,
    source: &'a str,
    source_offset: usize,
) -> Vec<TopLevelLetStatement<'a>> {
    let mut statements = Vec::new();
    for statement in &program.program().body {
        let Statement::VariableDeclaration(statement) = statement else {
            continue;
        };
        if statement.kind.as_str() != "let" {
            continue;
        }
        let (start, end) = span_range_with_offset(source_offset, statement.span);
        let Some(statement_source) = source.get(start..end) else {
            continue;
        };
        for declaration in &statement.declarations {
            if let Some(id) = declaration.id.get_binding_identifier() {
                statements.push(TopLevelLetStatement {
                    name: id.name.as_str(),
                    source: statement_source,
                });
            }
        }
    }

    statements
}

fn top_level_reactive_assignments<'a>(
    program: &'a ParsedJsProgram,
    source: &'a str,
    source_offset: usize,
) -> Vec<TopLevelReactiveAssignment<'a>> {
    let mut assignments = Vec::new();
    for statement in &program.program().body {
        let Statement::LabeledStatement(statement) = statement else {
            continue;
        };
        let (start, end) = span_range_with_offset(source_offset, statement.span);
        let Some(statement_source) = source.get(start..end) else {
            continue;
        };
        if !statement_source.trim_start().starts_with("$:") {
            continue;
        }
        let Statement::ExpressionStatement(body) = &statement.body else {
            continue;
        };
        let OxcExpression::AssignmentExpression(expression) =
            unwrap_oxc_parenthesized_expression(&body.expression)
        else {
            continue;
        };
        let AssignmentTarget::AssignmentTargetIdentifier(left) = &expression.left else {
            continue;
        };
        let right = expression.right.get_inner_expression();

        assignments.push(TopLevelReactiveAssignment {
            name: left.name.as_str(),
            source: statement_source,
            rhs_is_literal: is_literal_expression(right),
        });
    }

    assignments
}

fn fragment_bind_targets(fragment: &Fragment) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_fragment_bind_targets(fragment, &mut names);
    names
}

fn collect_fragment_bind_targets(fragment: &Fragment, names: &mut HashSet<String>) {
    for node in fragment.nodes.iter() {
        collect_node_bind_targets(node, names);
    }
}

fn collect_node_bind_targets(node: &Node, names: &mut HashSet<String>) {
    if let Some(element) = node.as_element() {
        collect_attributes_bind_targets(element.attributes(), names);
    }

    node.for_each_child_fragment(|fragment| collect_fragment_bind_targets(fragment, names));
}

fn collect_attributes_bind_targets(attributes: &[Attribute], names: &mut HashSet<String>) {
    for attribute in attributes {
        let Attribute::BindDirective(directive) = attribute else {
            continue;
        };
        if let Some(name) = directive.expression.identifier_name() {
            names.insert(name.to_string());
        }
    }
}

fn fragment_has_svelte_component(fragment: &Fragment) -> bool {
    fragment.nodes.iter().any(node_has_svelte_component)
}

fn node_has_svelte_component(node: &Node) -> bool {
    if matches!(node, Node::SvelteComponent(_)) {
        return true;
    }

    node.try_for_each_child_fragment(|fragment| {
        if fragment_has_svelte_component(fragment) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

fn collect_migrate_edits(
    source: &str,
    document: &Document,
    use_ts: bool,
    filename: Option<&camino::Utf8Path>,
    emit_svelte_self_task: bool,
    edits: &mut Vec<Edit>,
) {
    let Root::Modern(root) = &document.root else {
        return;
    };
    let mut svelte_component_state = SvelteComponentMigrationState {
        used_names: root
            .instance
            .as_ref()
            .map(|instance| declared_names_in_program(&instance.content))
            .unwrap_or_default(),
        ..Default::default()
    };

    if let Some(filename) = filename {
        collect_svelte_self_component_edits(source, root, filename, edits);
    }

    collect_export_alias_props_edits(source, root, edits);
    collect_export_specifier_props_edits(source, root, edits);
    collect_basic_props_edits(source, root, use_ts, edits);
    collect_unused_lifecycle_import_edits(source, root, edits);
    collect_stateful_let_edits(source, root, edits);
    collect_reactive_assignment_edits(source, root, edits);
    collect_reactive_state_run_edits(source, root, edits);
    collect_event_handler_edits(source, root, edits);
    collect_css_selector_migration_edits(source, root, edits);
    collect_component_slot_usage_structure_edits(
        source,
        &root.fragment,
        &mut svelte_component_state,
        edits,
    );
    let mut slot_structure_preview_edits = edits.clone();
    let slot_structure_preview = apply_edits(source, &mut slot_structure_preview_edits);
    let mut dynamic_component_state = SvelteComponentMigrationState {
        used_names: svelte_component_state.used_names.clone(),
        ..Default::default()
    };
    dynamic_component_state
        .used_names
        .extend(generated_svelte_component_names_in_source(
            &slot_structure_preview,
        ));
    collect_dynamic_svelte_component_edits_with_state(
        source,
        root,
        &mut dynamic_component_state,
        edits,
    );
    collect_slot_usage_edits(source, root, use_ts, edits);
    collect_fragment_edits(source, &root.fragment, emit_svelte_self_task, edits);
    collect_root_comment_edits(source, root, edits);
    collect_script_comment_edits(source, root, edits);
    collect_script_attribute_edits(source, root, edits);
    collect_directive_whitespace_edits(source, root, edits);
}

fn collect_fragment_edits(
    source: &str,
    fragment: &Fragment,
    emit_svelte_self_task: bool,
    edits: &mut Vec<Edit>,
) {
    for node in fragment.nodes.iter() {
        collect_node_edits(source, node, emit_svelte_self_task, edits);
    }
}

fn collect_node_edits(
    source: &str,
    node: &Node,
    emit_svelte_self_task: bool,
    edits: &mut Vec<Edit>,
) {
    match node {
        Node::RegularElement(element) => {
            if let Some(edit) = migrate_self_closing_element(source, element) {
                edits.push(edit);
            }
        }
        Node::Comment(comment) => {
            if let Some(edit) = migrate_html_comment(source, comment) {
                edits.push(edit);
            }
        }
        Node::IfBlock(block) => {
            collect_if_block_edits(source, block, emit_svelte_self_task, edits);
            return;
        }
        Node::EachBlock(block) => {
            collect_fragment_edits(source, &block.body, emit_svelte_self_task, edits);
            if let Some(fallback) = block.fallback.as_ref() {
                collect_fragment_edits(source, fallback, emit_svelte_self_task, edits);
            }
            return;
        }
        Node::KeyBlock(block) => {
            collect_key_block_edits(source, block, emit_svelte_self_task, edits);
            return;
        }
        Node::AwaitBlock(block) => {
            collect_await_block_edits(source, block, emit_svelte_self_task, edits);
            return;
        }
        Node::SvelteComponent(component) => {
            collect_static_svelte_component_edits(source, component, edits);
        }
        Node::SvelteElement(element) => {
            if let Some(edit) = migrate_svelte_element_static_this(source, element) {
                edits.push(edit);
            }
        }
        Node::SvelteSelf(component) => {
            if emit_svelte_self_task
                && let Some(edit) = migrate_svelte_self_without_filename(source, component.start)
            {
                edits.push(edit);
            }
        }
        Node::Text(_) | Node::ExpressionTag(_) | Node::RenderTag(_) | Node::DebugTag(_) => {}
        Node::HtmlTag(tag) => {
            if let Some(edit) = trim_braced_segment(source, tag.start, tag.end) {
                edits.push(edit);
            }
        }
        Node::ConstTag(tag) => {
            if let Some(edit) = trim_braced_segment(source, tag.start, tag.end) {
                edits.push(edit);
            }
        }
        _ => {}
    }

    collect_node_child_edits(source, node, emit_svelte_self_task, edits);
}

fn collect_node_child_edits(
    source: &str,
    node: &Node,
    emit_svelte_self_task: bool,
    edits: &mut Vec<Edit>,
) {
    node.for_each_child_fragment(|fragment| {
        collect_fragment_edits(source, fragment, emit_svelte_self_task, edits);
    });
}

fn collect_if_block_edits(
    source: &str,
    block: &IfBlock,
    emit_svelte_self_task: bool,
    edits: &mut Vec<Edit>,
) {
    if let Some(test_end) = expression_end(&block.test)
        && let Some(end) = braced_segment_close(source, test_end)
        && let Some(edit) = trim_braced_segment(source, block.start, end)
    {
        edits.push(edit);
    }

    collect_fragment_edits(source, &block.consequent, emit_svelte_self_task, edits);
    if let Some(alternate) = block.alternate.as_deref() {
        match alternate {
            Alternate::Fragment(fragment) => {
                collect_fragment_edits(source, fragment, emit_svelte_self_task, edits)
            }
            Alternate::IfBlock(block) => {
                collect_if_block_edits(source, block, emit_svelte_self_task, edits)
            }
        }
    }
}

fn collect_await_block_edits(
    source: &str,
    block: &AwaitBlock,
    emit_svelte_self_task: bool,
    edits: &mut Vec<Edit>,
) {
    let open_end = if block.pending.is_some() {
        expression_end(&block.expression)
    } else {
        block.value.as_ref().and_then(expression_end)
    };
    if let Some(expr_end) = open_end
        && let Some(end) = braced_segment_close(source, expr_end)
        && let Some(edit) = trim_braced_segment(source, block.start, end)
    {
        edits.push(edit);
    }

    if block.pending.is_some()
        && let Some(value) = block.value.as_ref()
        && let Some(value_start) = expression_start(value)
        && let Some(start) = braced_segment_open(source, value_start)
        && let Some(value_end) = expression_end(value)
        && let Some(end) = braced_segment_close(source, value_end)
        && let Some(edit) = trim_braced_segment(source, start, end)
    {
        edits.push(edit);
    }

    if let Some(error) = block.error.as_ref()
        && let Some(error_start) = expression_start(error)
        && let Some(start) = braced_segment_open(source, error_start)
        && let Some(error_end) = expression_end(error)
        && let Some(end) = braced_segment_close(source, error_end)
        && let Some(edit) = trim_braced_segment(source, start, end)
    {
        edits.push(edit);
    }

    for fragment in [
        block.pending.as_ref(),
        block.then.as_ref(),
        block.catch.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        collect_fragment_edits(source, fragment, emit_svelte_self_task, edits);
    }
}

fn collect_key_block_edits(
    source: &str,
    block: &KeyBlock,
    emit_svelte_self_task: bool,
    edits: &mut Vec<Edit>,
) {
    if let Some(expression_end) = expression_end(&block.expression)
        && let Some(end) = braced_segment_close(source, expression_end)
        && let Some(edit) = trim_braced_segment(source, block.start, end)
    {
        edits.push(edit);
    }
    collect_fragment_edits(source, &block.fragment, emit_svelte_self_task, edits);
}

fn collect_root_comment_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let Some(comments) = root.comments.as_deref() else {
        return;
    };

    for comment in comments {
        if let Some(edit) = migrate_source_comment(source, comment.start, comment.end) {
            edits.push(edit);
        }
    }
}

fn collect_script_comment_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let _ = (source, root, edits);
}

fn collect_script_attribute_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    for script in root.scripts.iter() {
        if let Some(edit) = migrate_script_context_module(source, script) {
            edits.push(edit);
        }
    }
}

fn collect_directive_whitespace_edits(source: &str, root: &ModernRoot, edits: &mut Vec<Edit>) {
    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    let mut style_ranges = if !root.styles.is_empty() {
        root.styles
            .iter()
            .map(|style| (style.start, style.end))
            .collect::<Vec<_>>()
    } else {
        root.css
            .iter()
            .map(|style| (style.start, style.end))
            .collect::<Vec<_>>()
    };
    style_ranges.sort_unstable_by_key(|(start, _)| *start);
    let mut style_index = 0usize;

    while cursor < bytes.len() {
        while let Some((_, end)) = style_ranges.get(style_index).copied() {
            if cursor >= end {
                style_index += 1;
                continue;
            }
            break;
        }
        if let Some((start, end)) = style_ranges.get(style_index).copied()
            && cursor >= start
            && cursor < end
        {
            cursor = end;
            continue;
        }
        if bytes[cursor] != b'{' {
            cursor += 1;
            continue;
        }

        let mut sigil = cursor + 1;
        while sigil < bytes.len() && bytes[sigil].is_ascii_whitespace() {
            sigil += 1;
        }

        if sigil >= bytes.len() || !matches!(bytes[sigil], b'@' | b'#' | b':') {
            cursor += 1;
            continue;
        }

        let Some(end) = find_directive_segment_end(source, cursor) else {
            cursor += 1;
            continue;
        };

        if let Some(edit) = trim_braced_segment(source, cursor, end) {
            edits.push(edit);
        }
        cursor = end;
    }
}

fn collect_svelte_self_component_edits(
    source: &str,
    root: &ModernRoot,
    filename: &camino::Utf8Path,
    edits: &mut Vec<Edit>,
) {
    let Some(base_component_name) = component_name_from_filename(filename) else {
        return;
    };
    let component_name = if let Some(instance) = root.instance.as_ref() {
        unique_component_name(
            &base_component_name,
            declared_names_in_program(&instance.content),
        )
    } else {
        base_component_name
    };

    let mut stats = SvelteSelfRewriteStats::default();
    collect_svelte_self_fragment_edits(source, &root.fragment, &component_name, &mut stats, edits);

    if !stats.has_svelte_self {
        return;
    }

    let Some(file_name) = filename.file_name() else {
        return;
    };

    if let Some(instance) = root.instance.as_ref() {
        let insertion_point =
            first_non_whitespace(source, instance.content_start, instance.content_end)
                .unwrap_or(instance.content_end);
        let indent = leading_whitespace_before(source, insertion_point).unwrap_or("\t");
        let mut replacement = format!("import {component_name} from './{file_name}';\n{indent}");
        if stats.needs_props {
            replacement.push_str("/** @type {{ [key: string]: any }} */\n");
            replacement.push_str(indent);
            replacement.push_str("let { ...props } = $props();\n");
            replacement.push_str(indent);
        }
        edits.push(Edit {
            start: insertion_point,
            end: insertion_point,
            replacement,
        });
        return;
    }

    let mut replacement = format!("<script>\n\timport {component_name} from './{file_name}';");
    if stats.needs_props {
        replacement
            .push_str("\n\t/** @type {{ [key: string]: any }} */\n\tlet { ...props } = $props();");
    }
    replacement.push_str("\n</script>\n\n");

    edits.push(Edit {
        start: 0,
        end: 0,
        replacement,
    });
}

#[derive(Default)]
struct SvelteSelfRewriteStats {
    has_svelte_self: bool,
    needs_props: bool,
}

fn collect_svelte_self_fragment_edits(
    source: &str,
    fragment: &Fragment,
    component_name: &str,
    stats: &mut SvelteSelfRewriteStats,
    edits: &mut Vec<Edit>,
) {
    for node in fragment.nodes.iter() {
        collect_svelte_self_node_edits(source, node, component_name, stats, edits);
    }
}

fn collect_svelte_self_node_edits(
    source: &str,
    node: &Node,
    component_name: &str,
    stats: &mut SvelteSelfRewriteStats,
    edits: &mut Vec<Edit>,
) {
    if let Node::SvelteSelf(component) = node {
        let Some(raw) = source.get(component.start..component.end) else {
            return;
        };
        stats.has_svelte_self = true;
        stats.needs_props |= raw.contains("$$props.");
        edits.push(Edit {
            start: component.start,
            end: component.end,
            replacement: raw
                .replace("svelte:self", component_name)
                .replace("$$props.", "props."),
        });
    }

    node.for_each_child_fragment(|fragment| {
        collect_svelte_self_fragment_edits(source, fragment, component_name, stats, edits);
    });
}

fn migrate_html_comment(source: &str, comment: &Comment) -> Option<Edit> {
    migrate_source_comment(source, comment.start, comment.end)
}

fn migrate_source_comment(source: &str, start: usize, end: usize) -> Option<Edit> {
    let raw = source.get(start..end)?;
    let replacement = rewrite_comment_source(raw)?;
    Some(Edit {
        start,
        end,
        replacement,
    })
}

fn rewrite_comment_source(raw: &str) -> Option<String> {
    if let Some(inner) = raw
        .strip_prefix("<!--")
        .and_then(|tail| tail.strip_suffix("-->"))
    {
        let migrated = migrate_svelte_ignore(inner)?;
        return Some(format!("<!--{migrated}-->"));
    }

    if let Some(inner) = raw.strip_prefix("//") {
        let migrated = migrate_svelte_ignore(inner)?;
        return Some(format!("//{migrated}"));
    }

    if let Some(inner) = raw
        .strip_prefix("/*")
        .and_then(|tail| tail.strip_suffix("*/"))
    {
        let migrated = migrate_svelte_ignore(inner)?;
        return Some(format!("/*{migrated}*/"));
    }

    None
}

fn trim_braced_segment(source: &str, start: usize, end: usize) -> Option<Edit> {
    let raw = source.get(start..end)?;
    if !raw.starts_with('{') || !raw.ends_with('}') || raw.len() < 2 {
        return None;
    }

    let inner = &raw[1..raw.len() - 1];
    let trimmed = inner.trim();
    if trimmed.len() == inner.len() {
        return None;
    }

    Some(Edit {
        start,
        end,
        replacement: format!("{{{trimmed}}}"),
    })
}

fn find_directive_segment_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = start + 1;
    let mut depth = 1usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_template = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escaped = false;

    while cursor < bytes.len() {
        let byte = bytes[cursor];

        if in_line_comment {
            if byte == b'\n' {
                in_line_comment = false;
            }
            cursor += 1;
            continue;
        }

        if in_block_comment {
            if byte == b'*' && bytes.get(cursor + 1).copied() == Some(b'/') {
                in_block_comment = false;
                cursor += 2;
            } else {
                cursor += 1;
            }
            continue;
        }

        if escaped {
            escaped = false;
            cursor += 1;
            continue;
        }

        if matches!(byte, b'\\') && (in_single || in_double || in_template) {
            escaped = true;
            cursor += 1;
            continue;
        }

        if in_single {
            if byte == b'\'' {
                in_single = false;
            }
            cursor += 1;
            continue;
        }

        if in_double {
            if byte == b'"' {
                in_double = false;
            }
            cursor += 1;
            continue;
        }

        if in_template {
            if byte == b'`' {
                in_template = false;
            }
            cursor += 1;
            continue;
        }

        match byte {
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b'`' => in_template = true,
            b'/' if bytes.get(cursor + 1).copied() == Some(b'/') => {
                in_line_comment = true;
                cursor += 2;
                continue;
            }
            b'/' if bytes.get(cursor + 1).copied() == Some(b'*') => {
                in_block_comment = true;
                cursor += 2;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }

        cursor += 1;
    }

    None
}

fn braced_segment_close(source: &str, start: usize) -> Option<usize> {
    source
        .get(start..)?
        .find('}')
        .map(|offset| start + offset + 1)
}

fn braced_segment_open(source: &str, end: usize) -> Option<usize> {
    source.get(..end)?.rfind('{')
}

fn expression_start(expression: &crate::ast::modern::Expression) -> Option<usize> {
    Some(expression.start)
}

fn expression_end(expression: &crate::ast::modern::Expression) -> Option<usize> {
    Some(expression.end)
}

fn migrate_svelte_element_static_this(source: &str, element: &SvelteElement) -> Option<Edit> {
    let expression = element.expression.as_ref()?;
    expression_literal_string(expression)?;

    let start = expression.start;
    let end = expression.end;
    if start == 0 || end >= source.len() {
        return None;
    }

    let bytes = source.as_bytes();
    let mut cursor = start;
    let mut is_static = true;

    while cursor > element.start {
        cursor -= 1;
        match bytes[cursor] {
            b'=' => break,
            b'{' => {
                is_static = false;
                break;
            }
            _ => {}
        }
    }

    if !is_static || bytes.get(cursor).copied() != Some(b'=') {
        return None;
    }

    let quote = bytes.get(start - 1).copied()?;
    if !matches!(quote, b'\'' | b'"') || bytes.get(end).copied() != Some(quote) {
        return None;
    }

    let quote_start = start - 1;
    let quote_end = end + 1;
    Some(Edit {
        start: quote_start,
        end: quote_end,
        replacement: format!("{{{}}}", &source[quote_start..quote_end]),
    })
}

fn collect_static_svelte_component_edits(
    source: &str,
    component: &crate::ast::modern::SvelteComponent,
    edits: &mut Vec<Edit>,
) {
    let Some(expression) = component.expression.as_ref() else {
        return;
    };
    if expression.identifier_name().is_none() {
        return;
    }
    let Some(name) = source
        .get(expression.start..expression.end)
        .map(ToString::to_string)
    else {
        return;
    };
    if !is_static_component_identifier(name.trim()) {
        return;
    }
    let Some(open_name_end) = component.name.len().checked_add(component.start + 1) else {
        return;
    };
    edits.push(Edit {
        start: component.start + 1,
        end: open_name_end,
        replacement: name.clone(),
    });

    remove_svelte_component_this_attribute(source, component, edits);

    let Some(close_start) = source
        .get(component.start..component.end)
        .and_then(|raw| raw.rfind("</"))
        .map(|offset| component.start + offset + 2)
    else {
        return;
    };
    edits.push(Edit {
        start: close_start,
        end: close_start + component.name.len(),
        replacement: name,
    });
}

fn migrate_script_context_module(source: &str, script: &Script) -> Option<Edit> {
    for attribute in script.attributes.iter() {
        let Attribute::Attribute(attribute) = attribute else {
            continue;
        };
        if attribute.name.as_ref() != "context" {
            continue;
        }
        if named_attribute_string_value(attribute) != Some("module") {
            continue;
        }

        let raw = source.get(attribute.start..attribute.end)?;
        if raw == "module" {
            continue;
        }

        return Some(Edit {
            start: attribute.start,
            end: attribute.end,
            replacement: "module".to_string(),
        });
    }

    None
}

fn migrate_svelte_self_without_filename(source: &str, start: usize) -> Option<Edit> {
    Some(Edit {
        start,
        end: start,
        replacement: format!(
            "<!-- @migration-task: svelte:self is deprecated, import this Svelte file into itself instead -->\n{}",
            line_indent_at(source, start)?
        ),
    })
}

fn named_attribute_string_value(attribute: &crate::ast::modern::NamedAttribute) -> Option<&str> {
    match &attribute.value {
        AttributeValueList::Values(values) => match values.as_ref() {
            [AttributeValue::Text(text)] => Some(text.data.as_ref()),
            _ => None,
        },
        AttributeValueList::ExpressionTag(_) | AttributeValueList::Boolean(_) => None,
    }
}

fn has_svelte_options_accessors(source: &str) -> bool {
    let mut cursor = 0usize;
    while let Some(offset) = source[cursor..].find("<svelte:options") {
        let start = cursor + offset;
        let Some(end_offset) = source[start..].find('>') else {
            break;
        };
        let end = start + end_offset + 1;
        if source[start..end].contains("accessors") {
            return true;
        }
        cursor = end;
    }
    false
}

fn collect_svelte_options_accessors_edits(source: &str, edits: &mut Vec<Edit>) {
    let mut cursor = 0usize;
    while let Some(offset) = source[cursor..].find("<svelte:options") {
        let start = cursor + offset;
        let Some(end_offset) = source[start..].find('>') else {
            break;
        };
        let end = start + end_offset + 1;
        let raw = &source[start..end];
        if raw.contains("accessors") {
            let cleaned = raw
                .replacen(" accessors", "", 1)
                .replacen("accessors ", "", 1)
                .replace("  ", " ");
            edits.push(Edit {
                start,
                end,
                replacement: cleaned,
            });
        }
        cursor = end;
    }
}

fn slot_usage_attribute_name(attributes: &[Attribute]) -> Option<&str> {
    attributes.iter().find_map(|attribute| {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        (attribute.name.as_ref() == "slot")
            .then_some(attribute)
            .and_then(static_text_attribute_value)
    })
}

fn normalize_slot_identifier(name: &str) -> String {
    if name == "default" {
        "children".to_string()
    } else {
        name.to_string()
    }
}

fn slot_render_argument_source(source: &str, attributes: &[Attribute]) -> Option<String> {
    let mut arguments = String::new();

    for attribute in attributes {
        let Attribute::Attribute(attribute) = attribute else {
            return None;
        };
        if matches!(attribute.name.as_ref(), "name" | "slot") {
            continue;
        }

        let value = match &attribute.value {
            AttributeValueList::Boolean(true) => attribute.name.to_string(),
            AttributeValueList::Boolean(false) => return None,
            AttributeValueList::ExpressionTag(tag) => source
                .get(expression_start(&tag.expression)?..expression_end(&tag.expression)?)?
                .trim()
                .to_string(),
            AttributeValueList::Values(values) => match values.as_ref() {
                [AttributeValue::Text(text)] => format!("{:?}", text.data),
                [AttributeValue::ExpressionTag(tag)] => source
                    .get(expression_start(&tag.expression)?..expression_end(&tag.expression)?)?
                    .trim()
                    .to_string(),
                _ => return None,
            },
        };

        if value == attribute.name.as_ref() {
            arguments.push_str(&format!("{value}, "));
        } else {
            arguments.push_str(&format!("{}: {value}, ", attribute.name));
        }
    }

    if arguments.is_empty() {
        Some(String::new())
    } else {
        Some(format!("{{ {arguments}}}"))
    }
}

fn line_indent_at(source: &str, index: usize) -> Option<&str> {
    let line_start = source
        .get(..index)?
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let line = source.get(line_start..index)?;
    let indent_len = line
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    Some(&line[..indent_len])
}

fn guess_indent(source: &str) -> &str {
    let lines = source.lines().collect::<Vec<_>>();
    let tabbed = lines.iter().filter(|line| line.starts_with('\t')).count();
    let spaced = lines
        .iter()
        .filter_map(|line| {
            let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
            (spaces >= 2).then_some(spaces)
        })
        .collect::<Vec<_>>();

    if tabbed == 0 && spaced.is_empty() {
        return "\t";
    }
    if tabbed >= spaced.len() {
        return "\t";
    }

    match spaced.into_iter().min() {
        Some(2) => "  ",
        Some(3) => "   ",
        Some(4) => "    ",
        Some(5) => "     ",
        Some(6) => "      ",
        Some(7) => "       ",
        Some(8) => "        ",
        _ => "\t",
    }
}

fn component_name_from_filename(filename: &camino::Utf8Path) -> Option<String> {
    let stem = filename.file_stem()?;
    let mut name = String::new();
    let mut uppercase_next = true;

    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            if uppercase_next {
                name.push(ch.to_ascii_uppercase());
                uppercase_next = false;
            } else {
                name.push(ch);
            }
        } else {
            uppercase_next = true;
        }
    }

    if name.is_empty() { None } else { Some(name) }
}

fn migrate_self_closing_element(source: &str, element: &RegularElement) -> Option<Edit> {
    if !element.self_closing || element.has_end_tag || !should_expand_self_closing(&element.name) {
        return None;
    }

    let raw = source.get(element.start..element.end)?;
    if !raw.ends_with("/>") {
        return None;
    }

    let mut trim_end = raw.len().saturating_sub(2);
    while trim_end > 0 && raw.as_bytes()[trim_end - 1].is_ascii_whitespace() {
        trim_end -= 1;
    }

    Some(Edit {
        start: element.start,
        end: element.end,
        replacement: format!("{}></{}>", &raw[..trim_end], element.name),
    })
}

fn should_expand_self_closing(name: &str) -> bool {
    let local_name = name.rsplit(':').next().unwrap_or(name);
    !is_void_element_name(local_name) && !is_svg_element_name(local_name)
}

fn is_svg_element_name(name: &str) -> bool {
    matches!(
        name,
        "altGlyph"
            | "altGlyphDef"
            | "altGlyphItem"
            | "animate"
            | "animateColor"
            | "animateMotion"
            | "animateTransform"
            | "circle"
            | "clipPath"
            | "color-profile"
            | "cursor"
            | "defs"
            | "desc"
            | "discard"
            | "ellipse"
            | "feBlend"
            | "feColorMatrix"
            | "feComponentTransfer"
            | "feComposite"
            | "feConvolveMatrix"
            | "feDiffuseLighting"
            | "feDisplacementMap"
            | "feDistantLight"
            | "feDropShadow"
            | "feFlood"
            | "feFuncA"
            | "feFuncB"
            | "feFuncG"
            | "feFuncR"
            | "feGaussianBlur"
            | "feImage"
            | "feMerge"
            | "feMergeNode"
            | "feMorphology"
            | "feOffset"
            | "fePointLight"
            | "feSpecularLighting"
            | "feSpotLight"
            | "feTile"
            | "feTurbulence"
            | "filter"
            | "font"
            | "font-face"
            | "font-face-format"
            | "font-face-name"
            | "font-face-src"
            | "font-face-uri"
            | "foreignObject"
            | "g"
            | "glyph"
            | "glyphRef"
            | "hatch"
            | "hatchpath"
            | "hkern"
            | "image"
            | "line"
            | "linearGradient"
            | "marker"
            | "mask"
            | "mesh"
            | "meshgradient"
            | "meshpatch"
            | "meshrow"
            | "metadata"
            | "missing-glyph"
            | "mpath"
            | "path"
            | "pattern"
            | "polygon"
            | "polyline"
            | "radialGradient"
            | "rect"
            | "set"
            | "solidcolor"
            | "stop"
            | "svg"
            | "switch"
            | "symbol"
            | "text"
            | "textPath"
            | "tref"
            | "tspan"
            | "unknown"
            | "use"
            | "view"
            | "vkern"
    )
}

fn apply_edits(source: &str, edits: &mut [Edit]) -> String {
    edits.sort_by(|left, right| {
        let left_insertion = left.start == left.end;
        let right_insertion = right.start == right.end;
        left.start
            .cmp(&right.start)
            .then_with(|| match (left_insertion, right_insertion) {
                (true, true) | (false, false) => right.end.cmp(&left.end),
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
            })
    });

    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;

    for edit in edits {
        if edit.start < cursor {
            continue;
        }
        output.push_str(&source[cursor..edit.start]);
        output.push_str(&edit.replacement);
        cursor = edit.end;
    }

    output.push_str(&source[cursor..]);
    output
}

fn unique_component_name(base: &str, declared_names: HashSet<String>) -> String {
    if !declared_names.contains(base) {
        return base.to_string();
    }

    let mut counter = 1usize;
    loop {
        let candidate = format!("{base}_{counter}");
        if !declared_names.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

fn unique_generated_name(base: &str, used_names: &mut HashSet<String>) -> String {
    if used_names.insert(base.to_string()) {
        return base.to_string();
    }

    let mut index = 1usize;
    loop {
        let candidate = format!("{base}_{index}");
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn declared_names_in_program(program: &ParsedJsProgram) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in &program.program().body {
        collect_declared_names_from_node(statement, &mut names);
    }

    names
}

fn collect_declared_names_from_node(statement: &Statement<'_>, names: &mut HashSet<String>) {
    match statement {
        Statement::ImportDeclaration(statement) => {
            if let Some(specifiers) = statement.specifiers.as_ref() {
                for specifier in specifiers {
                    match specifier {
                        oxc_ast::ast::ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                            names.insert(specifier.local.name.to_string());
                        }
                        oxc_ast::ast::ImportDeclarationSpecifier::ImportDefaultSpecifier(
                            specifier,
                        ) => {
                            names.insert(specifier.local.name.to_string());
                        }
                        oxc_ast::ast::ImportDeclarationSpecifier::ImportNamespaceSpecifier(
                            specifier,
                        ) => {
                            names.insert(specifier.local.name.to_string());
                        }
                    }
                }
            }
        }
        Statement::VariableDeclaration(statement) => {
            collect_declared_names_from_variable_declaration(statement, names);
        }
        Statement::FunctionDeclaration(statement) => {
            if let Some(id) = statement.id.as_ref() {
                names.insert(id.name.to_string());
            }
        }
        Statement::ClassDeclaration(statement) => {
            if let Some(id) = statement.id.as_ref() {
                names.insert(id.name.to_string());
            }
        }
        Statement::ExportNamedDeclaration(statement) => {
            if let Some(declaration) = statement.declaration.as_ref() {
                match declaration {
                    Declaration::VariableDeclaration(statement) => {
                        collect_declared_names_from_variable_declaration(statement, names);
                    }
                    Declaration::FunctionDeclaration(statement) => {
                        if let Some(id) = statement.id.as_ref() {
                            names.insert(id.name.to_string());
                        }
                    }
                    Declaration::ClassDeclaration(statement) => {
                        if let Some(id) = statement.id.as_ref() {
                            names.insert(id.name.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        Statement::ExportDefaultDeclaration(statement) => match &statement.declaration {
            oxc_ast::ast::ExportDefaultDeclarationKind::FunctionDeclaration(statement) => {
                if let Some(id) = statement.id.as_ref() {
                    names.insert(id.name.to_string());
                }
            }
            oxc_ast::ast::ExportDefaultDeclarationKind::ClassDeclaration(statement) => {
                if let Some(id) = statement.id.as_ref() {
                    names.insert(id.name.to_string());
                }
            }
            oxc_ast::ast::ExportDefaultDeclarationKind::Identifier(identifier) => {
                names.insert(identifier.name.to_string());
            }
            _ => {}
        },
        _ => {}
    }
}

fn first_non_whitespace(source: &str, start: usize, end: usize) -> Option<usize> {
    let slice = source.get(start..end)?;
    let offset = slice
        .char_indices()
        .find_map(|(offset, ch)| (!ch.is_whitespace()).then_some(offset))?;
    Some(start + offset)
}

fn leading_whitespace_before(source: &str, index: usize) -> Option<&str> {
    let line_start = source
        .get(..index)?
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let line = source.get(line_start..index)?;
    if line.chars().all(char::is_whitespace) {
        Some(line)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::migrate;
    use crate::MigrateOptions;

    #[test]
    fn migrate_expands_self_closing_non_void_elements() {
        let input =
            "<div />\n<div title=\"preserve\" />\n<input type=\"text\" />\n<hr />\n<f:table />";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<div></div>\n<div title=\"preserve\"></div>\n<input type=\"text\" />\n<hr />\n<f:table></f:table>"
        );
    }

    #[test]
    fn migrate_preserves_svg_self_closing_elements() {
        let input = "<svg />\n<path />";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(result.code.as_ref(), input);
    }

    #[test]
    fn migrate_rewrites_svelte_ignore_comments() {
        let input = "<script>\n\t// svelte-ignore non-top-level-reactive-declaration\n\t/* svelte-ignore a11y-something-something a11y-something-something2 */\n</script>\n\n<!-- svelte-ignore a11y-something-something -->";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script>\n\t// svelte-ignore reactive_declaration_invalid_placement\n\t/* svelte-ignore a11y_something_something, a11y_something_something2 */\n</script>\n\n<!-- svelte-ignore a11y_something_something -->"
        );
    }

    #[test]
    fn migrate_wraps_static_svelte_element_this_attributes() {
        let input = "<svelte:element this=\"div\" />\n<svelte:element this='div' />\n<svelte:element this={\"div\"} />\n<svelte:element this=\"h{n}\" />";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<svelte:element this={\"div\"} />\n<svelte:element this={'div'} />\n<svelte:element this={\"div\"} />\n<svelte:element this=\"h{n}\" />"
        );
    }

    #[test]
    fn migrate_rewrites_script_context_module_attribute() {
        let input = "<script context=\"module\">\n\tlet foo = true;\n</script>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script module>\n\tlet foo = true;\n</script>"
        );
    }

    #[test]
    fn migrate_trims_block_delimiter_whitespace() {
        let input = "{  @html \"some html\"   }\n\n{     #if ok  }\n\ttrue\n{    :else if nope  }\n\tfalse\n{/if}\n\n{     #await []    }\n\t{   @const x = 43   }\n\t{x}\n{   :then i   }\n\t{i}\n{  :catch e  }\n\tdlkdj\n{/await}\n\n{   #await [] then i   }\nstuff\n{/await}\n\n{      #key count    }\n\tdlkdj\n{/key}";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "{@html \"some html\"}\n\n{#if ok}\n\ttrue\n{:else if nope}\n\tfalse\n{/if}\n\n{#await []}\n\t{@const x = 43}\n\t{x}\n{:then i}\n\t{i}\n{:catch e}\n\tdlkdj\n{/await}\n\n{#await [] then i}\nstuff\n{/await}\n\n{#key count}\n\tdlkdj\n{/key}"
        );
    }

    #[test]
    fn migrate_inserts_task_for_svelte_self_without_filename() {
        let input = "{#if false}\n\t<svelte:self />\n{/if}";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "{#if false}\n\t<!-- @migration-task: svelte:self is deprecated, import this Svelte file into itself instead -->\n\t<svelte:self />\n{/if}"
        );
    }

    #[test]
    fn migrate_rewrites_svelte_self_with_filename() {
        let input = "{#if false}\n\t<svelte:self count={$$props.count} />\n{/if}";
        let result = migrate(
            input,
            MigrateOptions {
                filename: Some(camino::Utf8PathBuf::from("output.svelte")),
                use_ts: false,
            },
        )
        .expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script>\n\timport Output from './output.svelte';\n\t/** @type {{ [key: string]: any }} */\n\tlet { ...props } = $props();\n</script>\n\n{#if false}\n\t<Output count={props.count} />\n{/if}"
        );
    }

    #[test]
    fn migrate_rewrites_svelte_self_with_filename_name_conflict() {
        let input = "<script>\n\tlet Output;\n</script>\n\n{#if false}\n\t<svelte:self count={$$props.count} />\n{/if}";
        let result = migrate(
            input,
            MigrateOptions {
                filename: Some(camino::Utf8PathBuf::from("output.svelte")),
                use_ts: false,
            },
        )
        .expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script>\n\timport Output_1 from './output.svelte';\n\t/** @type {{ [key: string]: any }} */\n\tlet { ...props } = $props();\n\tlet Output;\n</script>\n\n{#if false}\n\t<Output_1 count={props.count} />\n{/if}"
        );
    }

    #[test]
    fn migrate_marks_before_after_update_as_manual() {
        let input = "<script>\n\timport { beforeUpdate, afterUpdate } from \"svelte\";\n\n\tbeforeUpdate(() => {});\n\tafterUpdate(() => {});\n</script>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: Can't migrate code with beforeUpdate and afterUpdate. Please migrate by hand. -->\n<script>\n\timport { beforeUpdate, afterUpdate } from \"svelte\";\n\n\tbeforeUpdate(() => {});\n\tafterUpdate(() => {});\n</script>"
        );
    }

    #[test]
    fn migrate_marks_non_identifier_export_pattern_as_manual() {
        let input = "<script>\n\texport let { value } = props;\n</script>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: Encountered an export declaration pattern that is not supported for automigration. -->\n<script>\n\texport let { value } = props;\n</script>"
        );
    }

    #[test]
    fn migrate_marks_named_props_with_dollar_props_as_manual() {
        let input = "<script>\n\texport let value = 42;\n</script>\n\n{$$props}";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: $$props is used together with named props in a way that cannot be automatically migrated. -->\n<script>\n\texport let value = 42;\n</script>\n\n{$$props}"
        );
    }

    #[test]
    fn migrate_marks_slot_name_conflict_as_manual() {
        let input = "<script>\n\tlet body;\n</script>\n\n<slot name=\"body\"></slot>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: This migration would change the name of a slot (body to body_1) making the component unusable -->\n<script>\n\tlet body;\n</script>\n\n<slot name=\"body\"></slot>"
        );
    }

    #[test]
    fn migrate_marks_invalid_slot_name_as_manual() {
        let input = "<slot name=\"dashed-name\"></slot>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: This migration would change the name of a slot (dashed-name to dashed_name) making the component unusable -->\n<slot name=\"dashed-name\"></slot>"
        );
    }

    #[test]
    fn migrate_marks_props_rune_name_conflict_as_manual() {
        let input = "<script>\n\tlet props;\n\texport let value;\n</script>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: migrating this component would require adding a `$props` rune but there's already a variable named props.\n     Rename the variable and try again or migrate by hand. -->\n<script>\n\tlet props;\n\texport let value;\n</script>"
        );
    }

    #[test]
    fn migrate_marks_state_rune_name_conflict_as_manual() {
        let input = "<script>\n\tlet state = 'world';\n\n\t$: other = 42;\n</script>\n\n<input bind:value={other} />";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: can't migrate `$: other = 42;` to `$state` because there's a variable named state.\n     Rename the variable and try again or migrate by hand. -->\n<script>\n\tlet state = 'world';\n\n\t$: other = 42;\n</script>\n\n<input bind:value={other} />"
        );
    }

    #[test]
    fn migrate_marks_derived_rune_name_conflict_as_manual() {
        let input = "<script>\n\tlet derived;\n</script>\n\n<svelte:component this={derived} />";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: migrating this component would require adding a `$derived` rune but there's already a variable named derived.\n     Rename the variable and try again or migrate by hand. -->\n<script>\n\tlet derived;\n</script>\n\n<svelte:component this={derived} />"
        );
    }

    #[test]
    fn migrate_marks_parse_errors_as_manual() {
        let input = "<script\n\nunterminated template";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<!-- @migration-task Error while migrating Svelte code: Unexpected end of input\nhttps://svelte.dev/e/unexpected_eof -->\n<script\n\nunterminated template"
        );
    }

    #[test]
    fn migrate_rewrites_simple_export_let_to_props() {
        let input = "<script>\n\texport let name;\n</script>\n\n<div>{name}</div>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script>\n\tlet { name } = $props();\n</script>\n\n<div>{name}</div>"
        );
    }

    #[test]
    fn migrate_rewrites_unused_export_let_with_dollar_props_to_rest_props() {
        let input = "<script>\n\texport let stuff;\n\n\tconsole.log($$props);\n</script>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script>\n\tlet { ...props } = $props();\n\n\tconsole.log(props);\n</script>"
        );
    }

    #[test]
    fn migrate_rewrites_simple_typed_export_let_to_typed_props() {
        let input = "<script lang=\"ts\">\n\timport type { $Test } from './types';\n  \n\texport let data: $Test;\n  </script>";
        let result = migrate(
            input,
            MigrateOptions {
                filename: None,
                use_ts: true,
            },
        )
        .expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script lang=\"ts\">\n\timport type { $Test } from './types';\n  \n\tinterface Props {\n\t\tdata: $Test;\n\t}\n\n\tlet { data }: Props = $props();\n  </script>"
        );
    }

    #[test]
    fn migrate_rewrites_rest_props() {
        let input = "<script>\n    export let foo;\n</script>\n\n<button {foo} {...$$restProps}>click me</button>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script>\n    let { foo, ...rest } = $props();\n</script>\n\n<button {foo} {...rest}>click me</button>"
        );
    }

    #[test]
    fn migrate_rewrites_top_level_reactive_assignment_to_derived() {
        let input = "<script>\n\t$: writable = !readonly;\n</script>";
        let result = migrate(input, MigrateOptions::default()).expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script>\n\tlet writable = $derived(!readonly);\n</script>"
        );
    }

    #[test]
    fn migrate_rewrites_export_alias_props() {
        let input = "<script lang=\"ts\">\n\tlet klass = '';\n\texport { klass as class }\n</script>\n\n{klass}";
        let result = migrate(
            input,
            MigrateOptions {
                filename: None,
                use_ts: true,
            },
        )
        .expect("migrate succeeds");

        assert_eq!(
            result.code.as_ref(),
            "<script lang=\"ts\">\n\tinterface Props {\n\t\tclass?: string;\n\t}\n\n\tlet { class: klass = '' }: Props = $props();\n\t\n</script>\n\n{klass}"
        );
    }
}
