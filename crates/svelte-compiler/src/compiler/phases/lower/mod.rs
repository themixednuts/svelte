use crate::api::CompileOptions;
use crate::ast::modern::Root;
use crate::compiler::phases::analyze::{Analysis, ComponentAnalysis};

#[derive(Debug, Clone, Copy)]
pub(crate) struct TransformState<'a> {
    pub analysis: Analysis<'a>,
    pub root: &'a Root,
}

impl TransformState<'_> {
    pub(crate) fn source(&self) -> &str {
        self.analysis.source()
    }

    pub(crate) fn options(&self) -> &CompileOptions {
        self.analysis.options()
    }

    pub(crate) fn root(&self) -> &Root {
        self.root
    }
}

impl AsRef<str> for TransformState<'_> {
    fn as_ref(&self) -> &str {
        self.source()
    }
}

impl AsRef<CompileOptions> for TransformState<'_> {
    fn as_ref(&self) -> &CompileOptions {
        self.options()
    }
}

impl AsRef<Root> for TransformState<'_> {
    fn as_ref(&self) -> &Root {
        self.root()
    }
}

impl<'a> From<&'a ComponentAnalysis<'a>> for TransformState<'a> {
    fn from(value: &'a ComponentAnalysis<'a>) -> Self {
        Self {
            analysis: value.into(),
            root: value.root(),
        }
    }
}

pub(crate) fn lower_component<'a>(analysis: &'a ComponentAnalysis<'a>) -> TransformState<'a> {
    analysis.into()
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, ast::modern::Root};

    #[test]
    fn lower_component_yields_typed_transform_state() {
        let parsed = crate::compiler::phases::parse::parse_component_for_compile("<div />")
            .expect("parse component");
        let options = CompileOptions::default();
        let analysis = crate::compiler::phases::analyze::analyze_component(&parsed, &options)
            .expect("analyze component");

        let lowered: super::TransformState<'_> = super::lower_component(&analysis);
        let via_from: super::TransformState<'_> = (&analysis).into();

        fn source<'a, T: AsRef<str>>(value: &'a T) -> &'a str {
            value.as_ref()
        }

        fn root<T: AsRef<Root>>(value: &T) -> &Root {
            value.as_ref()
        }

        assert_eq!(source(&lowered), "<div />");
        assert!(std::ptr::eq(root(&lowered), analysis.root()));

        assert_eq!(source(&via_from), "<div />");
        assert!(std::ptr::eq(root(&via_from), analysis.root()));
    }
}
