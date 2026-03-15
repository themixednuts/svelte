use crate::{PageOptions, statically_analyze_page_options};
use serde_json::Value;

pub fn statically_analyze_vite_page_options(source: &str) -> Option<PageOptions> {
    let options = statically_analyze_page_options(source)?;
    if options
        .values()
        .any(|value| matches!(value, Value::Array(_) | Value::Object(_)))
    {
        return None;
    }
    Some(options)
}
