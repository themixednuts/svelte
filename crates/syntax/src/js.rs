use std::sync::Arc;

use oxc_allocator::Allocator;

use oxc_ast::ast::{
    BindingPattern, Expression, FormalParameter, FormalParameterRest, FormalParameters, Program,
    Statement, VariableDeclaration,
};
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::{ParseOptions, Parser, ParserReturn};
use oxc_span::{GetSpan, SourceType};

use self_cell::self_cell;

struct ProgramOwner {
    source: Box<str>,
    allocator: Allocator,
    source_type: SourceType,
    options: ParseOptions,
}

self_cell! {
    struct ParsedProgramCell {
        owner: ProgramOwner,

        #[covariant]
        dependent: ParserReturn,
    }
}

struct ExpressionOwner {
    source: Box<str>,
    allocator: Allocator,
    source_type: SourceType,
}

struct ParsedExpressionData<'a> {
    expression: Expression<'a>,
}

self_cell! {
    struct ParsedExpressionCell {
        owner: ExpressionOwner,

        #[covariant]
        dependent: ParsedExpressionData,
    }
}

/// Reusable OXC-backed JavaScript/TypeScript program handle.
///
/// This owns the source text and arena allocator required by the parsed OXC
/// AST so downstream tools can inspect the AST without reparsing or converting
/// through ESTree JSON.
pub struct JsProgram {
    cell: ParsedProgramCell,
}

impl std::fmt::Debug for JsProgram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsProgram")
            .field("source", &self.source())
            .field("source_type", &self.source_type())
            .field("panicked", &self.panicked())
            .field("error_count", &self.errors().len())
            .finish()
    }
}

impl PartialEq for JsProgram {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source() && self.source_type() == other.source_type()
    }
}

impl Eq for JsProgram {}

impl JsProgram {
    /// Parse JavaScript or TypeScript source into a program AST.
    #[must_use]
    pub fn parse(source: impl Into<Box<str>>, source_type: SourceType) -> Self {
        Self::parse_with_options(source, source_type, ParseOptions::default())
    }

    /// Parse with explicit OXC parser options.
    #[must_use]
    pub fn parse_with_options(
        source: impl Into<Box<str>>,
        source_type: SourceType,
        options: ParseOptions,
    ) -> Self {
        let owner = ProgramOwner {
            source: source.into(),
            allocator: Allocator::default(),
            source_type,
            options,
        };
        let cell = ParsedProgramCell::new(owner, |owner| {
            Parser::new(&owner.allocator, owner.source.as_ref(), owner.source_type)
                .with_options(owner.options)
                .parse()
        });
        Self { cell }
    }

    /// Return the original source text.
    #[must_use]
    pub fn source(&self) -> &str {
        self.cell.borrow_owner().source.as_ref()
    }

    /// Return the OXC source type used for parsing.
    #[must_use]
    pub fn source_type(&self) -> SourceType {
        self.cell.borrow_owner().source_type
    }

    /// Return the parsed OXC program AST.
    #[must_use]
    pub fn program(&self) -> &Program<'_> {
        &self.cell.borrow_dependent().program
    }

    /// Return any parse errors (the program may still be partially valid).
    pub fn errors(&self) -> &[OxcDiagnostic] {
        &self.cell.borrow_dependent().errors
    }

    /// Return `true` if the parser panicked during parsing.
    #[must_use]
    pub fn panicked(&self) -> bool {
        self.cell.borrow_dependent().panicked
    }

    /// Return `true` if the source uses Flow type annotations.
    #[must_use]
    pub fn is_flow_language(&self) -> bool {
        self.cell.borrow_dependent().is_flow_language
    }

    /// Access the full parser return for consumers that need module records,
    /// irregular whitespaces, or other parser metadata.
    #[must_use]
    pub fn parser_return(&self) -> &ParserReturn<'_> {
        self.cell.borrow_dependent()
    }
}

/// Reusable OXC-backed JavaScript/TypeScript expression handle.
///
/// This owns the source text and arena allocator required by the parsed OXC
/// AST so downstream tools can inspect a template/script expression without
/// reparsing.
pub struct JsExpression {
    cell: ParsedExpressionCell,
}

impl std::fmt::Debug for JsExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsExpression")
            .field("source", &self.source())
            .field("source_type", &self.source_type())
            .finish()
    }
}

impl PartialEq for JsExpression {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source() && self.source_type() == other.source_type()
    }
}

impl Eq for JsExpression {}

impl JsExpression {
    /// Parse a single JavaScript or TypeScript expression.
    pub fn parse(
        source: impl Into<Box<str>>,
        source_type: SourceType,
    ) -> Result<Self, Box<[OxcDiagnostic]>> {
        let owner = ExpressionOwner {
            source: source.into(),
            allocator: Allocator::default(),
            source_type,
        };
        let cell = ParsedExpressionCell::try_new(owner, |owner| {
            Parser::new(&owner.allocator, owner.source.as_ref(), owner.source_type)
                .parse_expression()
                .map(|expression| ParsedExpressionData { expression })
                .map_err(|errors| errors.into_boxed_slice())
        })?;
        Ok(Self { cell })
    }

    /// Return the original source text.
    #[must_use]
    pub fn source(&self) -> &str {
        self.cell.borrow_owner().source.as_ref()
    }

    /// Return the OXC source type used for parsing.
    #[must_use]
    pub fn source_type(&self) -> SourceType {
        self.cell.borrow_owner().source_type
    }

    /// Return the parsed OXC expression AST node.
    #[must_use]
    pub fn expression(&self) -> &Expression<'_> {
        &self.cell.borrow_dependent().expression
    }
}

/// Reusable OXC-backed binding pattern handle.
///
/// Svelte stores certain binding and parameter positions in the same logical
/// expression slot as ordinary expressions. This handle keeps those nodes in
/// OXC form without routing through ESTree compatibility trees.
pub struct JsPattern {
    source: Box<str>,
    wrapper: Arc<JsExpression>,
}

impl std::fmt::Debug for JsPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsPattern")
            .field("source", &self.source())
            .finish()
    }
}

/// Reusable OXC-backed formal parameter list handle.
///
/// This preserves richer parameter information like rest/default/type metadata
/// while still exposing each parameter binding pattern without reparsing.
pub struct JsParameters {
    source: Box<str>,
    wrapper: Arc<JsExpression>,
}

impl std::fmt::Debug for JsParameters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsParameters")
            .field("source", &self.source())
            .field("parameter_count", &self.parameters().items.len())
            .field("has_rest", &self.parameters().rest.is_some())
            .finish()
    }
}

impl PartialEq for JsParameters {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source()
    }
}

impl Eq for JsParameters {}

impl JsParameters {
    pub fn parse(source: impl Into<Box<str>>) -> Result<Self, Box<[OxcDiagnostic]>> {
        let source = source.into();
        let wrapper_source = format!("({})=>{{}}", source);
        let wrapper = Arc::new(JsExpression::parse(
            wrapper_source,
            SourceType::ts().with_module(true),
        )?);
        let _ = Self::parameters_from_wrapper(&wrapper).ok_or_else(|| {
            vec![OxcDiagnostic::error(
                "failed to recover formal parameters from wrapper",
            )]
            .into_boxed_slice()
        })?;
        Ok(Self { source, wrapper })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        self.source.as_ref()
    }

    #[must_use]
    pub fn parameters(&self) -> &FormalParameters<'_> {
        Self::parameters_from_wrapper(&self.wrapper).expect("validated parsed parameters")
    }

    #[must_use]
    pub fn parameter(&self, index: usize) -> Option<&FormalParameter<'_>> {
        self.parameters().items.get(index)
    }

    #[must_use]
    pub fn rest_parameter(&self) -> Option<&FormalParameterRest<'_>> {
        self.parameters().rest.as_deref()
    }

    fn parameters_from_wrapper(wrapper: &JsExpression) -> Option<&FormalParameters<'_>> {
        match wrapper.expression() {
            Expression::ArrowFunctionExpression(function) => Some(&function.params),
            _ => None,
        }
    }
}

impl PartialEq for JsPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source() == other.source()
    }
}

impl Eq for JsPattern {}

impl JsPattern {
    pub fn parse(source: impl Into<Box<str>>) -> Result<Self, Box<[OxcDiagnostic]>> {
        let source = source.into();
        let wrapper_source = format!("({})=>{{}}", source);
        let wrapper = Arc::new(JsExpression::parse(
            wrapper_source,
            SourceType::ts().with_module(true),
        )?);

        let _ = Self::pattern_from_wrapper(&wrapper).ok_or_else(|| {
            vec![OxcDiagnostic::error(
                "failed to recover binding pattern from wrapper",
            )]
            .into_boxed_slice()
        })?;

        Ok(Self { source, wrapper })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        self.source.as_ref()
    }

    #[must_use]
    pub fn pattern(&self) -> &BindingPattern<'_> {
        Self::pattern_from_wrapper(&self.wrapper).expect("validated parsed pattern")
    }

    fn pattern_from_wrapper(wrapper: &JsExpression) -> Option<&BindingPattern<'_>> {
        match wrapper.expression() {
            Expression::ArrowFunctionExpression(function) => function
                .params
                .items
                .first()
                .map(|parameter| &parameter.pattern),
            _ => None,
        }
    }
}

impl JsProgram {
    #[must_use]
    pub fn statement(&self, index: usize) -> Option<&Statement<'_>> {
        self.program().body.get(index)
    }

    #[must_use]
    pub fn statement_source(&self, index: usize) -> Option<&str> {
        let statement = self.statement(index)?;
        let span = statement.span();
        self.source()
            .get(span.start as usize..span.end as usize)
    }

    #[must_use]
    pub fn variable_declaration(&self, index: usize) -> Option<&VariableDeclaration<'_>> {
        match self.statement(index)? {
            Statement::VariableDeclaration(declaration) => Some(declaration),
            Statement::ExportNamedDeclaration(declaration) => match declaration.declaration.as_ref() {
                Some(oxc_ast::ast::Declaration::VariableDeclaration(declaration)) => Some(declaration),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use oxc_ast::ast::{BindingPattern, Expression, Statement};
    use oxc_span::SourceType;

    use super::{JsExpression, JsPattern, JsProgram};

    #[test]
    fn parsed_js_program_exposes_reusable_oxc_program() {
        let parsed = JsProgram::parse("export const answer = 42;", SourceType::mjs());

        assert_eq!(parsed.source(), "export const answer = 42;");
        assert!(parsed.errors().is_empty());
        assert!(!parsed.panicked());
        assert!(matches!(
            parsed.program().body.first(),
            Some(Statement::ExportNamedDeclaration(_))
        ));
    }

    #[test]
    fn parsed_js_expression_exposes_reusable_oxc_expression() {
        let parsed = JsExpression::parse("count + 1", SourceType::ts().with_module(true))
            .expect("expression should parse");

        assert_eq!(parsed.source(), "count + 1");
        assert!(matches!(
            parsed.expression(),
            Expression::BinaryExpression(_)
        ));
    }

    #[test]
    fn parsed_js_expression_returns_oxc_errors_on_invalid_input() {
        let errors = JsExpression::parse("foo(", SourceType::ts().with_module(true))
            .err()
            .expect("expression should fail");

        assert!(!errors.is_empty());
    }

    #[test]
    fn parsed_js_pattern_exposes_reusable_oxc_pattern() {
        let parsed =
            JsPattern::parse("{ count, items: [item] }").expect("pattern should parse");

        assert!(matches!(parsed.pattern(), BindingPattern::ObjectPattern(_)));
    }
}
