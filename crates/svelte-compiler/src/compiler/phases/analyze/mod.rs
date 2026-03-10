mod validation;
mod warnings;

use crate::api::CompileOptions;
use crate::ast::modern::Root;
use crate::compiler::phases::parse::ParsedComponent;
use crate::error::CompileError;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Analysis<'a> {
    pub source: &'a str,
    pub options: &'a CompileOptions,
}

impl Analysis<'_> {
    pub(crate) fn source(&self) -> &str {
        self.source
    }

    pub(crate) fn options(&self) -> &CompileOptions {
        self.options
    }
}

impl AsRef<str> for Analysis<'_> {
    fn as_ref(&self) -> &str {
        self.source()
    }
}

impl AsRef<CompileOptions> for Analysis<'_> {
    fn as_ref(&self) -> &CompileOptions {
        self.options()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ComponentAnalysis<'a> {
    pub analysis: Analysis<'a>,
    pub root: &'a Root,
}

impl ComponentAnalysis<'_> {
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

impl AsRef<str> for ComponentAnalysis<'_> {
    fn as_ref(&self) -> &str {
        self.source()
    }
}

impl AsRef<CompileOptions> for ComponentAnalysis<'_> {
    fn as_ref(&self) -> &CompileOptions {
        self.options()
    }
}

impl AsRef<Root> for ComponentAnalysis<'_> {
    fn as_ref(&self) -> &Root {
        self.root()
    }
}

impl<'a> From<&ComponentAnalysis<'a>> for Analysis<'a> {
    fn from(value: &ComponentAnalysis<'a>) -> Self {
        value.analysis
    }
}

pub(crate) fn analyze_component<'a>(
    parsed: &'a ParsedComponent,
    options: &'a CompileOptions,
) -> Result<ComponentAnalysis<'a>, CompileError> {
    validate_component(parsed.as_ref(), options, parsed.root())?;
    Ok(ComponentAnalysis {
        analysis: Analysis {
            source: parsed.as_ref(),
            options,
        },
        root: parsed.root(),
    })
}

pub(crate) fn validate_component(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Result<(), CompileError> {
    if let Some(error) = validation::validate_component_source(source, options, root) {
        return Err(error);
    }
    Ok(())
}

pub(crate) fn validate_module(source: &str) -> Result<(), CompileError> {
    if let Some(error) = validation::validate_module_source(source) {
        return Err(error);
    }
    Ok(())
}

pub(crate) fn collect_compile_warnings(
    source: &str,
    options: &CompileOptions,
    root: &Root,
) -> Box<[crate::Warning]> {
    warnings::collect_compile_warnings(source, options, root).into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, ast::modern::Root};

    #[test]
    fn analyze_component_produces_typed_component_analysis() {
        let parsed = crate::compiler::phases::parse::parse_component_for_compile("<p>ok</p>")
            .expect("parse component");
        let options = CompileOptions::default();
        let analysis: super::ComponentAnalysis<'_> =
            super::analyze_component(&parsed, &options).expect("analyze component");

        fn source<'a, T: AsRef<str>>(value: &'a T) -> &'a str {
            value.as_ref()
        }

        fn as_options<'a, T: AsRef<CompileOptions>>(value: &'a T) -> &'a CompileOptions {
            value.as_ref()
        }

        fn root<T: AsRef<Root>>(value: &T) -> &Root {
            value.as_ref()
        }

        assert_eq!(source(&analysis), "<p>ok</p>");
        assert!(std::ptr::eq(as_options(&analysis), &options));
        assert!(std::ptr::eq(root(&analysis), parsed.root()));

        let common: super::Analysis<'_> = (&analysis).into();
        assert_eq!(source(&common), "<p>ok</p>");
    }

    fn analyze_error(source: &str) -> crate::error::CompileError {
        let parsed =
            crate::compiler::phases::parse::parse_component_for_compile(source).expect("parse");
        super::analyze_component(&parsed, &CompileOptions::default()).expect_err("analyze error")
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
}
