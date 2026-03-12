use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub mod common;
pub mod legacy;
pub mod modern;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Root {
    Legacy(legacy::Root),
    Modern(modern::Root),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CssRootType {
    StyleSheetFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CssAst {
    pub r#type: CssRootType,
    pub children: Box<[modern::CssNode]>,
    pub start: usize,
    pub end: usize,
}
