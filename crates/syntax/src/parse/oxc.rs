use std::sync::Arc;

use oxc_allocator::Allocator;
use oxc_parser::Parser as OxcParser;
use oxc_span::SourceType as OxcSourceType;

use crate::ast::modern::Expression;
use crate::js::{ParsedJsExpression, ParsedJsProgram};
use crate::parse::ParsedProgramContent;

pub(crate) struct OxcProgramOffsets {
    pub global_start: usize,
}

impl OxcProgramOffsets {
    pub(crate) fn for_root_source(_source_len: usize) -> Self {
        Self { global_start: 0 }
    }
}

pub(crate) struct SvelteOxcParser<'src> {
    source: &'src str,
    offsets: OxcProgramOffsets,
}

impl<'src> SvelteOxcParser<'src> {
    pub(crate) fn new(source: &'src str) -> Self {
        Self {
            source,
            offsets: OxcProgramOffsets::for_root_source(source.len()),
        }
    }

    pub(crate) fn with_offsets(mut self, offsets: OxcProgramOffsets) -> Self {
        self.offsets = offsets;
        self
    }

    pub(crate) fn parse_program_for_compile(&self, is_ts: bool) -> Option<ParsedProgramContent> {
        let source_type = if is_ts {
            OxcSourceType::ts().with_module(true)
        } else {
            OxcSourceType::mjs()
        };
        let parsed = Arc::new(ParsedJsProgram::parse(self.source, source_type));
        Some(ParsedProgramContent { parsed })
    }

    pub(crate) fn parse_expression_for_template(&self) -> Option<Expression> {
        let source_type = OxcSourceType::ts().with_module(true);
        let parsed = Arc::new(ParsedJsExpression::parse(self.source, source_type).ok()?);
        Some(Expression::from_expression(
            parsed,
            self.offsets.global_start,
            self.offsets.global_start + self.source.len(),
        ))
    }

    pub(crate) fn parse_expression_error_for_template(&self) -> Option<Arc<str>> {
        self.parse_expression_error_detail_for_template()
            .map(|(_, message)| message)
    }

    pub(crate) fn parse_expression_error_detail_for_template(&self) -> Option<(usize, Arc<str>)> {
        let allocator = Allocator::default();
        let source_type = OxcSourceType::ts().with_module(true);
        let errors = OxcParser::new(&allocator, self.source, source_type)
            .parse_expression()
            .err()?;
        let error = errors
            .iter()
            .min_by_key(|error| match error.to_string().as_str() {
                message if message.starts_with("Unexpected keyword ") => 0,
                "Unexpected token" => 1,
                _ => 2,
            })?;
        let start = error
            .labels
            .as_ref()
            .and_then(|labels| labels.first())
            .map(|label| self.offsets.global_start + label.inner().offset())
            .unwrap_or(self.offsets.global_start);

        Some((start, normalize_expression_error_message(error.to_string())))
    }
}

fn normalize_expression_error_message(message: String) -> Arc<str> {
    match message.as_str() {
        "Cannot assign to this expression" => Arc::from("Assigning to rvalue"),
        _ => Arc::from(message),
    }
}
