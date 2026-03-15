use crate::compiler::phases::component::{ComponentAnalysis, LoweredComponent};

pub(crate) fn lower_component(analysis: ComponentAnalysis<'_>) -> LoweredComponent<'_> {
    analysis.lower()
}

#[cfg(test)]
mod tests {
    use crate::{
        CompileOptions,
        ast::modern::Root,
        compiler::phases::component::{ComponentAnalysis, LoweredComponent},
    };

    #[test]
    fn lower_component_yields_typed_transform_state() {
        let parsed = crate::compiler::phases::parse::parse_component_for_compile("<div />")
            .expect("parse component");
        let options = CompileOptions::default();
        let analysis: ComponentAnalysis<'_> =
            crate::compiler::phases::analyze::analyze_component(parsed, &options)
                .expect("analyze component");

        let lowered: LoweredComponent<'_> = super::lower_component(analysis);

        fn source<T: AsRef<str>>(value: &T) -> &str {
            value.as_ref()
        }

        fn root<T: AsRef<Root>>(value: &T) -> &Root {
            value.as_ref()
        }

        assert_eq!(source(&lowered), "<div />");
        assert_eq!(root(&lowered).start, 0);
    }
}
