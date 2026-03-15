use std::collections::BTreeMap;

use url::Url;

use crate::{Result, UrlError};

const INTERNAL_BASE: &str = "sveltekit-internal://internal/";

pub fn resolve(base: &str, path: &str) -> String {
    if path.starts_with("//") {
        return path.to_string();
    }

    let internal = Url::parse(INTERNAL_BASE).expect("valid internal base url");
    let base_url = internal.join(base).expect("valid internal base path");
    let resolved = base_url.join(path).expect("valid resolved url");

    if resolved.scheme() == "sveltekit-internal" && resolved.host_str() == Some("internal") {
        let mut value = resolved.path().to_string();
        if let Some(query) = resolved.query() {
            value.push('?');
            value.push_str(query);
        }
        if let Some(fragment) = resolved.fragment() {
            value.push('#');
            value.push_str(fragment);
        }
        value
    } else {
        resolved.to_string()
    }
}

pub fn is_root_relative(path: &str) -> bool {
    path.starts_with('/') && !path.starts_with("//")
}

pub fn normalize_path(path: &str, trailing_slash: &str) -> String {
    if path == "/" || trailing_slash == "ignore" {
        return path.to_string();
    }

    match trailing_slash {
        "never" if path.ends_with('/') => path[..path.len() - 1].to_string(),
        "always" if !path.ends_with('/') => format!("{path}/"),
        _ => path.to_string(),
    }
}

pub fn decode_pathname(pathname: &str) -> String {
    try_decode_pathname(pathname).expect("valid percent-encoded pathname")
}

pub fn try_decode_pathname(pathname: &str) -> Result<String> {
    let parts = pathname
        .split("%25")
        .map(|segment| {
            percent_encoding::percent_decode_str(segment)
                .decode_utf8()
                .map(|decoded| decoded.into_owned())
                .map_err(|error| {
                    UrlError::InvalidPathnameUtf8 {
                        pathname: pathname.to_string(),
                        message: error.to_string(),
                    }
                    .into()
                })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(parts.join("%25"))
}

pub fn decode_params(params: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    params
        .into_iter()
        .map(|(key, value)| decode_uri_component(&value).map(|value| (key, value)))
        .collect()
}

pub fn decode_uri(uri: &str) -> Result<String> {
    percent_decode_with_context(uri, true)
}

pub fn strip_hash(href: &str) -> &str {
    href.split('#').next().unwrap_or(href)
}

fn decode_uri_component(uri: &str) -> Result<String> {
    percent_decode_with_context(uri, false)
}

fn percent_decode_with_context(uri: &str, preserve_reserved: bool) -> Result<String> {
    let bytes = uri.as_bytes();
    let mut index = 0usize;
    let mut decoded = String::with_capacity(uri.len());
    let mut pending = Vec::new();

    let flush_pending = |pending: &mut Vec<u8>, decoded: &mut String, uri: &str| -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }

        let text = std::str::from_utf8(pending).map_err(|error| UrlError::InvalidUriUtf8 {
            uri: uri.to_string(),
            message: error.to_string(),
        })?;
        decoded.push_str(text);
        pending.clear();
        Ok(())
    };

    while index < bytes.len() {
        if bytes[index] != b'%' {
            flush_pending(&mut pending, &mut decoded, uri)?;
            decoded.push(bytes[index] as char);
            index += 1;
            continue;
        }

        if index + 2 >= bytes.len() {
            return Err(UrlError::IncompletePercentEncoding {
                uri: uri.to_string(),
            }
            .into());
        }

        let hex = &uri[index + 1..index + 3];
        let value =
            u8::from_str_radix(hex, 16).map_err(|error| UrlError::InvalidPercentEncoding {
                uri: uri.to_string(),
                hex: hex.to_string(),
                message: error.to_string(),
            })?;

        let ch = value as char;
        if preserve_reserved
            && matches!(
                ch,
                ';' | ',' | '/' | '?' | ':' | '@' | '&' | '=' | '+' | '$' | '#'
            )
        {
            flush_pending(&mut pending, &mut decoded, uri)?;
            decoded.push('%');
            decoded.push_str(hex);
        } else {
            pending.push(value);
        }

        index += 3;
    }

    flush_pending(&mut pending, &mut decoded, uri)?;
    Ok(decoded)
}
