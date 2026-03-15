use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::{ExportsInternalError, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct HttpErrorClass {
    pub status: u16,
    pub body: Value,
}

impl HttpErrorClass {
    pub fn new(status: u16) -> Self {
        Self::from_body(status, message_body(format!("Error: {status}")))
    }

    pub fn from_message(status: u16, message: impl Into<String>) -> Self {
        Self::from_body(status, message_body(message))
    }

    pub fn from_body(status: u16, body: Value) -> Self {
        Self { status, body }
    }
}

impl std::fmt::Display for HttpErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.body)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedirectClass {
    pub status: u16,
    pub location: String,
}

impl RedirectClass {
    pub fn new(status: u16, location: impl Into<String>) -> Self {
        Self {
            status,
            location: location.into(),
        }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct SvelteKitErrorClass {
    pub status: u16,
    pub text: String,
    message: String,
}

impl SvelteKitErrorClass {
    pub fn new(status: u16, text: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            text: text.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionFailure<T> {
    pub status: u16,
    pub data: T,
}

impl<T> ActionFailure<T> {
    pub fn new(status: u16, data: T) -> Self {
        Self { status, data }
    }
}

#[derive(Debug, Error)]
#[error("Validation failed")]
pub struct ValidationErrorClass {
    pub issues: Vec<Value>,
}

impl ValidationErrorClass {
    pub fn new(issues: Vec<Value>) -> Self {
        Self { issues }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteFunctionKind {
    Command,
    Form,
    Prerender,
    Query,
    QueryBatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFunctionInfo {
    pub kind: RemoteFunctionKind,
    pub dynamic: bool,
    pub id: Option<String>,
    pub name: Option<String>,
}

impl RemoteFunctionInfo {
    pub fn new(kind: RemoteFunctionKind) -> Self {
        Self {
            kind,
            dynamic: false,
            id: None,
            name: None,
        }
    }

    pub fn with_dynamic(mut self, dynamic: bool) -> Self {
        self.dynamic = dynamic;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteExport {
    info: Option<RemoteFunctionInfo>,
}

impl RemoteExport {
    pub fn new(info: Option<RemoteFunctionInfo>) -> Self {
        Self { info }
    }

    pub fn info(&self) -> Option<&RemoteFunctionInfo> {
        self.info.as_ref()
    }
}

pub fn init_remote_functions(
    module: &mut BTreeMap<String, RemoteExport>,
    file: &str,
    hash: &str,
) -> Result<()> {
    if module.contains_key("default") {
        return Err(ExportsInternalError::DefaultRemoteExport {
            file: file.to_string(),
        }
        .into());
    }

    for (name, export) in module {
        let Some(info) = export.info.as_mut() else {
            return Err(ExportsInternalError::InvalidRemoteExport {
                name: name.clone(),
                file: file.to_string(),
            }
            .into());
        };

        info.id = Some(format!("{hash}/{name}"));
        info.name = Some(name.clone());
    }

    Ok(())
}

fn message_body(message: impl Into<String>) -> Value {
    Value::Object(Map::from_iter([(
        "message".to_string(),
        Value::String(message.into()),
    )]))
}
