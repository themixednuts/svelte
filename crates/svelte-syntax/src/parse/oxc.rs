use std::collections::BTreeMap;
use std::sync::Arc;

use oxc_allocator::Allocator;
use oxc_estree::{CompactTSSerializer, ESTree};
use oxc_parser::Parser as OxcParser;
use oxc_span::SourceType as OxcSourceType;

use crate::ast::modern::{EstreeNode, EstreeValue, Expression};
use crate::parse::component::modern::{
    RawField, attach_estree_comments_to_tree, estree_node_field, normalize_estree_node,
    parse_all_comment_nodes, parse_leading_comment_nodes, position_raw_node,
};

pub(crate) struct OxcProgramOffsets {
    pub global_start: usize,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

impl OxcProgramOffsets {
    pub(crate) fn for_root_source(source_len: usize) -> Self {
        Self {
            global_start: 0,
            start_line: 1,
            start_column: 0,
            end_line: 1,
            end_column: source_len,
        }
    }
}

pub(crate) struct SvelteOxcParser<'src> {
    source: &'src str,
    offsets: OxcProgramOffsets,
}

struct ParsedRawNode {
    node: EstreeNode,
}

fn program_body_is_empty(program: &EstreeNode) -> bool {
    match estree_node_field(program, RawField::Body) {
        Some(EstreeValue::Array(items)) => items.is_empty(),
        _ => false,
    }
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

    pub(crate) fn parse_program_for_compile(&self, is_ts: bool) -> Option<EstreeNode> {
        let allocator = Allocator::default();
        let source_type = if is_ts {
            OxcSourceType::ts().with_module(true)
        } else {
            OxcSourceType::mjs()
        };
        let parsed = OxcParser::new(&allocator, self.source, source_type).parse();

        let json = if is_ts {
            parsed.program.to_estree_ts_json(true)
        } else {
            parsed.program.to_estree_js_json(true)
        };
        let mut parsed_program = self.parse_and_normalize_raw_node(&json)?;
        let program = &mut parsed_program.node;

        program.fields.insert(
            "start".to_string(),
            EstreeValue::UInt(self.offsets.global_start as u64),
        );
        program.fields.insert(
            "end".to_string(),
            EstreeValue::UInt((self.offsets.global_start + self.source.len()) as u64),
        );

        let mut loc_fields = BTreeMap::new();
        loc_fields.insert(
            "start".to_string(),
            EstreeValue::Object(position_raw_node(
                self.offsets.start_line,
                self.offsets.start_column,
            )),
        );
        loc_fields.insert(
            "end".to_string(),
            EstreeValue::Object(position_raw_node(
                self.offsets.end_line,
                self.offsets.end_column,
            )),
        );
        program.fields.insert(
            "loc".to_string(),
            EstreeValue::Object(EstreeNode { fields: loc_fields }),
        );

        if program_body_is_empty(program) {
            let trailing = parse_leading_comment_nodes(self.source, self.offsets.global_start);
            if !trailing.is_empty() {
                let trailing_values = trailing
                    .into_iter()
                    .map(EstreeValue::Object)
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                program.fields.insert(
                    "trailingComments".to_string(),
                    EstreeValue::Array(trailing_values),
                );
            }
        }

        Some(parsed_program.node)
    }

    pub(crate) fn parse_expression_for_template(&self) -> Option<Expression> {
        let allocator = Allocator::default();
        let source_type = OxcSourceType::ts().with_module(true);
        let parsed = OxcParser::new(&allocator, self.source, source_type)
            .parse_expression()
            .ok()?;

        let mut serializer =
            CompactTSSerializer::with_capacity(self.source.len().saturating_mul(8), false);
        parsed.serialize(&mut serializer);
        let json = serializer.into_string();

        let parsed_expression = self.parse_and_normalize_raw_node(&json)?;
        Some(Expression(parsed_expression.node, Default::default()))
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

    fn parse_and_normalize_raw_node(&self, estree_json: &str) -> Option<ParsedRawNode> {
        let mut node = serde_json::from_str::<EstreeNode>(estree_json).ok()?;
        normalize_estree_node(
            &mut node,
            self.source,
            self.offsets.global_start,
            self.offsets.start_line,
            self.offsets.start_column,
        );

        let comments = parse_all_comment_nodes(self.source, self.offsets.global_start);
        if !comments.is_empty() {
            attach_estree_comments_to_tree(
                &mut node,
                self.source,
                self.offsets.global_start,
                &comments,
            );
        }

        Some(ParsedRawNode { node })
    }
}

fn normalize_expression_error_message(message: String) -> Arc<str> {
    match message.as_str() {
        "Cannot assign to this expression" => Arc::from("Assigning to rvalue"),
        _ => Arc::from(message),
    }
}
