use serde_json::Value;

use crate::{Result, RuntimeSharedError};

pub fn validate_depends(route_id: &str, dep: &str) -> Option<String> {
    let scheme = ["moz-icon", "view-source", "jar"]
        .into_iter()
        .find(|scheme| dep.starts_with(&format!("{scheme}:")))?;

    Some(format!(
        "{route_id}: Calling `depends('{dep}')` will throw an error in Firefox because `{scheme}` is a special URI scheme"
    ))
}

pub fn validate_load_response(data: &Value, location_description: Option<&str>) -> Result<()> {
    if data.is_null() || data.is_object() {
        return Ok(());
    }

    let description = location_description.unwrap_or_default();
    let kind = match data {
        Value::Array(_) => "an array".to_string(),
        Value::String(_) => "a string".to_string(),
        Value::Number(_) => "a number".to_string(),
        Value::Bool(_) => "a boolean".to_string(),
        Value::Null => return Ok(()),
        Value::Object(_) => return Ok(()),
    };

    Err(RuntimeSharedError::InvalidLoadResponse {
        location_description: description.to_string(),
        kind,
    }
    .into())
}

pub fn create_remote_key(id: &str, payload: &str) -> String {
    format!("{id}/{payload}")
}
