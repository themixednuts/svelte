use std::{collections::BTreeMap, sync::Arc};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value};

use crate::{Result, RuntimeAppError};

pub type ServerTransportDecoder = Arc<dyn Fn(&Value) -> Result<Value> + Send + Sync>;
pub type ServerTransportEncoder = Arc<dyn Fn(&Value) -> Result<Option<Value>> + Send + Sync>;

#[derive(Clone)]
pub struct ServerTransportHook {
    pub decode: ServerTransportDecoder,
    pub encode: Option<ServerTransportEncoder>,
}

#[derive(Clone, Default)]
pub struct AppState {
    pub decoders: BTreeMap<String, ServerTransportDecoder>,
    pub encoders: BTreeMap<String, ServerTransportEncoder>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("decoders", &self.decoders.keys().collect::<Vec<_>>())
            .field("encoders", &self.encoders.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl AppState {
    pub fn decode_app_value(&self, kind: &str, value: &Value) -> Result<Value> {
        decode_app_value(self, kind, value)
    }

    pub fn encode_app_value(&self, kind: &str, value: &Value) -> Result<Value> {
        encode_app_value(self, kind, value)
    }

    pub fn encode_transport_value(&self, value: &Value) -> Result<Value> {
        encode_transport_value(self, value)
    }

    pub fn decode_transport_value(&self, value: &Value) -> Result<Value> {
        decode_transport_value(self, value)
    }

    pub fn stringify_transport_payload(&self, value: &Value) -> Result<String> {
        stringify_transport_payload(self, value)
    }

    pub fn parse_transport_payload(&self, payload: &str) -> Result<Value> {
        parse_transport_payload(self, payload)
    }

    pub fn stringify_remote_arg(&self, value: Option<&Value>) -> Result<String> {
        stringify_remote_arg(self, value)
    }

    pub fn parse_remote_arg(&self, value: &str) -> Result<Option<Value>> {
        parse_remote_arg(self, value)
    }
}

pub fn decode_app_value(app_state: &AppState, kind: &str, value: &Value) -> Result<Value> {
    let Some(decoder) = app_state.decoders.get(kind).cloned() else {
        return Err(RuntimeAppError::MissingTransportDecoder {
            kind: kind.to_string(),
        }
        .into());
    };

    decoder(value)
}

pub fn encode_app_value(app_state: &AppState, kind: &str, value: &Value) -> Result<Value> {
    let Some(encoder) = app_state.encoders.get(kind).cloned() else {
        return Err(RuntimeAppError::MissingTransportEncoder {
            kind: kind.to_string(),
        }
        .into());
    };

    encoder(value)?.ok_or_else(|| {
        RuntimeAppError::UnencodedTransportValue {
            kind: kind.to_string(),
        }
        .into()
    })
}

pub fn encode_transport_value(app_state: &AppState, value: &Value) -> Result<Value> {
    encode_transport_value_with_state(value, app_state)
}

pub fn decode_transport_value(app_state: &AppState, value: &Value) -> Result<Value> {
    decode_transport_value_with_state(value, app_state)
}

pub fn stringify_transport_payload(app_state: &AppState, value: &Value) -> Result<String> {
    serde_json::to_string(&encode_transport_value(app_state, value)?).map_err(|error| {
        RuntimeAppError::SerializeTransportPayload {
            message: error.to_string(),
        }
        .into()
    })
}

pub fn parse_transport_payload(app_state: &AppState, payload: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(payload).map_err(|error| {
        RuntimeAppError::InvalidTransportPayload {
            message: error.to_string(),
        }
    })?;
    decode_transport_value(app_state, &value)
}

pub fn stringify_remote_arg(app_state: &AppState, value: Option<&Value>) -> Result<String> {
    let Some(value) = value else {
        return Ok(String::new());
    };

    let payload = stringify_transport_payload(app_state, value)?;
    Ok(URL_SAFE_NO_PAD.encode(payload.as_bytes()))
}

pub fn parse_remote_arg(app_state: &AppState, value: &str) -> Result<Option<Value>> {
    if value.is_empty() {
        return Ok(None);
    }

    let bytes = URL_SAFE_NO_PAD.decode(value.as_bytes()).map_err(|error| {
        RuntimeAppError::InvalidRemotePayload {
            message: error.to_string(),
        }
    })?;
    let payload =
        String::from_utf8(bytes).map_err(|error| RuntimeAppError::InvalidRemotePayloadUtf8 {
            message: error.to_string(),
        })?;
    parse_transport_payload(app_state, &payload).map(Some)
}

fn encode_transport_value_with_state(value: &Value, state: &AppState) -> Result<Value> {
    if let Some(encoded) = try_encode_transport_value(value, state)? {
        return Ok(encoded);
    }

    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| encode_transport_value_with_state(item, state))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(entries) => {
            let mut encoded = Map::with_capacity(entries.len());
            for (key, value) in entries {
                encoded.insert(
                    key.clone(),
                    encode_transport_value_with_state(value, state)?,
                );
            }
            Ok(Value::Object(encoded))
        }
        _ => Ok(value.clone()),
    }
}

fn try_encode_transport_value(value: &Value, state: &AppState) -> Result<Option<Value>> {
    for (kind, encoder) in &state.encoders {
        if let Some(encoded) = encoder(value)? {
            return Ok(Some(Value::Object(Map::from_iter([
                ("type".to_string(), Value::String("Transport".to_string())),
                ("kind".to_string(), Value::String(kind.clone())),
                (
                    "value".to_string(),
                    encode_transport_value_with_state(&encoded, state)?,
                ),
            ]))));
        }
    }

    Ok(None)
}

fn decode_transport_value_with_state(value: &Value, state: &AppState) -> Result<Value> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| decode_transport_value_with_state(item, state))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(entries) => {
            if let (Some(Value::String(kind_type)), Some(Value::String(kind)), Some(encoded)) = (
                entries.get("type"),
                entries.get("kind"),
                entries.get("value"),
            ) && kind_type == "Transport"
            {
                let decoded = decode_transport_value_with_state(encoded, state)?;
                let decoder = state.decoders.get(kind).cloned().ok_or_else(|| {
                    RuntimeAppError::MissingTransportDecoder { kind: kind.clone() }
                })?;
                return decoder(&decoded);
            }

            let mut decoded = Map::with_capacity(entries.len());
            for (key, value) in entries {
                decoded.insert(
                    key.clone(),
                    decode_transport_value_with_state(value, state)?,
                );
            }
            Ok(Value::Object(decoded))
        }
        _ => Ok(value.clone()),
    }
}
