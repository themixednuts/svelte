use super::*;
use crate::ast::modern::{Root, Script};
use oxc_ast::ast::{
    BindingPattern, Expression as OxcExpression, ImportDeclarationSpecifier, Program, Statement,
};
use oxc_ast_visit::{Visit, walk};

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
        .any(|script| script_has_rune_calls(script, has_ambiguous_state_call))
}

fn script_has_rune_calls(script: &Script, has_ambiguous_state_call: bool) -> bool {
    struct RuneCallVisitor {
        has_ambiguous_state_call: bool,
        found: bool,
    }

    impl<'a> Visit<'a> for RuneCallVisitor {
        fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'a>) {
            if self.found {
                return;
            }

            if let Some(name) = callee_rune_name(&it.callee)
                && is_rune_mode_trigger(name.as_str())
            {
                if name == "$state" && self.has_ambiguous_state_call {
                    walk::walk_call_expression(self, it);
                    return;
                }
                self.found = true;
                return;
            }

            walk::walk_call_expression(self, it);
        }
    }

    let mut visitor = RuneCallVisitor {
        has_ambiguous_state_call,
        found: false,
    };
    visitor.visit_program(script.oxc_program());
    visitor.found
}

fn callee_rune_name(callee: &OxcExpression<'_>) -> Option<String> {
    match callee.get_inner_expression() {
        OxcExpression::Identifier(reference) => Some(reference.name.to_string()),
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
        if script_has_top_level_state_expression(script.oxc_program())
            && script_has_top_level_binding_named(script.oxc_program(), "state")
        {
            return true;
        }
    }
    false
}

fn script_has_top_level_state_expression(program: &Program<'_>) -> bool {
    program.body.iter().any(|statement| {
        let Statement::ExpressionStatement(statement) = statement else {
            return false;
        };
        let OxcExpression::CallExpression(call) = statement.expression.get_inner_expression()
        else {
            return false;
        };
        matches!(
            call.callee.get_inner_expression(),
            OxcExpression::Identifier(identifier) if identifier.name == "$state"
        )
    })
}

fn script_has_top_level_binding_named(program: &Program<'_>, expected: &str) -> bool {
    program
        .body
        .iter()
        .any(|statement| statement_binds_name(statement, expected))
}

fn statement_binds_name(statement: &Statement<'_>, expected: &str) -> bool {
    match statement {
        Statement::VariableDeclaration(declaration) => declaration
            .declarations
            .iter()
            .any(|declarator| binding_pattern_identifier_name(&declarator.id) == Some(expected)),
        Statement::FunctionDeclaration(declaration) => declaration
            .id
            .as_ref()
            .is_some_and(|id| id.name == expected),
        Statement::ClassDeclaration(declaration) => declaration
            .id
            .as_ref()
            .is_some_and(|id| id.name == expected),
        Statement::ImportDeclaration(declaration) => {
            declaration.specifiers.as_ref().is_some_and(|specifiers| {
                specifiers
                    .iter()
                    .any(|specifier| import_specifier_local_name(specifier) == Some(expected))
            })
        }
        _ => false,
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

fn binding_pattern_identifier_name<'a>(pattern: &'a BindingPattern<'a>) -> Option<&'a str> {
    match pattern.get_binding_identifier() {
        Some(identifier) => Some(identifier.name.as_str()),
        None => None,
    }
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
