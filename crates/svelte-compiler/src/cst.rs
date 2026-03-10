use std::marker::PhantomData;

use tree_sitter::{Node, Parser, Tree};

use crate::error::CompileError;
use crate::primitives::{BytePos, Span};
use crate::source::SourceText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Svelte,
}

#[derive(Debug)]
pub struct Document<'src> {
    pub language: Language,
    pub source: SourceText<'src>,
    pub tree: Tree,
}

impl<'src> Document<'src> {
    pub fn root_node(&self) -> Node<'_> {
        self.tree.root_node()
    }

    pub fn root_kind(&self) -> &str {
        self.root_node().kind()
    }

    pub fn has_error(&self) -> bool {
        self.root_node().has_error()
    }

    pub fn root_span(&self) -> Span {
        node_span(self.root_node())
    }
}

pub struct Unconfigured;
pub struct Configured;

pub struct CstParser<State> {
    parser: Parser,
    language: Option<Language>,
    _state: PhantomData<State>,
}

impl CstParser<Unconfigured> {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            language: None,
            _state: PhantomData,
        }
    }

    pub fn configure(mut self, language: Language) -> Result<CstParser<Configured>, CompileError> {
        let ts_lang = match language {
            Language::Svelte => tree_sitter_svelte::language(),
        };

        self.parser
            .set_language(&ts_lang)
            .map_err(|_| CompileError::internal("failed to configure tree-sitter language"))?;

        Ok(CstParser {
            parser: self.parser,
            language: Some(language),
            _state: PhantomData,
        })
    }
}

impl Default for CstParser<Unconfigured> {
    fn default() -> Self {
        Self::new()
    }
}

impl CstParser<Configured> {
    pub fn parse<'src>(
        &mut self,
        source: SourceText<'src>,
    ) -> Result<Document<'src>, CompileError> {
        let tree = self
            .parser
            .parse(source.text, None)
            .ok_or_else(|| CompileError::internal("tree-sitter parser returned no syntax tree"))?;

        Ok(Document {
            language: self
                .language
                .expect("configured parser must contain selected language"),
            source,
            tree,
        })
    }
}

pub fn parse_svelte<'src>(source: SourceText<'src>) -> Result<Document<'src>, CompileError> {
    let mut parser = CstParser::new().configure(Language::Svelte)?;
    parser.parse(source)
}

fn node_span(node: Node<'_>) -> Span {
    let start = BytePos::try_from(node.start_byte()).unwrap_or(BytePos::ZERO);
    let end = BytePos::try_from(node.end_byte()).unwrap_or(BytePos::ZERO);
    Span::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::SourceId;

    #[test]
    fn parses_svelte_cst_document() {
        let source = SourceText::new(SourceId::new(1), "<div>Hello</div>", None);
        let cst = parse_svelte(source).expect("expected tree-sitter CST parse to succeed");

        assert!(!cst.root_kind().is_empty());
        assert!(cst.root_span().end.as_usize() >= cst.source.len());
    }

    #[test]
    fn cst_contains_attribute_nodes() {
        let source = SourceText::new(SourceId::new(2), "<div class='foo'></div>", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("(attribute"));
        assert!(sexp.contains("(attribute_name"));
    }

    #[test]
    fn cst_style_directive_shape() {
        let source = SourceText::new(SourceId::new(3), "<div style:color={myColor}></div>", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("attribute_directive"));
        assert!(sexp.contains("attribute_identifier"));
    }

    #[test]
    fn cst_if_block_shape() {
        let source = SourceText::new(SourceId::new(4), "{#if foo}bar{/if}", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("if_block"));
        assert!(sexp.contains("block_end"));
    }

    #[test]
    fn cst_breaks_unterminated_tags_before_block_branches() {
        let source = SourceText::new(
            SourceId::new(5),
            "{#if true}\n\t<input>\n{:else}\n{/if}\n\n{#await true}\n\t<input>\n{:then f}\n{/await}",
            None,
        );
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.matches("(else_clause").count() + sexp.matches("(await_branch").count() >= 2);
    }

    #[test]
    fn cst_directive_and_debug_tag_shapes() {
        let source = SourceText::new(
            SourceId::new(6),
            "<div let:x style:color={c} transition:fade={t} animate:flip={a} use:act={u}></div>{@debug x, y}",
            None,
        );
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();

        assert!(sexp.contains("attribute_name"));
        assert!(sexp.contains("debug_tag"));
        assert!(sexp.contains("expression_value"));
    }

    #[test]
    fn cst_malformed_snippet_headers_report_error_shape() {
        let source = SourceText::new(SourceId::new(7), "{#snippet children()hi{/snippet}", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();
        assert!(
            cst.has_error(),
            "expected malformed snippet header CST error"
        );
        assert!(sexp.contains("(snippet_name"));

        let source = SourceText::new(SourceId::new(8), "{#snippet children(hi{/snippet}", None);
        let cst = parse_svelte(source).expect("expected cst parse to succeed");
        let sexp = cst.root_node().to_sexp();
        assert!(sexp.contains("(snippet_name"));
        assert!(sexp.contains("(snippet_parameters"));
    }
}
