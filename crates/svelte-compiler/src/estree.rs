use std::sync::Arc;

use crate::api::modern::{
    RawField, estree_node_field_array, estree_node_field_object, estree_node_field_str,
    estree_node_type,
};
use crate::ast::modern::{EstreeNode, EstreeValue};

pub(crate) fn raw_identifier_name(node: &EstreeNode) -> Option<Arc<str>> {
    (estree_node_type(node) == Some("Identifier"))
        .then(|| estree_node_field_str(node, RawField::Name))
        .flatten()
        .map(Arc::from)
}

pub(crate) fn raw_callee_name(node: &EstreeNode) -> Option<Arc<str>> {
    match estree_node_type(node) {
        Some("Identifier") => estree_node_field_str(node, RawField::Name).map(Arc::from),
        Some("MemberExpression") => {
            let object = estree_node_field_object(node, RawField::Object)?;
            let property = estree_node_field_object(node, RawField::Property)?;
            let object_name = raw_callee_name(object)?;
            let property_name = raw_identifier_name(property)?;
            Some(format!("{object_name}.{property_name}").into())
        }
        _ => None,
    }
}

pub(crate) fn raw_base_identifier_name(node: &EstreeNode) -> Option<Arc<str>> {
    match estree_node_type(node) {
        Some("Identifier") => estree_node_field_str(node, RawField::Name).map(Arc::from),
        Some("MemberExpression") => {
            let object = estree_node_field_object(node, RawField::Object)?;
            raw_base_identifier_name(object)
        }
        _ => None,
    }
}

pub(crate) fn raw_member_property_name(node: &EstreeNode) -> Option<Arc<str>> {
    if estree_node_type(node) != Some("MemberExpression") {
        return None;
    }
    let property = estree_node_field_object(node, RawField::Property)?;
    raw_identifier_name(property)
}

pub(crate) fn raw_literal_string(node: &EstreeNode) -> Option<Arc<str>> {
    if estree_node_type(node) != Some("Literal") {
        return None;
    }
    match node.fields.get("value")? {
        EstreeValue::String(value) => Some(value.to_string().into()),
        _ => None,
    }
}

pub(crate) fn export_specifier_exported_name(specifier: &EstreeNode) -> Option<Arc<str>> {
    let value = specifier.fields.get("exported")?;
    let EstreeValue::Object(exported) = value else {
        return None;
    };

    match estree_node_type(exported) {
        Some("Identifier") => estree_node_field_str(exported, RawField::Name).map(Arc::from),
        Some("Literal") => raw_literal_string(exported),
        _ => None,
    }
}

pub(crate) fn unwrap_typescript_expression(mut node: &EstreeNode) -> &EstreeNode {
    loop {
        match estree_node_type(node) {
            Some(
                "ParenthesizedExpression"
                | "TSAsExpression"
                | "TSSatisfiesExpression"
                | "TSNonNullExpression"
                | "TSTypeAssertion",
            ) => {
                let Some(expression) = estree_node_field_object(node, RawField::Expression) else {
                    return node;
                };
                node = expression;
            }
            _ => return node,
        }
    }
}

pub(crate) fn is_identifier_or_member_expression(node: &EstreeNode) -> bool {
    matches!(
        estree_node_type(unwrap_typescript_expression(node)),
        Some("Identifier" | "MemberExpression")
    )
}

pub(crate) fn collect_assignment_target_identifiers(
    target: &EstreeNode,
    out: &mut impl Extend<Arc<str>>,
) {
    match estree_node_type(target) {
        Some("Identifier") => {
            if let Some(name) = raw_identifier_name(target) {
                out.extend([name]);
            }
        }
        Some("AssignmentPattern") => {
            if let Some(left) = estree_node_field_object(target, RawField::Left) {
                collect_assignment_target_identifiers(left, out);
            }
        }
        Some("RestElement") => {
            if let Some(argument) = estree_node_field_object(target, RawField::Argument) {
                collect_assignment_target_identifiers(argument, out);
            }
        }
        Some("ArrayPattern") => {
            if let Some(elements) = estree_node_field_array(target, RawField::Elements) {
                for element in elements {
                    let EstreeValue::Object(element) = element else {
                        continue;
                    };
                    collect_assignment_target_identifiers(element, out);
                }
            }
        }
        Some("ObjectPattern") => {
            if let Some(properties) = estree_node_field_array(target, RawField::Properties) {
                for property in properties {
                    let EstreeValue::Object(property) = property else {
                        continue;
                    };
                    match estree_node_type(property) {
                        Some("Property") => {
                            if let Some(value) = estree_node_field_object(property, RawField::Value)
                            {
                                collect_assignment_target_identifiers(value, out);
                            }
                        }
                        Some("RestElement") => {
                            if let Some(argument) =
                                estree_node_field_object(property, RawField::Argument)
                            {
                                collect_assignment_target_identifiers(argument, out);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn collect_pattern_binding_names<E>(pattern: &EstreeNode, out: &mut E)
where
    E: Extend<Arc<str>>,
{
    match estree_node_type(pattern) {
        Some("Identifier") => {
            if let Some(name) = estree_node_field_str(pattern, RawField::Name) {
                out.extend([Arc::from(name)]);
            }
        }
        Some("RestElement") => {
            if let Some(argument) = estree_node_field_object(pattern, RawField::Argument) {
                collect_pattern_binding_names(argument, out);
            }
        }
        Some("AssignmentPattern") => {
            if let Some(left) = estree_node_field_object(pattern, RawField::Left) {
                collect_pattern_binding_names(left, out);
            }
        }
        Some("ArrayPattern") => {
            if let Some(elements) = estree_node_field_array(pattern, RawField::Elements) {
                for element in elements {
                    let EstreeValue::Object(element) = element else {
                        continue;
                    };
                    collect_pattern_binding_names(element, out);
                }
            }
        }
        Some("ObjectPattern") => {
            if let Some(properties) = estree_node_field_array(pattern, RawField::Properties) {
                for property in properties {
                    let EstreeValue::Object(property) = property else {
                        continue;
                    };
                    match estree_node_type(property) {
                        Some("Property") => {
                            if let Some(value) = estree_node_field_object(property, RawField::Value)
                            {
                                collect_pattern_binding_names(value, out);
                            }
                        }
                        Some("RestElement") => {
                            if let Some(argument) =
                                estree_node_field_object(property, RawField::Argument)
                            {
                                collect_pattern_binding_names(argument, out);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn class_key_name(node: &EstreeNode) -> Option<Arc<str>> {
    match estree_node_type(node) {
        Some("Identifier") => estree_node_field_str(node, RawField::Name).map(Arc::from),
        Some("PrivateIdentifier") => {
            estree_node_field_str(node, RawField::Name).map(|name| Arc::from(format!("#{name}")))
        }
        Some("Literal") => match node.fields.get("value") {
            Some(EstreeValue::String(value)) => Some(value.clone()),
            Some(EstreeValue::Int(value)) => Some(Arc::from(value.to_string())),
            Some(EstreeValue::UInt(value)) => Some(Arc::from(value.to_string())),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn this_member_name(node: &EstreeNode) -> Option<Arc<str>> {
    if estree_node_type(node) != Some("MemberExpression") {
        return None;
    }
    let object = estree_node_field_object(node, RawField::Object)?;
    if estree_node_type(object) != Some("ThisExpression") {
        return None;
    }
    let property = estree_node_field_object(node, RawField::Property)?;
    if estree_node_field_bool_named(node, "computed").unwrap_or(false)
        && estree_node_type(property) != Some("Literal")
    {
        return None;
    }
    class_key_name(property)
}

#[derive(Clone, Copy)]
pub(crate) struct PathStep<'a> {
    pub parent: &'a EstreeNode,
    pub via_key: &'a str,
}

pub(crate) fn walk_estree_node_with_path<'a>(
    node: &'a EstreeNode,
    path: &mut Vec<PathStep<'a>>,
    visitor: &mut impl FnMut(&'a EstreeNode, &[PathStep<'a>]),
) {
    visitor(node, path);
    for (key, value) in &node.fields {
        walk_estree_value_with_path(value, node, key.as_str(), path, visitor);
    }
}

pub(crate) fn walk_reference_identifiers_with_path<'a>(
    node: &'a EstreeNode,
    path: &mut Vec<PathStep<'a>>,
    visitor: &mut impl FnMut(&'a EstreeNode, &'a str, &[PathStep<'a>]),
) {
    walk_estree_node_with_path(node, path, &mut |current, current_path| {
        if estree_node_type(current) != Some("Identifier")
            || is_ignored_identifier_context(current_path)
            || is_type_identifier_context(current_path)
        {
            return;
        }
        let Some(name) = estree_node_field_str(current, RawField::Name) else {
            return;
        };
        visitor(current, name, current_path);
    });
}

pub(crate) fn path_has_function_scope(path: &[PathStep<'_>]) -> bool {
    path.iter().any(|step| {
        matches!(
            estree_node_type(step.parent),
            Some("FunctionDeclaration" | "FunctionExpression" | "ArrowFunctionExpression")
        )
    })
}

pub(crate) fn is_ignored_identifier_context(path: &[PathStep<'_>]) -> bool {
    let Some(step) = path.last() else {
        return false;
    };
    let parent_type = estree_node_type(step.parent);
    if matches!(
        parent_type,
        Some(
            "VariableDeclarator"
                | "FunctionDeclaration"
                | "FunctionExpression"
                | "ArrowFunctionExpression"
                | "ClassDeclaration"
                | "ImportSpecifier"
                | "ImportDefaultSpecifier"
                | "ImportNamespaceSpecifier"
                | "CatchClause"
                | "LabeledStatement"
                | "ExportSpecifier"
        )
    ) && matches!(
        step.via_key,
        "id" | "params" | "local" | "exported" | "param" | "label"
    ) {
        return true;
    }
    if parent_type == Some("MemberExpression") && step.via_key == "property" {
        return true;
    }
    if parent_type == Some("Property") && step.via_key == "key" {
        return true;
    }
    false
}

pub(crate) fn is_type_identifier_context(path: &[PathStep<'_>]) -> bool {
    path.iter().any(|step| {
        estree_node_type(step.parent)
            .is_some_and(|kind| kind.starts_with("TS") || kind == "TSTypeAnnotation")
    })
}

fn walk_estree_value_with_path<'a>(
    value: &'a EstreeValue,
    parent: &'a EstreeNode,
    via_key: &'a str,
    path: &mut Vec<PathStep<'a>>,
    visitor: &mut impl FnMut(&'a EstreeNode, &[PathStep<'a>]),
) {
    match value {
        EstreeValue::Object(node) => {
            path.push(PathStep { parent, via_key });
            walk_estree_node_with_path(node, path, visitor);
            path.pop();
        }
        EstreeValue::Array(values) => {
            for item in values {
                walk_estree_value_with_path(item, parent, via_key, path, visitor);
            }
        }
        EstreeValue::String(_)
        | EstreeValue::Int(_)
        | EstreeValue::UInt(_)
        | EstreeValue::Number(_)
        | EstreeValue::Bool(_)
        | EstreeValue::Null => {}
    }
}

fn estree_node_field_bool_named(node: &EstreeNode, key: &str) -> Option<bool> {
    match node.fields.get(key) {
        Some(EstreeValue::Bool(value)) => Some(*value),
        _ => None,
    }
}
