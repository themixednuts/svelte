use super::*;
use crate::api::modern::{
    RawField, estree_node_field_array, estree_node_field_object, estree_node_field_str,
    estree_node_type,
};
use crate::ast::modern::{EstreeNode, EstreeValue, Root};

pub(crate) fn infer_runes_mode(options: &CompileOptions, root: &Root) -> bool {
    is_runes_mode(options, root)
}

pub(crate) fn is_runes_mode(options: &CompileOptions, root: &Root) -> bool {
    options.runes.unwrap_or_else(|| {
        root.options
            .as_ref()
            .and_then(|options| options.runes)
            .unwrap_or_else(|| {
                scripts_have_rune_calls(root, has_ambiguous_top_level_state_call(root))
            })
    })
}

fn scripts_have_rune_calls(root: &Root, has_ambiguous_state_call: bool) -> bool {
    [root.module.as_ref(), root.instance.as_ref()]
        .into_iter()
        .flatten()
        .any(|script| script_has_rune_calls(&script.content, has_ambiguous_state_call))
}

fn script_has_rune_calls(program: &EstreeNode, has_ambiguous_state_call: bool) -> bool {
    let mut stack = vec![program];
    while let Some(node) = stack.pop() {
        if estree_node_type(node) == Some("CallExpression")
            && let Some(callee) = estree_node_field_object(node, RawField::Callee)
            && let Some(name) = callee_rune_name(callee)
            && is_rune_mode_trigger(name.as_str())
        {
            if name == "$state" && has_ambiguous_state_call {
                continue;
            }
            return true;
        }

        for value in node.fields.values() {
            push_estree_value_nodes(value, &mut stack);
        }
    }
    false
}

fn push_estree_value_nodes<'a>(value: &'a EstreeValue, stack: &mut Vec<&'a EstreeNode>) {
    match value {
        EstreeValue::Object(node) => stack.push(node),
        EstreeValue::Array(values) => {
            for value in values {
                push_estree_value_nodes(value, stack);
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

fn callee_rune_name(callee: &EstreeNode) -> Option<String> {
    if estree_node_type(callee) == Some("Identifier") {
        return estree_node_field_str(callee, RawField::Name).map(ToOwned::to_owned);
    }

    if estree_node_type(callee) != Some("MemberExpression") {
        return None;
    }

    if estree_node_field_bool(callee, RawField::Computed).unwrap_or(false) {
        return None;
    }

    let object = estree_node_field_object(callee, RawField::Object)?;
    let property = estree_node_field_object(callee, RawField::Property)?;
    let object_name = estree_node_field_str(object, RawField::Name)?;
    let property_name = estree_node_field_str(property, RawField::Name)?;
    Some(format!("{object_name}.{property_name}"))
}

fn estree_node_field_bool(node: &EstreeNode, key: RawField) -> Option<bool> {
    match super::modern::estree_node_field(node, key) {
        Some(EstreeValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn is_rune_mode_trigger(name: &str) -> bool {
    matches!(
        name,
        "$state"
            | "$state.raw"
            | "$state.snapshot"
            | "$derived"
            | "$derived.by"
            | "$effect"
            | "$effect.active"
            | "$effect.pre"
            | "$effect.tracking"
            | "$effect.root"
            | "$bindable"
            | "$props"
            | "$props.id"
            | "$inspect"
            | "$inspect.trace"
            | "$host"
    )
}

fn has_ambiguous_top_level_state_call(root: &Root) -> bool {
    for script in [root.module.as_ref(), root.instance.as_ref()] {
        let Some(script) = script else {
            continue;
        };
        if script_has_top_level_state_expression(&script.content)
            && script_has_top_level_binding_named(&script.content, "state")
        {
            return true;
        }
    }
    false
}

fn script_has_top_level_state_expression(program: &EstreeNode) -> bool {
    let Some(body) = estree_node_field_array(program, RawField::Body) else {
        return false;
    };

    body.iter().any(|statement| {
        let EstreeValue::Object(statement) = statement else {
            return false;
        };
        if estree_node_type(statement) != Some("ExpressionStatement") {
            return false;
        }
        let Some(expression) = estree_node_field_object(statement, RawField::Expression) else {
            return false;
        };
        if estree_node_type(expression) != Some("CallExpression") {
            return false;
        }
        let Some(callee) = estree_node_field_object(expression, RawField::Callee) else {
            return false;
        };
        estree_node_type(callee) == Some("Identifier")
            && estree_node_field_str(callee, RawField::Name) == Some("$state")
    })
}

fn script_has_top_level_binding_named(program: &EstreeNode, expected: &str) -> bool {
    let Some(body) = estree_node_field_array(program, RawField::Body) else {
        return false;
    };

    body.iter().any(|statement| {
        let EstreeValue::Object(statement) = statement else {
            return false;
        };
        match estree_node_type(statement) {
            Some("VariableDeclaration") => {
                estree_node_field_array(statement, RawField::Declarations).is_some_and(
                    |declarations| {
                        declarations.iter().any(|declaration| {
                            let EstreeValue::Object(declaration) = declaration else {
                                return false;
                            };
                            estree_node_field_object(declaration, RawField::Id).is_some_and(|id| {
                                estree_node_type(id) == Some("Identifier")
                                    && estree_node_field_str(id, RawField::Name) == Some(expected)
                            })
                        })
                    },
                )
            }
            Some("FunctionDeclaration" | "ClassDeclaration") => {
                estree_node_field_object(statement, RawField::Id).is_some_and(|id| {
                    estree_node_type(id) == Some("Identifier")
                        && estree_node_field_str(id, RawField::Name) == Some(expected)
                })
            }
            Some("ImportDeclaration") => estree_node_field_array(statement, RawField::Specifiers)
                .is_some_and(|specifiers| {
                    specifiers.iter().any(|specifier| {
                        let EstreeValue::Object(specifier) = specifier else {
                            return false;
                        };
                        estree_node_field_object(specifier, RawField::Local).is_some_and(|local| {
                            estree_node_type(local) == Some("Identifier")
                                && estree_node_field_str(local, RawField::Name) == Some(expected)
                        })
                    })
                }),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::infer_runes_mode;
    use crate::api::CompileOptions;
    use crate::compiler::phases::parse::parse_component_for_compile;

    fn runes_mode(source: &str) -> bool {
        let parsed = parse_component_for_compile(source).expect("parse component");
        infer_runes_mode(&CompileOptions::default(), parsed.root())
    }

    #[test]
    fn runes_mode_uses_svelte_options_true_from_ast() {
        assert!(runes_mode("<svelte:options runes />"));
    }

    #[test]
    fn runes_mode_uses_svelte_options_false_from_ast() {
        assert!(!runes_mode("<svelte:options runes={false} />"));
    }

    #[test]
    fn runes_mode_uses_rune_calls_when_options_are_absent() {
        assert!(runes_mode("<script>let count = $state(0);</script>"));
    }

    #[test]
    fn runes_mode_ignores_ambiguous_top_level_state_calls() {
        assert!(!runes_mode("<script>let state = 1; $state(0);</script>"));
    }
}
