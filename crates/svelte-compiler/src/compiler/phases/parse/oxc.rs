use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_parser::Parser as OxcParser;
use oxc_span::SourceType as OxcSourceType;
use std::collections::BTreeMap;

use crate::api::modern::{
    RawField, attach_estree_comments_to_tree, estree_node_field, normalize_estree_node,
    parse_all_comment_nodes, parse_leading_comment_nodes, position_raw_node,
};
use crate::ast::modern::{EstreeNode, EstreeValue};

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
    is_ts: bool,
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
            is_ts: false,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_offsets(mut self, offsets: OxcProgramOffsets) -> Self {
        self.offsets = offsets;
        self
    }

    #[allow(dead_code)]
    pub(crate) fn with_typescript(mut self, is_ts: bool) -> Self {
        self.is_ts = is_ts;
        self
    }

    pub(crate) fn parse_program_for_compile(&self) -> Option<EstreeNode> {
        let allocator = Allocator::default();
        let source_type = if self.is_ts {
            OxcSourceType::ts().with_module(true)
        } else {
            OxcSourceType::mjs()
        };
        let parsed = OxcParser::new(&allocator, self.source, source_type).parse();

        let json = if self.is_ts {
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

    pub(crate) fn parse_import_ranges_for_compile(&self) -> Option<Vec<(usize, usize)>> {
        let allocator = Allocator::default();
        let source_type = if self.is_ts {
            OxcSourceType::ts().with_module(true)
        } else {
            OxcSourceType::mjs()
        };
        let parsed = OxcParser::new(&allocator, self.source, source_type).parse();
        if !parsed.errors.is_empty() {
            return None;
        }

        let mut ranges = Vec::with_capacity(parsed.program.body.len());
        for statement in parsed.program.body.iter() {
            let Statement::ImportDeclaration(declaration) = statement else {
                return None;
            };
            ranges.push((
                declaration.span.start as usize,
                declaration.span.end as usize,
            ));
        }

        Some(ranges)
    }

    pub(crate) fn can_parse_program(&self) -> bool {
        let allocator = Allocator::default();
        let source_type = if self.is_ts {
            OxcSourceType::ts().with_module(true)
        } else {
            OxcSourceType::mjs()
        };
        let parsed = OxcParser::new(&allocator, self.source, source_type).parse();
        parsed.errors.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;
    fn program_body_len(program: &EstreeNode) -> usize {
        match estree_node_field(program, RawField::Body) {
            Some(EstreeValue::Array(items)) => items.len(),
            _ => 0,
        }
    }

    #[test]
    fn parses_program_with_rune_call_in_js_mode() {
        let source = "let playbackRate = $state(0.5);";
        let program = SvelteOxcParser::new(source)
            .parse_program_for_compile()
            .expect("program should parse");

        assert_eq!(program_body_len(&program), 1);
    }

    #[test]
    fn parses_program_with_dollar_dollar_props_in_js_mode() {
        let source = "let x = $$props;";
        let program = SvelteOxcParser::new(source)
            .parse_program_for_compile()
            .expect("program should parse");

        assert_eq!(program_body_len(&program), 1);
    }

    #[test]
    fn estree_js_json_for_rune_call_deserializes() {
        let source = "let playbackRate = $state(0.5);";
        let allocator = Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, source, oxc_span::SourceType::mjs()).parse();
        let json = parsed.program.to_estree_js_json(true);

        let _value: serde_json::Value =
            serde_json::from_str(&json).expect("serializer should emit valid json");
        let _node: EstreeNode =
            serde_json::from_str(&json).expect("serializer output should fit EstreeNode");
    }
}
