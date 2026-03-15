use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use svelte_syntax::ast::{CssAst, CssRootType, Root};
pub use svelte_syntax::ast::{common, legacy, modern};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    #[serde(flatten)]
    pub root: Root,

    #[serde(skip)]
    pub(crate) source: Arc<str>,
}

impl Document {
    pub fn source(&self) -> &str {
        &self.source
    }
}
