use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use crate::{Result, TelemetryError};

pub type TelemetryAttributes = BTreeMap<String, TelemetryValue>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryValue {
    Bool(bool),
    Int(i64),
    String(String),
}

impl From<bool> for TelemetryValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for TelemetryValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<u16> for TelemetryValue {
    fn from(value: u16) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<String> for TelemetryValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for TelemetryValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryException {
    pub name: String,
    pub message: String,
    pub stack: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryStatus {
    pub code: i32,
    pub message: Option<String>,
}

pub trait TelemetrySpan: Send + Sync {
    fn set_attributes(&self, attributes: TelemetryAttributes);
    fn set_status(&self, status: TelemetryStatus);
    fn record_exception(&self, exception: TelemetryException);
    fn end(&self);
}

pub trait TelemetryTracer: Send + Sync {
    fn start_active_span(
        &self,
        name: &str,
        attributes: &TelemetryAttributes,
    ) -> Arc<dyn TelemetrySpan>;
}

#[derive(Clone)]
pub struct TelemetryApi {
    pub tracer: Arc<dyn TelemetryTracer>,
    pub span_status_error_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub status: u16,
    pub location: String,
}

impl Redirect {
    pub fn new(status: u16, location: impl Into<String>) -> Self {
        Self {
            status,
            location: location.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordSpanError {
    Http(HttpError),
    Redirect(Redirect),
    Error {
        name: String,
        message: String,
        stack: Option<String>,
    },
    Other(String),
}

impl fmt::Display for RecordSpanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "{}", error.message),
            Self::Redirect(redirect) => write!(f, "redirect to {}", redirect.location),
            Self::Error { message, .. } => write!(f, "{message}"),
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RecordSpanError {}

pub struct RecordSpanParams<'a, F> {
    pub name: &'a str,
    pub attributes: TelemetryAttributes,
    pub fn_call: F,
}

pub fn load_otel(
    tracing_enabled: bool,
    provider: std::result::Result<TelemetryApi, ()>,
) -> Result<Option<TelemetryApi>> {
    if !tracing_enabled {
        return Ok(None);
    }

    provider
        .map(Some)
        .map_err(|_| TelemetryError::MissingOpenTelemetryApi.into())
}

pub async fn record_span<T, F, Fut>(
    otel: Option<&TelemetryApi>,
    params: RecordSpanParams<'_, F>,
) -> std::result::Result<T, RecordSpanError>
where
    F: FnOnce(Arc<dyn TelemetrySpan>) -> Fut,
    Fut: Future<Output = std::result::Result<T, RecordSpanError>>,
{
    let Some(otel) = otel else {
        return (params.fn_call)(noop_span()).await;
    };

    let span = otel
        .tracer
        .start_active_span(params.name, &params.attributes);

    match (params.fn_call)(Arc::clone(&span)).await {
        Ok(value) => {
            span.end();
            Ok(value)
        }
        Err(error) => {
            match &error {
                RecordSpanError::Http(http) => {
                    span.set_attributes(BTreeMap::from([
                        (
                            format!("{}.result.type", params.name),
                            TelemetryValue::from("known_error"),
                        ),
                        (
                            format!("{}.result.status", params.name),
                            TelemetryValue::from(http.status),
                        ),
                        (
                            format!("{}.result.message", params.name),
                            TelemetryValue::from(http.message.clone()),
                        ),
                    ]));
                    if http.status >= 500 {
                        span.record_exception(TelemetryException {
                            name: "HttpError".to_string(),
                            message: http.message.clone(),
                            stack: None,
                        });
                        span.set_status(TelemetryStatus {
                            code: otel.span_status_error_code,
                            message: Some(http.message.clone()),
                        });
                    }
                }
                RecordSpanError::Redirect(redirect) => {
                    span.set_attributes(BTreeMap::from([
                        (
                            format!("{}.result.type", params.name),
                            TelemetryValue::from("redirect"),
                        ),
                        (
                            format!("{}.result.status", params.name),
                            TelemetryValue::from(redirect.status),
                        ),
                        (
                            format!("{}.result.location", params.name),
                            TelemetryValue::from(redirect.location.clone()),
                        ),
                    ]));
                }
                RecordSpanError::Error {
                    name,
                    message,
                    stack,
                } => {
                    span.set_attributes(BTreeMap::from([(
                        format!("{}.result.type", params.name),
                        TelemetryValue::from("unknown_error"),
                    )]));
                    span.record_exception(TelemetryException {
                        name: name.clone(),
                        message: message.clone(),
                        stack: stack.clone(),
                    });
                    span.set_status(TelemetryStatus {
                        code: otel.span_status_error_code,
                        message: Some(message.clone()),
                    });
                }
                RecordSpanError::Other(_) => {
                    span.set_attributes(BTreeMap::from([(
                        format!("{}.result.type", params.name),
                        TelemetryValue::from("unknown_error"),
                    )]));
                    span.set_status(TelemetryStatus {
                        code: otel.span_status_error_code,
                        message: None,
                    });
                }
            }
            span.end();
            Err(error)
        }
    }
}

pub fn noop_span() -> Arc<dyn TelemetrySpan> {
    Arc::new(NoopSpan)
}

struct NoopSpan;

impl TelemetrySpan for NoopSpan {
    fn set_attributes(&self, _attributes: TelemetryAttributes) {}

    fn set_status(&self, _status: TelemetryStatus) {}

    fn record_exception(&self, _exception: TelemetryException) {}

    fn end(&self) {}
}
