use std::sync::Arc;

use tree_sitter::Node;

use crate::SourceId;
use crate::api::ParseOptions;
use crate::ast::modern::Root;
use crate::ast::{self, Document};
use crate::cst::parse_svelte;
use crate::error::CompileError;
use crate::source::SourceText;

pub(crate) struct SvelteParserCore<'src> {
    source: &'src str,
    options: ParseOptions,
    source_text: SourceText<'src>,
}

impl<'src> SvelteParserCore<'src> {
    pub(crate) fn new(source: &'src str, options: ParseOptions) -> Self {
        Self {
            source,
            options,
            source_text: SourceText::new(SourceId::new(0), source, None),
        }
    }

    fn parse_root(&self, root: Node<'_>) -> ast::Root {
        match self.options.effective_mode() {
            crate::api::ParseMode::Legacy => ast::Root::Legacy(
                crate::api::parse_legacy_root_from_cst(self.source, root, self.options.loose),
            ),
            crate::api::ParseMode::Modern => ast::Root::Modern(crate::api::parse_root_from_cst(
                self.source,
                root,
                self.options.loose,
            )),
        }
    }

    pub(crate) fn parse(self) -> Result<Document, CompileError> {
        let cst = parse_svelte(self.source_text)?;
        Ok(Document {
            root: self.parse_root(cst.root_node()),
            source: Arc::from(self.source),
        })
    }
}

pub(crate) fn parse_root_for_compile(source: &str) -> Result<Root, CompileError> {
    let source_text = SourceText::new(SourceId::new(0), source, None);
    let cst = parse_svelte(source_text)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::api::parse_root_from_cst(source, cst.root_node(), false)
    }))
    .map_err(|_| CompileError::internal("failed to parse component root from cst"))
}
