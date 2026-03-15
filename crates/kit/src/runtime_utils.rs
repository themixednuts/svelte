use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::{Result, UtilityError};

pub fn get_relative_path(from: &str, to: &str) -> String {
    let mut from_parts = from.split(['/', '\\']).collect::<Vec<_>>();
    let mut to_parts = to.split(['/', '\\']).collect::<Vec<_>>();
    from_parts.pop();

    while from_parts.first() == to_parts.first() {
        from_parts.remove(0);
        to_parts.remove(0);
    }

    let mut parts = vec![".."; from_parts.len()];
    parts.extend(to_parts);
    parts.join("/")
}

pub fn base64_encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

pub fn base64_decode(encoded: &str) -> Result<Vec<u8>> {
    STANDARD.decode(encoded).map_err(|error| {
        UtilityError::InvalidBase64Payload {
            message: error.to_string(),
        }
        .into()
    })
}
