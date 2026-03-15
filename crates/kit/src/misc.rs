use serde::Serialize;

use crate::{Result, UtilityError};

pub fn json_stringify<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| {
        UtilityError::JsonSerialize {
            message: error.to_string(),
        }
        .into()
    })
}
