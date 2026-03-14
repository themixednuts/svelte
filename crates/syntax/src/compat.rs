//! Compatibility helpers for legacy projections.
//!
//! The primary `svelte-syntax` API is the typed Svelte AST plus reusable
//! OXC-backed JS handles in [`crate::js`].

pub use crate::parse::legacy_expression_from_modern_expression;
