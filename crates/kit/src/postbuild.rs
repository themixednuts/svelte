use serde::{Deserialize, Serialize};
use url::Url;

use crate::entities::decode_entities;
use crate::{PostbuildError, Result};

const DOCTYPE: &[u8] = b"DOCTYPE";
const CDATA_OPEN: &[u8] = b"[CDATA[";
const CDATA_CLOSE: &[u8] = b"]]>";
const COMMENT_OPEN: &[u8] = b"--";
const COMMENT_CLOSE: &[u8] = b"-->";

const CRAWLABLE_META_NAME_ATTRS: &[&str] = &[
    "og:url",
    "og:image",
    "og:image:url",
    "og:image:secure_url",
    "og:video",
    "og:video:url",
    "og:video:secure_url",
    "og:audio",
    "og:audio:url",
    "og:audio:secure_url",
    "twitter:image",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlResult {
    pub ids: Vec<String>,
    pub hrefs: Vec<String>,
}

pub fn crawl(html: &str, base: &str) -> Result<CrawlResult> {
    let bytes = html.as_bytes();
    let mut ids = Vec::new();
    let mut hrefs = Vec::new();
    let mut i = 0;
    let mut base = base.to_string();

    while i < bytes.len() {
        if bytes[i] == b'<' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'!' {
                i += 2;

                if starts_with_ascii_case_insensitive(bytes, i, DOCTYPE) {
                    i += DOCTYPE.len();
                    while i < bytes.len() {
                        if bytes[i] == b'>' {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }

                if starts_with(bytes, i, CDATA_OPEN) {
                    i += CDATA_OPEN.len();
                    while i < bytes.len() {
                        if starts_with(bytes, i, CDATA_CLOSE) {
                            i += CDATA_CLOSE.len();
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }

                if starts_with(bytes, i, COMMENT_OPEN) {
                    i += COMMENT_OPEN.len();
                    while i < bytes.len() {
                        if starts_with(bytes, i, COMMENT_CLOSE) {
                            i += COMMENT_CLOSE.len();
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
            }

            i += 1;
            if i < bytes.len() && is_tag_open(bytes[i]) {
                let start = i;
                while i < bytes.len() && is_tag_char(bytes[i]) {
                    i += 1;
                }
                let tag = html[start..i].to_ascii_uppercase();
                let mut attributes = std::collections::BTreeMap::<String, String>::new();

                if tag == "SCRIPT" || tag == "STYLE" {
                    while i < bytes.len() {
                        if bytes[i] == b'<'
                            && i + 2 + tag.len() <= bytes.len()
                            && bytes[i + 1] == b'/'
                            && html[i + 2..i + 2 + tag.len()].eq_ignore_ascii_case(&tag)
                        {
                            break;
                        }
                        i += 1;
                    }
                }

                while i < bytes.len() {
                    let start = i;
                    if bytes[start] == b'>' {
                        break;
                    }

                    if is_attribute_name(bytes[start]) {
                        i += 1;
                        while i < bytes.len() && is_attribute_name(bytes[i]) {
                            i += 1;
                        }
                        let name = html[start..i].to_ascii_lowercase();
                        while i < bytes.len() && is_whitespace(bytes[i]) {
                            i += 1;
                        }

                        if i < bytes.len() && bytes[i] == b'=' {
                            i += 1;
                            while i < bytes.len() && is_whitespace(bytes[i]) {
                                i += 1;
                            }

                            let value =
                                if i < bytes.len() && (bytes[i] == b'\'' || bytes[i] == b'"') {
                                    let quote = bytes[i];
                                    i += 1;
                                    let start = i;
                                    let mut escaped = false;

                                    while i < bytes.len() {
                                        if escaped {
                                            escaped = false;
                                        } else if bytes[i] == quote {
                                            break;
                                        } else if bytes[i] == b'\\' {
                                            escaped = true;
                                        }
                                        i += 1;
                                    }

                                    html[start..i].to_string()
                                } else {
                                    let start = i;
                                    while i < bytes.len()
                                        && bytes[i] != b'>'
                                        && !is_whitespace(bytes[i])
                                    {
                                        i += 1;
                                    }
                                    let value = html[start..i].to_string();
                                    i = i.saturating_sub(1);
                                    value
                                };

                            attributes.insert(name, decode_entities(&value));
                        } else {
                            i = i.saturating_sub(1);
                        }
                    }

                    i += 1;
                }

                let href = attributes.get("href").cloned();
                let id = attributes.get("id").cloned();
                let name = attributes.get("name").cloned();
                let property = attributes.get("property").cloned();
                let rel = attributes.get("rel").cloned();
                let src = attributes.get("src").cloned();
                let srcset = attributes.get("srcset").cloned();
                let content = attributes.get("content").cloned();

                if let Some(href) = href.filter(|href| !href.is_empty()) {
                    if tag == "BASE" {
                        base = resolve_url(&base, &href)?;
                    } else if rel.as_ref().is_none_or(|rel| !contains_external_rel(rel)) {
                        hrefs.push(resolve_url(&base, &href)?);
                    }
                }

                if let Some(id) = id.filter(|id| !id.is_empty()) {
                    ids.push(decode_uri(&id)?);
                }

                if tag == "A"
                    && let Some(name) = name.as_ref().filter(|name| !name.is_empty())
                {
                    ids.push(decode_uri(name)?);
                }

                if let Some(src) = src.filter(|src| !src.is_empty()) {
                    hrefs.push(resolve_url(&base, &src)?);
                }

                if let Some(srcset) = srcset.filter(|srcset| !srcset.is_empty()) {
                    for candidate in parse_srcset_candidates(&srcset) {
                        if let Some(src) = candidate.split(char::is_whitespace).next()
                            && !src.is_empty()
                        {
                            hrefs.push(resolve_url(&base, src)?);
                        }
                    }
                }

                if tag == "META"
                    && let Some(content) = content.filter(|content| !content.is_empty())
                {
                    let attr = name.as_ref().or(property.as_ref());
                    if attr
                        .as_ref()
                        .is_some_and(|attr| CRAWLABLE_META_NAME_ATTRS.contains(&attr.as_str()))
                    {
                        hrefs.push(resolve_url(&base, &content)?);
                    }
                }
            }
        }

        i += 1;
    }

    Ok(CrawlResult { ids, hrefs })
}

fn resolve_url(base: &str, path: &str) -> Result<String> {
    if path.starts_with("//") {
        return Ok(path.to_string());
    }

    let internal =
        Url::parse("sveltekit-internal://internal/").expect("static internal URL should parse");
    let base_url = internal
        .join(base)
        .map_err(|error| PostbuildError::ResolveBaseUrl {
            base: base.to_string(),
            message: error.to_string(),
        })?;
    let url = base_url
        .join(path)
        .map_err(|error| PostbuildError::ResolveUrl {
            path: path.to_string(),
            message: error.to_string(),
        })?;

    if url.scheme() == internal.scheme() {
        let mut resolved = url.path().to_string();
        if let Some(query) = url.query() {
            resolved.push('?');
            resolved.push_str(query);
        }
        if let Some(fragment) = url.fragment() {
            resolved.push('#');
            resolved.push_str(fragment);
        }
        Ok(resolved)
    } else {
        Ok(url.to_string())
    }
}

fn decode_uri(value: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut i = 0;
    let raw = value.as_bytes();

    while i < raw.len() {
        if raw[i] == b'%' {
            if i + 2 >= raw.len() {
                return Err(PostbuildError::IncompletePercentEncoding {
                    value: value.to_string(),
                }
                .into());
            }
            let hex = &value[i + 1..i + 3];
            let byte = u8::from_str_radix(hex, 16).map_err(|error| {
                PostbuildError::InvalidPercentEncoding {
                    value: value.to_string(),
                    hex: hex.to_string(),
                    message: error.to_string(),
                }
            })?;
            bytes.push(byte);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }

    String::from_utf8(bytes).map_err(|error| {
        PostbuildError::InvalidDecodedUriUtf8 {
            value: value.to_string(),
            message: error.to_string(),
        }
        .into()
    })
}

fn parse_srcset_candidates(value: &str) -> Vec<String> {
    let mut value = value.trim().to_string();
    let mut candidates = Vec::new();
    loop {
        let mut inside_url = true;
        let chars = value.as_bytes();
        let mut split_at = None;

        for (i, byte) in chars.iter().copied().enumerate() {
            if byte == b','
                && (!inside_url
                    || (inside_url
                        && chars
                            .get(i + 1)
                            .is_some_and(|byte| byte.is_ascii_whitespace())))
            {
                split_at = Some(i);
                break;
            }
            if byte.is_ascii_whitespace() {
                inside_url = false;
            }
        }

        let Some(i) = split_at else {
            break;
        };

        candidates.push(value[..i].to_string());
        value = value[i + 1..].trim().to_string();
        if value.is_empty() {
            break;
        }
    }

    if !value.is_empty() {
        candidates.push(value);
    }
    candidates
}

fn starts_with(bytes: &[u8], at: usize, pattern: &[u8]) -> bool {
    bytes.get(at..at + pattern.len()) == Some(pattern)
}

fn starts_with_ascii_case_insensitive(bytes: &[u8], at: usize, pattern: &[u8]) -> bool {
    bytes
        .get(at..at + pattern.len())
        .is_some_and(|slice| slice.eq_ignore_ascii_case(pattern))
}

fn is_tag_open(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}

fn is_tag_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

fn is_attribute_name(byte: u8) -> bool {
    !matches!(
        byte,
        b'\t' | b'\n' | 0x0C | b' ' | b'/' | b'>' | b'"' | b'\'' | b'='
    )
}

fn is_whitespace(byte: u8) -> bool {
    byte.is_ascii_whitespace()
}

fn contains_external_rel(value: &str) -> bool {
    value
        .split(|char: char| char.is_ascii_whitespace())
        .any(|part| part.eq_ignore_ascii_case("external"))
}
