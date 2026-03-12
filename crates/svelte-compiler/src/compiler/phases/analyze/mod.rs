mod validation;
mod warnings;

use crate::api::{CompileOptions, infer_runes_mode};
use crate::compiler::phases::component::{ComponentAnalysis, ComponentContext};
use crate::compiler::phases::parse::ParsedComponent;
use crate::error::CompileError;
use crate::source::SourceText;

pub(crate) fn analyze_component<'a>(
    parsed: ParsedComponent,
    options: &'a CompileOptions,
) -> Result<ComponentAnalysis<'a>, CompileError> {
    validate_component(parsed.as_ref(), options, parsed.root())?;
    let runes = infer_runes_mode(options, parsed.root());
    Ok(ComponentAnalysis::from_context(
        ComponentContext::new(parsed, options),
        runes,
    ))
}

pub(crate) fn validate_component(
    source: &str,
    options: &CompileOptions,
    root: &crate::ast::modern::Root,
) -> Result<(), CompileError> {
    if let Some(error) = validation::validate_component_source(source, options, root) {
        return Err(error.with_filename(options.filename.as_deref()));
    }
    Ok(())
}

pub(crate) fn validate_module(source: SourceText<'_>) -> Result<(), CompileError> {
    if let Some(error) = validation::validate_module_source(source.text) {
        return Err(error.with_source_text(source));
    }
    Ok(())
}

pub(crate) fn collect_compile_warnings(
    source: SourceText<'_>,
    options: &CompileOptions,
    root: &crate::ast::modern::Root,
) -> Box<[crate::Warning]> {
    warnings::collect_compile_warnings(source, options, root).into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use crate::{
        CompileOptions, SourceId,
        ast::modern::Root,
        compiler::phases::component::{ComponentAnalysis, LoweredComponent},
        source::SourceText,
    };

    #[test]
    fn analyze_component_produces_typed_component_analysis() {
        let parsed = crate::compiler::phases::parse::parse_component_for_compile("<p>ok</p>")
            .expect("parse component");
        let options = CompileOptions::default();
        let analysis: ComponentAnalysis<'_> =
            super::analyze_component(parsed, &options).expect("analyze component");

        fn source<T: AsRef<str>>(value: &T) -> &str {
            value.as_ref()
        }

        fn as_options<T: AsRef<CompileOptions>>(value: &T) -> &CompileOptions {
            value.as_ref()
        }

        fn root<T: AsRef<Root>>(value: &T) -> &Root {
            value.as_ref()
        }

        assert_eq!(source(&analysis), "<p>ok</p>");
        assert!(std::ptr::eq(as_options(&analysis), &options));
        assert_eq!(root(&analysis).start, 0);
        assert!(!analysis.runes());

        let lowered: LoweredComponent<'_> = analysis.lower();
        assert_eq!(source(&lowered), "<p>ok</p>");
        assert_eq!(root(&lowered).start, 0);
    }

    fn analyze_error(source: &str) -> crate::error::CompileError {
        let parsed =
            crate::compiler::phases::parse::parse_component_for_compile(source).expect("parse");
        super::analyze_component(parsed, &CompileOptions::default()).expect_err("analyze error")
    }

    #[test]
    fn analyze_rejects_top_level_then_continuation() {
        let error = analyze_error("{:then theValue}");
        assert_eq!(error.code.as_ref(), "block_invalid_continuation_placement");
    }

    #[test]
    fn analyze_rejects_top_level_catch_continuation() {
        let error = analyze_error("{:catch theValue}");
        assert_eq!(error.code.as_ref(), "block_invalid_continuation_placement");
    }

    #[test]
    fn analyze_rejects_else_after_open_element() {
        let error = analyze_error("<li>\n{:else}");
        assert_eq!(error.code.as_ref(), "block_invalid_continuation_placement");
    }

    #[test]
    fn collect_compile_warnings_counts_else_if_test_as_export_usage() {
        let source = "<script>export let foo;</script>{#if ok}{:else if foo}<p>{foo}</p>{/if}";
        let parsed = crate::compiler::phases::parse::parse_component_for_compile(source)
            .expect("parse component");
        let options = CompileOptions::default();

        let warnings = super::collect_compile_warnings(
            SourceText::new(SourceId::new(0), source, None),
            &options,
            parsed.root(),
        );

        assert!(
            !warnings
                .iter()
                .any(|warning| warning.code.as_ref() == "export_let_unused"),
            "else-if test should count as using `foo`: {warnings:?}"
        );
    }
}
