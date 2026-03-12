use crate::ast::modern::{EachBlock, EstreeNode, SnippetBlock};
use crate::estree::collect_pattern_binding_names;
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

pub(crate) fn pattern_binds_name(pattern: &EstreeNode, name: &str) -> bool {
    let mut bindings = NameSet::default();
    collect_pattern_binding_names(pattern, &mut bindings);
    bindings.contains(name)
}

pub(crate) fn extend_name_set_with_pattern_bindings(names: &mut NameSet, pattern: &EstreeNode) {
    collect_pattern_binding_names(pattern, names);
}

pub(crate) fn extend_name_set_with_optional_name(names: &mut NameSet, name: Option<&Name>) {
    if let Some(name) = name {
        names.insert(name.clone());
    }
}

pub(crate) fn scope_frame_for_each_block(block: &EachBlock) -> NameSet {
    let mut names = NameSet::default();
    if let Some(context) = block.context.as_ref() {
        extend_name_set_with_pattern_bindings(&mut names, &context.0);
    }
    extend_name_set_with_optional_name(&mut names, block.index.as_ref());
    names
}

pub(crate) fn scope_frame_for_snippet_block(block: &SnippetBlock) -> NameSet {
    let mut names = NameSet::default();
    for parameter in &block.parameters {
        extend_name_set_with_pattern_bindings(&mut names, &parameter.0);
    }
    names
}
