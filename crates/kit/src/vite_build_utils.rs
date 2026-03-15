use crate::error::{Result, ViteBuildUtilsError};

pub fn create_function_as_string(
    name: &str,
    placeholder_names: &[&str],
    value: &str,
) -> Result<String> {
    let escaped = serde_json::to_string(value).map_err(|error| {
        ViteBuildUtilsError::FunctionBodyStringify {
            message: error.to_string(),
        }
    })?;
    let escaped = escaped
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or(ViteBuildUtilsError::MissingJsonStringDelimiters)?;
    let args = placeholder_names.join(", ");
    Ok(format!("function {name}({args}) {{ return `{escaped}`; }}"))
}
