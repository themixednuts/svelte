use std::collections::BTreeMap;

use http::{Method, StatusCode};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedResponse {
    pub url: String,
    pub method: Method,
    pub request_body: Option<String>,
    pub request_headers: Option<BTreeMap<String, String>>,
    pub response_body: String,
    pub response_status: StatusCode,
    pub response_status_text: String,
    pub response_headers: BTreeMap<String, String>,
    pub is_b64: bool,
}

pub fn serialize_data(
    fetched: &FetchedResponse,
    filter: impl Fn(&str, &str) -> bool,
    prerendering: bool,
) -> String {
    let mut serialized_headers = Map::new();
    let mut cache_control = None;
    let mut age = None;
    let mut vary_any = false;

    for (key, value) in &fetched.response_headers {
        if filter(key, value) {
            serialized_headers.insert(key.clone(), Value::String(value.clone()));
        }

        match key.as_str() {
            "cache-control" => cache_control = Some(value.as_str()),
            "age" => age = Some(value.as_str()),
            "vary" if value.trim() == "*" => vary_any = true,
            _ => {}
        }
    }

    let safe_payload = format!(
        "{{\"status\":{},\"statusText\":{},\"headers\":{},\"body\":{}}}",
        fetched.response_status.as_u16(),
        json_string(&fetched.response_status_text),
        serde_json::to_string(&serialized_headers).expect("serialize fetched headers"),
        json_string(&fetched.response_body),
    )
    .replace('<', "\\u003C")
    .replace('\u{2028}', "\\u2028")
    .replace('\u{2029}', "\\u2029");

    let mut attrs = vec![
        r#"type="application/json""#.to_string(),
        "data-sveltekit-fetched".to_string(),
        format!(r#"data-url="{}""#, escape_html_attribute(&fetched.url)),
    ];

    if fetched.is_b64 {
        attrs.push("data-b64".to_string());
    }

    if fetched.request_headers.is_some() || fetched.request_body.is_some() {
        let mut values = Vec::new();

        if let Some(headers) = &fetched.request_headers {
            values.push(
                headers
                    .iter()
                    .map(|(key, value)| format!("{key},{value}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }

        if let Some(body) = &fetched.request_body {
            values.push(body.clone());
        }

        attrs.push(format!(r#"data-hash="{}""#, djb2_hash_base36(&values)));
    }

    if !prerendering
        && fetched.method == Method::GET
        && !vary_any
        && let Some(cache_control) = cache_control
        && let Some(ttl) = cache_ttl(cache_control, age)
    {
        attrs.push(format!(r#"data-ttl="{ttl}""#));
    }

    format!(r#"<script {}>{safe_payload}</script>"#, attrs.join(" "))
}

fn cache_ttl(cache_control: &str, age: Option<&str>) -> Option<i64> {
    let age = age.unwrap_or("0").parse::<i64>().ok().unwrap_or(0);

    let directive = cache_control
        .split(',')
        .map(str::trim)
        .find_map(|entry| {
            entry
                .strip_prefix("s-maxage=")
                .or_else(|| entry.strip_prefix("max-age="))
        })?
        .parse::<i64>()
        .ok()?;

    Some(directive - age)
}

fn escape_html_attribute(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(ch),
        }
    }

    escaped
}

fn djb2_hash_base36(values: &[String]) -> String {
    let mut hash: u32 = 5381;

    for value in values {
        for byte in value.as_bytes().iter().rev() {
            hash = hash.wrapping_mul(33) ^ u32::from(*byte);
        }
    }

    base36(hash)
}

fn base36(mut value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }

    let mut digits = Vec::new();

    while value > 0 {
        let digit = (value % 36) as u8;
        digits.push(if digit < 10 {
            (b'0' + digit) as char
        } else {
            (b'a' + (digit - 10)) as char
        });
        value /= 36;
    }

    digits.iter().rev().collect()
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("serialize string")
}
