use super::*;
use crate::ast::modern::{EstreeNode, EstreeValue, Fragment, Node};
use crate::estree::{export_specifier_exported_name, raw_callee_name, raw_identifier_name};
use crate::{SourceId, SourceText};
use std::collections::BTreeMap;
use std::sync::Arc;

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
        if let Some((start, end)) = find_import_svelte_internal_in_program(&script.content) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::ImportSvelteInternalForbidden,
                start,
                end,
            ));
        }
    }
    None
}

pub(super) fn detect_import_svelte_internal(
    source: &str,
    program: &EstreeNode,
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
        && let Some((start, end)) = find_any_export_default(&instance.content)
    {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::ModuleIllegalDefaultExport,
            start,
            end,
        ));
    }

    let module = root.module.as_ref()?;
    let mut exportable_snippets = NameSet::default();
    collect_exportable_snippet_names(&root.fragment, &mut exportable_snippets);
    detect_export_rules(
        source,
        &module.content,
        &exportable_snippets,
        ExportMode::Component,
    )
}

pub(super) fn detect_declaration_duplicate_module_import(
    source: &str,
    root: &Root,
) -> Option<CompileError> {
    let module = root.module.as_ref()?;
    let instance = root.instance.as_ref()?;

    let imported = collect_module_import_local_names(&module.content);
    if imported.is_empty() {
        return None;
    }

    let (start, end) = find_duplicate_module_import_declaration(&instance.content, &imported)?;
    Some(compile_error_custom_imports(
        source,
        "declaration_duplicate_module_import",
        "Cannot declare a variable with the same name as an import inside `<script module>`",
        start,
        end,
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
    program: &EstreeNode,
    additional_exportables: &NameSet,
    mode: ExportMode,
) -> Option<CompileError> {
    let body = estree_node_field_array(program, RawField::Body)?;
    if mode == ExportMode::Component
        && let Some((start, end)) = find_illegal_default_export_in_body(body)
    {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::ModuleIllegalDefaultExport,
            start,
            end,
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
                decl.statement_start,
                decl.statement_end,
            ));
        }
        if let Some((start, end)) = find_export_default_of(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::DerivedInvalidExport,
                start,
                end,
            ));
        }
        if let Some((start, end)) = find_export_list_name(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::DerivedInvalidExport,
                start,
                end,
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
                decl.statement_start,
                decl.statement_end,
            ));
        }
        if let Some((start, end)) = find_export_default_of(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidExport,
                start,
                end,
            ));
        }
        if let Some((start, end)) = find_export_list_name(body, decl.name.as_ref()) {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::StateInvalidExport,
                start,
                end,
            ));
        }
    }

    if let Some((name, start, end)) = find_undefined_export_name(body, additional_exportables) {
        return Some(compile_error_with_range(
            source,
            CompilerDiagnosticKind::ExportUndefined { name },
            start,
            end,
        ));
    }

    None
}

fn find_import_svelte_internal_in_program(program: &EstreeNode) -> Option<(usize, usize)> {
    let body = estree_node_field_array(program, RawField::Body)?;
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ImportDeclaration") {
            continue;
        }
        let Some(source_literal) = estree_node_field_object(statement, RawField::Source) else {
            continue;
        };
        let Some(import_source) = estree_node_literal_string(source_literal) else {
            continue;
        };
        if !import_source.contains("svelte/internal/") {
            continue;
        }
        if let Some((start, end)) = estree_node_span(source_literal) {
            return Some((start, end));
        }
    }
    None
}

fn collect_rune_decls(body: &[EstreeValue]) -> Vec<RuneDecl> {
    let mut out = Vec::new();
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        match estree_node_type(statement) {
            Some("VariableDeclaration") => {
                collect_rune_decls_from_variable_declaration(
                    statement,
                    false,
                    estree_node_span(statement),
                    &mut out,
                );
            }
            Some("ExportNamedDeclaration") => {
                let exported_direct =
                    estree_node_field_object(statement, RawField::Source).is_none();
                if let Some(declaration) =
                    estree_node_field_object(statement, RawField::Declaration)
                    && estree_node_type(declaration) == Some("VariableDeclaration")
                {
                    collect_rune_decls_from_variable_declaration(
                        declaration,
                        exported_direct,
                        if exported_direct {
                            estree_node_span(statement)
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
    declaration: &EstreeNode,
    exported_direct: bool,
    statement_span: Option<(usize, usize)>,
    out: &mut Vec<RuneDecl>,
) {
    let Some((statement_start, statement_end)) =
        statement_span.or_else(|| estree_node_span(declaration))
    else {
        return;
    };
    let Some(declarations) = estree_node_field_array(declaration, RawField::Declarations) else {
        return;
    };
    for declarator in declarations {
        let EstreeValue::Object(declarator) = declarator else {
            continue;
        };
        let Some(id) = estree_node_field_object(declarator, RawField::Id) else {
            continue;
        };
        let Some(name) = raw_identifier_name(id) else {
            continue;
        };
        let Some(init) = estree_node_field_object(declarator, RawField::Init) else {
            continue;
        };
        if estree_node_type(init) != Some("CallExpression") {
            continue;
        }
        let Some(callee) = estree_node_field_object(init, RawField::Callee) else {
            continue;
        };
        let kind = match raw_callee_name(callee).as_deref() {
            Some("$state") => RuneDeclKind::State,
            Some("$derived") => RuneDeclKind::Derived,
            _ => continue,
        };
        out.push(RuneDecl {
            kind,
            name,
            statement_start,
            statement_end,
            exported_direct,
        });
    }
}

fn collect_reassignments(program: &EstreeNode) -> BTreeMap<Arc<str>, (usize, usize)> {
    let mut out = BTreeMap::<Arc<str>, (usize, usize)>::new();
    walk_estree_node(program, &mut |node| match estree_node_type(node) {
        Some("AssignmentExpression") => {
            let Some(left) = estree_node_field_object(node, RawField::Left) else {
                return;
            };
            let Some(name) = raw_identifier_name(left) else {
                return;
            };
            let Some(span) = estree_node_span(left) else {
                return;
            };
            out.entry(name).or_insert(span);
        }
        Some("UpdateExpression") => {
            let Some(argument) = estree_node_field_object(node, RawField::Argument) else {
                return;
            };
            let Some(name) = raw_identifier_name(argument) else {
                return;
            };
            let Some(span) = estree_node_span(argument) else {
                return;
            };
            out.entry(name).or_insert(span);
        }
        _ => {}
    });
    out
}

fn find_export_default_of(body: &[EstreeValue], name: &str) -> Option<(usize, usize)> {
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ExportDefaultDeclaration") {
            continue;
        }
        let declaration = estree_node_field_object(statement, RawField::Declaration)?;
        if raw_identifier_name(declaration).as_deref() == Some(name) {
            return estree_node_span(statement).or_else(|| estree_node_span(declaration));
        }
    }
    None
}

fn find_any_export_default(program: &EstreeNode) -> Option<(usize, usize)> {
    let body = estree_node_field_array(program, RawField::Body)?;
    find_illegal_default_export_in_body(body)
}

fn find_illegal_default_export_in_body(body: &[EstreeValue]) -> Option<(usize, usize)> {
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        match estree_node_type(statement) {
            Some("ExportDefaultDeclaration") => {
                return estree_node_span(statement).or_else(|| {
                    estree_node_field_object(statement, RawField::Declaration)
                        .and_then(estree_node_span)
                });
            }
            Some("ExportNamedDeclaration")
                if estree_node_field_object(statement, RawField::Source).is_none() =>
            {
                let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers)
                else {
                    continue;
                };
                let exports_default = specifiers.iter().any(|specifier| {
                    let EstreeValue::Object(specifier) = specifier else {
                        return false;
                    };
                    export_specifier_exported_name(specifier).as_deref() == Some("default")
                });
                if exports_default {
                    return estree_node_span(statement);
                }
            }
            _ => {}
        }
    }

    None
}

fn find_export_list_name(body: &[EstreeValue], name: &str) -> Option<(usize, usize)> {
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ExportNamedDeclaration")
            || estree_node_field_object(statement, RawField::Source).is_some()
        {
            continue;
        }
        let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers) else {
            continue;
        };
        for specifier in specifiers {
            let EstreeValue::Object(specifier) = specifier else {
                continue;
            };
            let local = estree_node_field_object(specifier, RawField::Local)?;
            if raw_identifier_name(local).as_deref() == Some(name) {
                return estree_node_span(local).or_else(|| estree_node_span(specifier));
            }
        }
    }
    None
}

fn find_undefined_export_name(
    body: &[EstreeValue],
    additional_exportables: &NameSet,
) -> Option<(Arc<str>, usize, usize)> {
    let declared = collect_declared_names(body);
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ExportNamedDeclaration")
            || estree_node_field_object(statement, RawField::Source).is_some()
        {
            continue;
        }
        let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers) else {
            continue;
        };
        for specifier in specifiers {
            let EstreeValue::Object(specifier) = specifier else {
                continue;
            };
            let Some(local) = estree_node_field_object(specifier, RawField::Local) else {
                continue;
            };
            let Some(name) = raw_identifier_name(local) else {
                continue;
            };
            if declared.contains(name.as_ref()) || additional_exportables.contains(name.as_ref()) {
                continue;
            }
            let (start, end) = estree_node_span(local).or_else(|| estree_node_span(specifier))?;
            return Some((name, start, end));
        }
    }
    None
}

fn collect_exportable_snippet_names(fragment: &Fragment, out: &mut NameSet) {
    for node in fragment.nodes.iter() {
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

fn collect_declared_names(body: &[EstreeValue]) -> NameSet {
    let mut declared = NameSet::default();
    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        match estree_node_type(statement) {
            Some("ImportDeclaration") => {
                let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers)
                else {
                    continue;
                };
                for specifier in specifiers {
                    let EstreeValue::Object(specifier) = specifier else {
                        continue;
                    };
                    let Some(local) = estree_node_field_object(specifier, RawField::Local) else {
                        continue;
                    };
                    if let Some(name) = raw_identifier_name(local) {
                        declared.insert(name);
                    }
                }
            }
            Some("VariableDeclaration") => {
                collect_declared_names_from_variable(statement, &mut declared)
            }
            Some("FunctionDeclaration" | "ClassDeclaration") => {
                if let Some(id) = estree_node_field_object(statement, RawField::Id)
                    && let Some(name) = raw_identifier_name(id)
                {
                    declared.insert(name);
                }
            }
            Some(
                "TSInterfaceDeclaration"
                | "TSTypeAliasDeclaration"
                | "TSEnumDeclaration"
                | "TSModuleDeclaration",
            ) => {
                if let Some(id) = estree_node_field_object(statement, RawField::Id)
                    .or_else(|| estree_node_field_object(statement, RawField::Name))
                    && let Some(name) = raw_identifier_name(id)
                {
                    declared.insert(name);
                }
            }
            Some("ExportNamedDeclaration") => {
                if let Some(declaration) =
                    estree_node_field_object(statement, RawField::Declaration)
                {
                    match estree_node_type(declaration) {
                        Some("VariableDeclaration") => {
                            collect_declared_names_from_variable(declaration, &mut declared);
                        }
                        Some("FunctionDeclaration" | "ClassDeclaration") => {
                            if let Some(id) = estree_node_field_object(declaration, RawField::Id)
                                && let Some(name) = raw_identifier_name(id)
                            {
                                declared.insert(name);
                            }
                        }
                        Some(
                            "TSInterfaceDeclaration"
                            | "TSTypeAliasDeclaration"
                            | "TSEnumDeclaration"
                            | "TSModuleDeclaration",
                        ) => {
                            if let Some(id) = estree_node_field_object(declaration, RawField::Id)
                                .or_else(|| estree_node_field_object(declaration, RawField::Name))
                                && let Some(name) = raw_identifier_name(id)
                            {
                                declared.insert(name);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    declared
}

fn collect_declared_names_from_variable(declaration: &EstreeNode, declared: &mut NameSet) {
    let Some(declarations) = estree_node_field_array(declaration, RawField::Declarations) else {
        return;
    };
    for declarator in declarations {
        let EstreeValue::Object(declarator) = declarator else {
            continue;
        };
        let Some(id) = estree_node_field_object(declarator, RawField::Id) else {
            continue;
        };
        extend_name_set_with_pattern_bindings(declared, id);
    }
}

fn collect_module_import_local_names(program: &EstreeNode) -> NameSet {
    let mut names = NameSet::default();
    let Some(body) = estree_node_field_array(program, RawField::Body) else {
        return names;
    };

    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("ImportDeclaration") {
            continue;
        }
        let Some(specifiers) = estree_node_field_array(statement, RawField::Specifiers) else {
            continue;
        };
        for specifier in specifiers {
            let EstreeValue::Object(specifier) = specifier else {
                continue;
            };
            let Some(local) = estree_node_field_object(specifier, RawField::Local) else {
                continue;
            };
            let Some(name) = raw_identifier_name(local) else {
                continue;
            };
            names.insert(name);
        }
    }

    names
}

fn find_duplicate_module_import_declaration(
    program: &EstreeNode,
    imported: &NameSet,
) -> Option<(usize, usize)> {
    let body = estree_node_field_array(program, RawField::Body)?;

    for statement in body {
        let EstreeValue::Object(statement) = statement else {
            continue;
        };
        if estree_node_type(statement) != Some("VariableDeclaration") {
            continue;
        }
        let Some(declarations) = estree_node_field_array(statement, RawField::Declarations) else {
            continue;
        };
        for declarator in declarations {
            let EstreeValue::Object(declarator) = declarator else {
                continue;
            };
            let Some(id) = estree_node_field_object(declarator, RawField::Id) else {
                continue;
            };
            if let Some(span) = find_duplicate_import_binding_span(id, imported) {
                return Some(span);
            }
        }
    }

    None
}

fn find_duplicate_import_binding_span(
    pattern: &EstreeNode,
    imported: &NameSet,
) -> Option<(usize, usize)> {
    match estree_node_type(pattern) {
        Some("Identifier") => {
            let name = estree_node_field_str(pattern, RawField::Name)?;
            if !imported.contains(name) {
                return None;
            }
            estree_node_span(pattern)
        }
        Some("RestElement") => {
            let argument = estree_node_field_object(pattern, RawField::Argument)?;
            find_duplicate_import_binding_span(argument, imported)
        }
        Some("AssignmentPattern") => {
            let left = estree_node_field_object(pattern, RawField::Left)?;
            find_duplicate_import_binding_span(left, imported)
        }
        Some("ArrayPattern") => {
            let elements = estree_node_field_array(pattern, RawField::Elements)?;
            for element in elements {
                let EstreeValue::Object(element) = element else {
                    continue;
                };
                if let Some(span) = find_duplicate_import_binding_span(element, imported) {
                    return Some(span);
                }
            }
            None
        }
        Some("ObjectPattern") => {
            let properties = estree_node_field_array(pattern, RawField::Properties)?;
            for property in properties {
                let EstreeValue::Object(property) = property else {
                    continue;
                };
                match estree_node_type(property) {
                    Some("Property") => {
                        let Some(value) = estree_node_field_object(property, RawField::Value)
                        else {
                            continue;
                        };
                        if let Some(span) = find_duplicate_import_binding_span(value, imported) {
                            return Some(span);
                        }
                    }
                    Some("RestElement") => {
                        let Some(argument) = estree_node_field_object(property, RawField::Argument)
                        else {
                            continue;
                        };
                        if let Some(span) = find_duplicate_import_binding_span(argument, imported) {
                            return Some(span);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => None,
    }
}

fn estree_node_span(node: &EstreeNode) -> Option<(usize, usize)> {
    Some((
        estree_value_to_usize(estree_node_field(node, RawField::Start))?,
        estree_value_to_usize(estree_node_field(node, RawField::End))?,
    ))
}

fn estree_node_literal_string(node: &EstreeNode) -> Option<String> {
    if estree_node_type(node) != Some("Literal") {
        return None;
    }
    match estree_node_field(node, RawField::Value) {
        Some(EstreeValue::String(value)) => Some(value.to_string()),
        _ => None,
    }
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
