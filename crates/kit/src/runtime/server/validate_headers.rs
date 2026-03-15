use http::HeaderMap;

use regex::Regex;

fn valid_cache_control_directives() -> &'static [&'static str] {
    &[
        "max-age",
        "public",
        "private",
        "no-cache",
        "no-store",
        "must-revalidate",
        "proxy-revalidate",
        "s-maxage",
        "immutable",
        "stale-while-revalidate",
        "stale-if-error",
        "no-transform",
        "only-if-cached",
        "max-stale",
        "min-fresh",
    ]
}

fn content_type_pattern() -> Regex {
    Regex::new(r"(?i)^(application|audio|example|font|haptics|image|message|model|multipart|text|video|x-[a-z]+)/[-+.\w]+$")
        .expect("valid content-type regex")
}

pub fn validate_headers(headers: &HeaderMap) -> Vec<String> {
    let mut warnings = Vec::new();

    for (key, value) in headers {
        let Ok(value) = value.to_str() else {
            continue;
        };
        let lower = key.as_str().to_ascii_lowercase();
        let warning = match lower.as_str() {
            "cache-control" => validate_cache_control(value),
            "content-type" => validate_content_type(value),
            _ => None,
        };

        if let Some(message) = warning {
            warnings.push(format!("[SvelteKit] {message}"));
        }
    }

    warnings
}

fn validate_cache_control(value: &str) -> Option<String> {
    let error_suffix = format!("(While parsing \"{value}\".)");
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.iter().any(|part| part.is_empty()) {
        return Some(format!(
            "`cache-control` header contains empty directives. {error_suffix}"
        ));
    }

    let directives = parts
        .iter()
        .map(|part| {
            part.split('=')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    let valid = valid_cache_control_directives();
    let invalid = directives
        .iter()
        .find(|directive| !valid.contains(&directive.as_str()))?;

    Some(format!(
        "Invalid cache-control directive \"{invalid}\". Did you mean one of: {}? {error_suffix}",
        valid.join(", ")
    ))
}

fn validate_content_type(value: &str) -> Option<String> {
    let kind = value.split(';').next().unwrap_or_default().trim();
    let error_suffix = format!("(While parsing \"{value}\".)");
    if content_type_pattern().is_match(kind) {
        None
    } else {
        Some(format!(
            "Invalid content-type value \"{kind}\". {error_suffix}"
        ))
    }
}
