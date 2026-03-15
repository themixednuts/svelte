use crate::ast::modern::{EachBlock, Expression, SnippetBlock};
use crate::names::{Name, NameSet};

#[derive(Debug, Default)]
pub(crate) struct ScopeStack {
    frames: Vec<NameSet>,
}

impl ScopeStack {
    pub(crate) fn push(&mut self, frame: NameSet) {
        self.frames.push(frame);
    }

    pub(crate) fn pop(&mut self) {
        let _ = self.frames.pop();
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.frames.iter().rev().any(|frame| frame.contains(name))
    }

    pub(crate) fn with_frame<T>(
        &mut self,
        frame: NameSet,
        visit: impl FnOnce(&mut Self) -> T,
    ) -> T {
        self.push(frame);
        let result = visit(self);
        self.pop();
        result
    }
}

pub(crate) fn extend_name_set_with_oxc_pattern_bindings(
    names: &mut NameSet,
    pattern: &oxc_ast::ast::BindingPattern<'_>,
) {
    collect_oxc_pattern_binding_names(pattern, names);
}

pub(crate) fn extend_name_set_with_expression_pattern_bindings(
    names: &mut NameSet,
    expression: &Expression,
) {
    if let Some(pattern) = expression.oxc_pattern() {
        extend_name_set_with_oxc_pattern_bindings(names, pattern);
    }
}

pub(crate) fn extend_name_set_with_optional_name(names: &mut NameSet, name: Option<&Name>) {
    if let Some(name) = name {
        names.insert(name.clone());
    }
}

pub(crate) fn scope_frame_for_each_block(block: &EachBlock) -> NameSet {
    let mut names = NameSet::default();
    if let Some(context) = block.context.as_ref() {
        extend_name_set_with_expression_pattern_bindings(&mut names, context);
    }
    extend_name_set_with_optional_name(&mut names, block.index.as_ref());
    names
}

pub(crate) fn scope_frame_for_snippet_block(block: &SnippetBlock) -> NameSet {
    let mut names = NameSet::default();
    for parameter in &block.parameters {
        extend_name_set_with_expression_pattern_bindings(&mut names, parameter);
    }
    names
}

fn collect_oxc_pattern_binding_names(pattern: &oxc_ast::ast::BindingPattern<'_>, names: &mut NameSet) {
    match pattern {
        oxc_ast::ast::BindingPattern::BindingIdentifier(identifier) => {
            names.insert(identifier.name.as_str().into());
        }
        oxc_ast::ast::BindingPattern::AssignmentPattern(pattern) => {
            collect_oxc_pattern_binding_names(&pattern.left, names);
        }
        oxc_ast::ast::BindingPattern::ObjectPattern(pattern) => {
            for property in &pattern.properties {
                collect_oxc_pattern_binding_names(&property.value, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_oxc_pattern_binding_names(&rest.argument, names);
            }
        }
        oxc_ast::ast::BindingPattern::ArrayPattern(pattern) => {
            for element in pattern.elements.iter().flatten() {
                collect_oxc_pattern_binding_names(element, names);
            }
            if let Some(rest) = pattern.rest.as_ref() {
                collect_oxc_pattern_binding_names(&rest.argument, names);
            }
        }
    }
}
