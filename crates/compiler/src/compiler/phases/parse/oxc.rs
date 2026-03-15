use std::sync::Arc;

use oxc_ast::ast::Statement;
use oxc_span::SourceType as OxcSourceType;

use svelte_syntax::ParsedJsProgram;

pub(crate) struct SvelteOxcParser<'src> {
    source: &'src str,
    is_ts: bool,
}

impl<'src> SvelteOxcParser<'src> {
    pub(crate) fn new(source: &'src str) -> Self {
        Self {
            source,
            is_ts: false,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_typescript(mut self, is_ts: bool) -> Self {
        self.is_ts = is_ts;
        self
    }

    pub(crate) fn parse_program_for_compile(&self) -> Option<Arc<ParsedJsProgram>> {
        let source_type = source_type(self.is_ts);
        let parsed = Arc::new(ParsedJsProgram::parse(self.source, source_type));
        parsed.errors().is_empty().then_some(parsed)
    }

    pub(crate) fn parse_import_ranges_for_compile(&self) -> Option<Vec<(usize, usize)>> {
        let parsed = ParsedJsProgram::parse(self.source, source_type(self.is_ts));
        if !parsed.errors().is_empty() {
            return None;
        }

        let mut ranges = Vec::with_capacity(parsed.program().body.len());
        for statement in parsed.program().body.iter() {
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
}

fn source_type(is_ts: bool) -> OxcSourceType {
    if is_ts {
        OxcSourceType::ts().with_module(true)
    } else {
        OxcSourceType::mjs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn program_body_len(program: &ParsedJsProgram) -> usize {
        program.program().body.len()
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
}
