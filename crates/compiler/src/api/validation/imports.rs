use super::*;
use crate::ast::modern::{Fragment, Node};
use crate::{SourceId, SourceText};
use oxc_ast::ast::{
    AssignmentTarget, BindingPattern, Declaration, ExportDefaultDeclarationKind,
    Expression as OxcExpression, ImportDeclarationSpecifier, ModuleExportName, Statement,
    VariableDeclaration,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::{GetSpan, Span};
use std::collections::BTreeMap;
use std::sync::Arc;
use svelte_syntax::ParsedJsProgram;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ExportMode {
    Component,
    Module,
}

pub(super) fn detect_import_svelte_internal_forbidden(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    for script in [&root.module, &root.instance] {
        let Some(script) = script.as_ref() else {
            continue;
        };
        let offset = script.content_start;
        if let Some((start, end)) = find_import_svelte_internal_in_program(script.content.as_ref())
        {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::ImportSvelteInternalForbidden,
                start + offset,
                end + offset,
            ));
        }
    }
    None
}

pub(super) fn detect_import_svelte_internal(
    source: &str,
    program: &ParsedJsProgram,
) -> Option<CompileError> {
    let (start, end) = find_import_svelte_internal_in_program(program)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::ImportSvelteInternalForbidden,
        start,
        end,
    ))
}

pub(super) fn detect_export_rules_in_module_scripts(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    if let Some(instance) = root.instance.as_ref()
        && let Some((start, end)) = find_any_export_default(instance.content.as_ref())
    {
        let offset = instance.content_start;
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::ModuleIllegalDefaultExport,
            start + offset,
            end + offset,
        ));
    }

    let module = root.module.as_ref()?;
    let mut exportable_snippets = NameSet::default();
    collect_exportable_snippet_names(&root.fragment, &mut exportable_snippets);
    detect_export_rules(
        source,
        module.content.as_ref(),
        &exportable_snippets,
        ExportMode::Component,
        module.content_start,
    )
}

pub(super) fn detect_declaration_duplicate_module_import(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let module = root.module.as_ref()?;
    let instance = root.instance.as_ref()?;

    let imported = collect_module_import_local_names(module.content.as_ref());
    if imported.is_empty() {
        return None;
    }

    let offset = instance.content_start;
    let (start, end) =
        find_duplicate_module_import_declaration(instance.content.as_ref(), &imported)?;
    Some(compile_error_custom_imports(
        source,
        "declaration_duplicate_module_import",
        "Cannot declare a variable with the same name as an import inside `<script module>`",
        start + offset,
        end + offset,
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuneDeclKind {
    State,
    Derived,
}

#[derive(Clone)]
struct RuneDecl {
    kind: RuneDeclKind,
    name: Arc<str>,
    statement_start: usize,
    statement_end: usize,
    exported_direct: bool,
}

pub(super) fn detect_export_rules(
    source: &str,
    program: &ParsedJsProgram,
    additional_exportables: &NameSet,
    mode: ExportMode,
    offset: usize,
) -> Option<CompileError> {
    let body = &program.program().body;
    if mode == ExportMode::Component
        && let Some((start, end)) = find_illegal_default_export_in_body(body)
    {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::ModuleIllegalDefaultExport,
            start + offset,
            end + offset,
        ));
    }

    let rune_decls = collect_rune_decls(body);
    let reassignments = collect_reassignments(program);

    for decl in rune_decls
        .iter()
        .filter(|decl| decl.kind == RuneDeclKind::Derived)
    {
        if decl.exported_direct {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::DerivedInvalidExport,
                decl.statement_start + offset,
                decl.statement_end + offset,
            ));
        }
        if let Some((start, end)) = find_export_default_of(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::DerivedInvalidExport,
                start + offset,
                end + offset,
            ));
        }
        if let Some((start, end)) = find_export_list_name(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::DerivedInvalidExport,
                start + offset,
                end + offset,
            ));
        }
    }

    for decl in rune_decls
        .iter()
        .filter(|decl| decl.kind == RuneDeclKind::State)
    {
        let reassigned = reassignments
            .get(decl.name.as_ref())
            .is_some_and(|(start, _)| *start > decl.statement_end);
        if !reassigned {
            continue;
        }

        if decl.exported_direct {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidExport,
                decl.statement_start + offset,
                decl.statement_end + offset,
            ));
        }
        if let Some((start, end)) = find_export_default_of(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidExport,
                start + offset,
                end + offset,
            ));
        }
        if let Some((start, end)) = find_export_list_name(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidExport,
                start + offset,
                end + offset,
            ));
        }
    }

    if let Some((name, start, end)) = find_undefined_export_name(body, additional_exportables) {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::ExportUndefined { name },
            start + offset,
            end + offset,
        ));
    }

    None
}

fn find_import_svelte_internal_in_program(program: &ParsedJsProgram) -> Option<(usize, usize)> {
    for statement in &program.program().body {
        let Statement::ImportDeclaration(declaration) = statement else {
            continue;
        };
        let import_source = declaration.source.value.as_str();
        if !import_source.contains("svelte/internal/") {
            continue;
        }
        return Some(span_range(declaration.source.span));
    }
    None
}

fn collect_rune_decls(body: &[Statement<'_>]) -> Vec<RuneDecl> {
    let mut out = Vec::new();
    for statement in body {
        match statement {
            Statement::VariableDeclaration(declaration) => {
                collect_rune_decls_from_variable_declaration(
                    declaration,
                    false,
                    Some(statement.span()),
                    &mut out,
                );
            }
            Statement::ExportNamedDeclaration(declaration) => {
                let exported_direct = declaration.source.is_none();
                if let Some(Declaration::VariableDeclaration(variable)) =
                    declaration.declaration.as_ref()
                {
                    collect_rune_decls_from_variable_declaration(
                        variable,
                        exported_direct,
                        if exported_direct {
                            Some(declaration.span)
                        } else {
                            None
                        },
                        &mut out,
                    );
                }
            }
            _ => {}
        }
    }
    out
}

fn collect_rune_decls_from_variable_declaration(
    declaration: &VariableDeclaration<'_>,
    exported_direct: bool,
    statement_span: Option<Span>,
    out: &mut Vec<RuneDecl>,
) {
    let Some(statement_span) = statement_span.or_else(|| Some(declaration.span)) else {
        return;
    };
    let (statement_start, statement_end) = span_range(statement_span);

    for declarator in &declaration.declarations {
        let Some(name) = binding_pattern_identifier_name(&declarator.id) else {
            continue;
        };
        let Some(init) = declarator.init.as_ref() else {
            continue;
        };
        let OxcExpression::CallExpression(call) = init.get_inner_expression() else {
            continue;
        };
        let kind = match callee_name(&call.callee).as_deref() {
            Some("$state") => RuneDeclKind::State,
            Some("$derived") => RuneDeclKind::Derived,
            _ => continue,
        };
        out.push(RuneDecl {
            kind,
            name: Arc::from(name),
            statement_start,
            statement_end,
            exported_direct,
        });
    }
}

fn collect_reassignments(program: &ParsedJsProgram) -> BTreeMap<Arc<str>, (usize, usize)> {
    struct ReassignmentVisitor {
        out: BTreeMap<Arc<str>, (usize, usize)>,
    }

    impl<'a> Visit<'a> for ReassignmentVisitor {
        fn visit_assignment_expression(&mut self, it: &oxc_ast::ast::AssignmentExpression<'a>) {
            if let Some((name, span)) = assignment_target_identifier_name_and_span(&it.left) {
                self.out.entry(Arc::from(name)).or_insert(span_range(span));
            }
            walk::walk_assignment_expression(self, it);
        }

        fn visit_update_expression(&mut self, it: &oxc_ast::ast::UpdateExpression<'a>) {
            if let Some(name) = it.argument.get_identifier_name() {
                self.out
                    .entry(Arc::from(name))
                    .or_insert(span_range(it.argument.span()));
            }
            walk::walk_update_expression(self, it);
        }
    }

    let mut visitor = ReassignmentVisitor {
        out: BTreeMap::new(),
    };
    visitor.visit_program(program.program());
    visitor.out
}

fn find_export_default_of(body: &[Statement<'_>], name: &str) -> Option<(usize, usize)> {
    for statement in body {
        let Statement::ExportDefaultDeclaration(declaration) = statement else {
            continue;
        };
        if export_default_identifier_name(&declaration.declaration) == Some(name) {
            return Some(span_range(declaration.span));
        }
    }
    None
}

fn find_any_export_default(program: &ParsedJsProgram) -> Option<(usize, usize)> {
    find_illegal_default_export_in_body(&program.program().body)
}

fn find_illegal_default_export_in_body(body: &[Statement<'_>]) -> Option<(usize, usize)> {
    for statement in body {
        match statement {
            Statement::ExportDefaultDeclaration(declaration) => {
                return Some(span_range(declaration.span));
            }
            Statement::ExportNamedDeclaration(declaration) if declaration.source.is_none() => {
                let exports_default = declaration
                    .specifiers
                    .iter()
                    .any(|specifier| specifier.exported.name().as_ref() == "default");
                if exports_default {
                    return Some(span_range(declaration.span));
                }
            }
            _ => {}
        }
    }

    None
}

fn find_export_list_name(body: &[Statement<'_>], name: &str) -> Option<(usize, usize)> {
    for statement in body {
        let Statement::ExportNamedDeclaration(declaration) = statement else {
            continue;
        };
        if declaration.source.is_some() {
            continue;
        }
        for specifier in &declaration.specifiers {
            if module_export_name_as_str(&specifier.local) == Some(name) {
                return Some(span_range(specifier.local.span()));
            }
        }
    }
    None
}

fn find_undefined_export_name(
    body: &[Statement<'_>],
    additional_exportables: &NameSet,
) -> Option<(Arc<str>, usize, usize)> {
    let declared = collect_declared_names(body);
    for statement in body {
        let Statement::ExportNamedDeclaration(declaration) = statement else {
            continue;
        };
        if declaration.source.is_some() {
            continue;
        }
        for specifier in &declaration.specifiers {
            let Some(name) = module_export_name_as_str(&specifier.local) else {
                continue;
            };
            if declared.contains(name) || additional_exportables.contains(name) {
                continue;
            }
            let (start, end) = span_range(specifier.local.span());
            return Some((Arc::from(name), start, end));
        }
    }
    None
}

fn collect_exportable_snippet_names(fragment: &Fragment, out: &mut NameSet) {
    for node in &fragment.nodes {
        collect_exportable_snippet_names_in_node(node, out);
        node.for_each_child_fragment(|child| collect_exportable_snippet_names(child, out));
    }
}

fn collect_exportable_snippet_names_in_node(node: &Node, out: &mut NameSet) {
    match node {
        Node::SnippetBlock(block) => {
            if let Some(name) = expression_identifier_name(&block.expression) {
                out.insert(name);
            }
        }
        Node::Text(_)
        | Node::Comment(_)
        | Node::ExpressionTag(_)
        | Node::RenderTag(_)
        | Node::HtmlTag(_)
        | Node::DebugTag(_)
        | Node::ConstTag(_)
        | Node::IfBlock(_)
        | Node::EachBlock(_)
        | Node::AwaitBlock(_)
        | Node::KeyBlock(_) => {}
        _ => {}
    }
}

fn collect_declared_names(body: &[Statement<'_>]) -> NameSet {
    let mut declared = NameSet::default();
    for statement in body {
        collect_declared_names_from_statement(statement, &mut declared);
    }
    declared
}

fn collect_declared_names_from_statement(statement: &Statement<'_>, declared: &mut NameSet) {
    match statement {
        Statement::ImportDeclaration(declaration) => {
            if let Some(specifiers) = declaration.specifiers.as_ref() {
                for specifier in specifiers {
                    if let Some(name) = import_specifier_local_name(specifier) {
                        declared.insert(Arc::from(name));
                    }
                }
            }
        }
        Statement::VariableDeclaration(declaration) => {
            collect_declared_names_from_variable(declaration, declared);
        }
        Statement::FunctionDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                declared.insert(Arc::from(id.name.as_str()));
            }
        }
        Statement::ClassDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                declared.insert(Arc::from(id.name.as_str()));
            }
        }
        Statement::TSTypeAliasDeclaration(declaration) => {
            declared.insert(Arc::from(declaration.id.name.as_str()));
        }
        Statement::TSInterfaceDeclaration(declaration) => {
            declared.insert(Arc::from(declaration.id.name.as_str()));
        }
        Statement::TSEnumDeclaration(declaration) => {
            declared.insert(Arc::from(declaration.id.name.as_str()));
        }
        Statement::TSModuleDeclaration(declaration) => {
            if let Some(name) = ts_module_declaration_name(declaration) {
                declared.insert(Arc::from(name));
            }
        }
        Statement::ExportNamedDeclaration(declaration) => {
            if let Some(inner) = declaration.declaration.as_ref() {
                collect_declared_names_from_declaration(inner, declared);
            }
        }
        _ => {}
    }
}

fn collect_declared_names_from_declaration(declaration: &Declaration<'_>, declared: &mut NameSet) {
    match declaration {
        Declaration::VariableDeclaration(declaration) => {
            collect_declared_names_from_variable(declaration, declared)
        }
        Declaration::FunctionDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                declared.insert(Arc::from(id.name.as_str()));
            }
        }
        Declaration::ClassDeclaration(declaration) => {
            if let Some(id) = declaration.id.as_ref() {
                declared.insert(Arc::from(id.name.as_str()));
            }
        }
        Declaration::TSTypeAliasDeclaration(declaration) => {
            declared.insert(Arc::from(declaration.id.name.as_str()));
        }
        Declaration::TSInterfaceDeclaration(declaration) => {
            declared.insert(Arc::from(declaration.id.name.as_str()));
        }
        Declaration::TSEnumDeclaration(declaration) => {
            declared.insert(Arc::from(declaration.id.name.as_str()));
        }
        Declaration::TSModuleDeclaration(declaration) => {
            if let Some(name) = ts_module_declaration_name(declaration) {
                declared.insert(Arc::from(name));
            }
        }
        _ => {}
    }
}

fn collect_declared_names_from_variable(
    declaration: &VariableDeclaration<'_>,
    declared: &mut NameSet,
) {
    for declarator in &declaration.declarations {
        extend_name_set_with_binding_pattern(declared, &declarator.id);
    }
}

fn collect_module_import_local_names(program: &ParsedJsProgram) -> NameSet {
    let mut names = NameSet::default();

    for statement in &program.program().body {
        let Statement::ImportDeclaration(declaration) = statement else {
            continue;
        };
        let Some(specifiers) = declaration.specifiers.as_ref() else {
            continue;
        };
        for specifier in specifiers {
            let Some(name) = import_specifier_local_name(specifier) else {
                continue;
            };
            names.insert(Arc::from(name));
        }
    }

    names
}

fn find_duplicate_module_import_declaration(
    program: &ParsedJsProgram,
    imported: &NameSet,
) -> Option<(usize, usize)> {
    for statement in &program.program().body {
        let Statement::VariableDeclaration(declaration) = statement else {
            continue;
        };
        for declarator in &declaration.declarations {
            if let Some(span) = find_duplicate_import_binding_span(&declarator.id, imported) {
                return Some(span);
            }
        }
    }

    None
}

fn find_duplicate_import_binding_span(
    pattern: &BindingPattern<'_>,
    imported: &NameSet,
) -> Option<(usize, usize)> {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            let name = identifier.name.as_str();
            imported.contains(name).then(|| span_range(identifier.span))
        }
        BindingPattern::AssignmentPattern(pattern) => {
            find_duplicate_import_binding_span(&pattern.left, imported)
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                if let Some(span) = find_duplicate_import_binding_span(element, imported) {
                    return Some(span);
                }
            }
            if let Some(rest) = pattern.rest.as_ref() {
                return find_duplicate_import_binding_span(&rest.argument, imported);
            }
            None
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                if let Some(span) = find_duplicate_import_binding_span(&property.value, imported) {
                    return Some(span);
                }
            }
            if let Some(rest) = pattern.rest.as_ref() {
                return find_duplicate_import_binding_span(&rest.argument, imported);
            }
            None
        }
    }
}

fn span_range(span: Span) -> (usize, usize) {
    (span.start as usize, span.end as usize)
}

fn compile_error_custom_imports(
    source: &str,
    code: &'static str,
    message: impl Into<Arc<str>>,
    start: usize,
    end: usize,
) -> CompileError {
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let start_location = source_text.location_at_offset(start);
    let end_location = source_text.location_at_offset(end);

    CompileError {
        code: Arc::from(code),
        message: message.into(),
        position: Some(Box::new(SourcePosition {
            start: start_location.character,
            end: end_location.character,
        })),
        start: Some(Box::new(start_location)),
        end: Some(Box::new(end_location)),
        filename: None,
    }
}

fn import_specifier_local_name<'a>(
    specifier: &'a ImportDeclarationSpecifier<'a>,
) -> Option<&'a str> {
    match specifier {
        ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
        ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
        ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
            Some(specifier.local.name.as_str())
        }
    }
}

fn module_export_name_as_str<'a>(name: &'a ModuleExportName<'a>) -> Option<&'a str> {
    match name {
        ModuleExportName::IdentifierName(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::IdentifierReference(identifier) => Some(identifier.name.as_str()),
        ModuleExportName::StringLiteral(_) => None,
    }
}

fn export_default_identifier_name<'a>(
    declaration: &'a ExportDefaultDeclarationKind<'a>,
) -> Option<&'a str> {
    match declaration {
        ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
            function.id.as_ref().map(|id| id.name.as_str())
        }
        ExportDefaultDeclarationKind::ClassDeclaration(class) => {
            class.id.as_ref().map(|id| id.name.as_str())
        }
        ExportDefaultDeclarationKind::Identifier(identifier) => Some(identifier.name.as_str()),
        _ => None,
    }
}

fn callee_name(callee: &OxcExpression<'_>) -> Option<String> {
    match callee.get_inner_expression() {
        OxcExpression::Identifier(reference) => Some(reference.name.as_str().to_owned()),
        OxcExpression::StaticMemberExpression(member) => {
            let object = member.object.get_inner_expression();
            let OxcExpression::Identifier(object) = object else {
                return None;
            };
            Some(format!("{}.{}", object.name, member.property.name))
        }
        _ => None,
    }
}

fn binding_pattern_identifier_name<'a>(pattern: &'a BindingPattern<'a>) -> Option<&'a str> {
    pattern
        .get_binding_identifier()
        .map(|identifier| identifier.name.as_str())
}

fn extend_name_set_with_binding_pattern(names: &mut NameSet, pattern: &BindingPattern<'_>) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            names.insert(Arc::from(identifier.name.as_str()));
        }
        BindingPattern::AssignmentPattern(pattern) => {
            extend_name_set_with_binding_pattern(names, &pattern.left);
        }
        BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                extend_name_set_with_binding_pattern(names, element);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                extend_name_set_with_binding_pattern(names, &rest.argument);
            }
        }
        BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                extend_name_set_with_binding_pattern(names, &property.value);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                extend_name_set_with_binding_pattern(names, &rest.argument);
            }
        }
    }
}

fn assignment_target_identifier_name_and_span<'a>(
    target: &'a AssignmentTarget<'a>,
) -> Option<(&'a str, Span)> {
    match target {
        AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
            Some((identifier.name.as_str(), identifier.span))
        }
        _ => None,
    }
}

fn ts_module_declaration_name<'a>(
    declaration: &'a oxc_ast::ast::TSModuleDeclaration<'a>,
) -> Option<&'a str> {
    match &declaration.id {
        oxc_ast::ast::TSModuleDeclarationName::Identifier(identifier) => {
            Some(identifier.name.as_str())
        }
        _ => None,
    }
}
