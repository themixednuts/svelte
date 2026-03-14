//! Svelte AST types.
//!
//! This module defines the tree structures produced by [`parse`](crate::parse)
//! and [`parse_modern_root`](crate::parse_modern_root). The AST comes in two
//! flavors:
//!
//! - [`modern`] — the primary representation with typed nodes for every Svelte
//!   construct (elements, control flow blocks, snippets, scripts, styles).
//! - [`legacy`] — a compatibility representation that mirrors the ESTree-like
//!   shape used by the JavaScript Svelte compiler.
//!
//! Most consumers should use [`modern::Root`] via [`parse_modern_root`](crate::parse_modern_root).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub mod common;
pub mod legacy;
pub mod modern;

/// A parsed Svelte component document containing either a [`modern`] or
/// [`legacy`] AST root.
///
/// Returned by [`parse`](crate::parse).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// The AST root — either modern or legacy depending on [`ParseMode`](crate::ParseMode).
    #[serde(flatten)]
    pub root: Root,

    #[serde(skip)]
    pub(crate) source: Arc<str>,
}

/// The top-level AST, either modern or legacy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Root {
    /// Legacy ESTree-compatible AST.
    Legacy(legacy::Root),
    /// Modern typed AST (recommended).
    Modern(modern::Root),
}

impl Document {
    /// Return the original source text that was parsed.
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Discriminant for a standalone CSS AST root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssRootType {
    /// A `.css` stylesheet file.
    StyleSheetFile,
}

/// A parsed CSS stylesheet AST, returned by [`parse_css`](crate::parse_css).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssAst {
    /// Always [`CssRootType::StyleSheetFile`].
    pub r#type: CssRootType,
    /// Top-level CSS nodes (rules, at-rules, declarations).
    pub children: Box<[modern::CssNode]>,
    /// Byte offset of the start of the stylesheet.
    pub start: usize,
    /// Byte offset of the end of the stylesheet.
    pub end: usize,
}
