use std::collections::BTreeMap;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use url::Url;

use crate::{
    error::{CookieError, Result},
    pathname::add_data_suffix,
    url::{normalize_path, resolve},
};

const COOKIE_VALUE_ENCODE_SET: &AsciiSet =
    &CONTROLS.add(b' ').add(b'"').add(b',').add(b';').add(b'\\');

pub type CookieEncoder = fn(&str) -> String;
pub type CookieDecoder = fn(&str) -> String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SameSite {
    Lax,
    Strict,
    None,
}

#[derive(Debug, Clone, Default)]
pub struct CookieOptions {
    pub domain: Option<String>,
    pub encode: Option<CookieEncoder>,
    pub http_only: Option<bool>,
    pub max_age: Option<i64>,
    pub path: Option<String>,
    pub same_site: Option<SameSite>,
    pub secure: Option<bool>,
}

impl PartialEq for CookieOptions {
    fn eq(&self, other: &Self) -> bool {
        self.domain == other.domain
            && encoder_matches(self.encode, other.encode)
            && self.http_only == other.http_only
            && self.max_age == other.max_age
            && self.path == other.path
            && self.same_site == other.same_site
            && self.secure == other.secure
    }
}

impl Eq for CookieOptions {}

#[derive(Debug, Clone, Default)]
pub struct CookieParseOptions {
    pub decode: Option<CookieDecoder>,
}

impl PartialEq for CookieParseOptions {
    fn eq(&self, other: &Self) -> bool {
        decoder_matches(self.decode, other.decode)
    }
}

impl Eq for CookieParseOptions {}

#[derive(Debug, Clone)]
pub struct ResolvedCookieOptions {
    pub domain: Option<String>,
    pub encode: Option<CookieEncoder>,
    pub http_only: bool,
    pub max_age: Option<i64>,
    pub path: String,
    pub same_site: SameSite,
    pub secure: bool,
}

impl PartialEq for ResolvedCookieOptions {
    fn eq(&self, other: &Self) -> bool {
        self.domain == other.domain
            && encoder_matches(self.encode, other.encode)
            && self.http_only == other.http_only
            && self.max_age == other.max_age
            && self.path == other.path
            && self.same_site == other.same_site
            && self.secure == other.secure
    }
}

impl Eq for ResolvedCookieOptions {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub options: ResolvedCookieOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct CookieJar {
    initial_cookies: BTreeMap<String, String>,
    new_cookies: BTreeMap<String, Cookie>,
    pending: Vec<(String, String, ResolvedCookieOptions)>,
    url: Url,
    normalized_url: Option<String>,
    defaults: CookieDefaults,
}

#[derive(Debug, Clone)]
struct CookieDefaults {
    http_only: bool,
    same_site: SameSite,
    secure: bool,
}

impl CookieJar {
    pub fn new(cookie_header: Option<&str>, url: &Url) -> Self {
        Self {
            initial_cookies: parse_cookie_header(cookie_header.unwrap_or_default()),
            new_cookies: BTreeMap::new(),
            pending: Vec::new(),
            url: url.clone(),
            normalized_url: None,
            defaults: CookieDefaults {
                http_only: true,
                same_site: SameSite::Lax,
                secure: !(host_name(url) == "localhost" && url.scheme() == "http"),
            },
        }
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.get_with_options(name, CookieParseOptions::default())
    }

    pub fn get_with_options(&self, name: &str, options: CookieParseOptions) -> Option<String> {
        if let Some(best_match) = self.best_matching_cookie(&self.url, name) {
            return if best_match.options.max_age == Some(0) {
                None
            } else {
                Some(best_match.value.clone())
            };
        }

        self.initial_cookies
            .get(name)
            .map(|value| decode_cookie_value_with(options.decode, value))
    }

    pub fn get_all(&self) -> Vec<CookieEntry> {
        self.get_all_with_options(CookieParseOptions::default())
    }

    pub fn get_all_with_options(&self, options: CookieParseOptions) -> Vec<CookieEntry> {
        let mut cookies = self
            .initial_cookies
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    decode_cookie_value_with(options.decode, value.as_str()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut lookup = BTreeMap::<String, &Cookie>::new();

        for cookie in self.new_cookies.values() {
            if !domain_matches(host_name(&self.url), cookie.options.domain.as_deref()) {
                continue;
            }
            if !path_matches(self.url.path(), Some(cookie.options.path.as_str())) {
                continue;
            }

            let replace = lookup
                .get(&cookie.name)
                .map(|existing| cookie.options.path.len() > existing.options.path.len())
                .unwrap_or(true);
            if replace {
                lookup.insert(cookie.name.clone(), cookie);
            }
        }

        for cookie in lookup.values() {
            if cookie.options.max_age == Some(0) {
                cookies.remove(&cookie.name);
            } else {
                cookies.insert(cookie.name.clone(), cookie.value.clone());
            }
        }

        cookies
            .into_iter()
            .map(|(name, value)| CookieEntry { name, value })
            .collect()
    }

    pub fn set(&mut self, name: &str, value: &str, options: CookieOptions) -> Result<()> {
        let path = options.path.clone().ok_or(CookieError::MissingPath)?;
        let resolved = ResolvedCookieOptions {
            domain: options.domain,
            encode: options.encode,
            http_only: options.http_only.unwrap_or(self.defaults.http_only),
            max_age: options.max_age,
            path,
            same_site: options
                .same_site
                .unwrap_or_else(|| self.defaults.same_site.clone()),
            secure: options.secure.unwrap_or(self.defaults.secure),
        };
        self.set_internal(name, value, resolved)
    }

    pub fn delete(&mut self, name: &str, mut options: CookieOptions) -> Result<()> {
        if options.path.is_none() {
            return Err(CookieError::MissingPath.into());
        }
        options.max_age = Some(0);
        self.set(name, "", options)
    }

    pub fn serialize(&self, name: &str, value: &str, options: CookieOptions) -> Result<String> {
        let path = options.path.clone().ok_or(CookieError::MissingPath)?;

        let mut resolved = ResolvedCookieOptions {
            domain: options.domain,
            encode: options.encode,
            http_only: options.http_only.unwrap_or(self.defaults.http_only),
            max_age: options.max_age,
            path,
            same_site: options
                .same_site
                .unwrap_or_else(|| self.defaults.same_site.clone()),
            secure: options.secure.unwrap_or(self.defaults.secure),
        };

        if resolved.domain.as_deref().is_none()
            || resolved.domain.as_deref() == Some(host_name(&self.url))
        {
            let normalized_url = self
                .normalized_url
                .as_deref()
                .ok_or(CookieError::MissingNormalizedUrl)?;
            resolved.path = resolve(normalized_url, &resolved.path);
        }

        Ok(serialize_cookie(name, value, &resolved))
    }

    pub fn set_internal(
        &mut self,
        name: &str,
        value: &str,
        mut options: ResolvedCookieOptions,
    ) -> Result<()> {
        if self.normalized_url.is_none() {
            self.pending
                .push((name.to_string(), value.to_string(), options.clone()));
            return Ok(());
        }

        if options.domain.as_deref().is_none()
            || options.domain.as_deref() == Some(host_name(&self.url))
        {
            let normalized_url = self
                .normalized_url
                .as_deref()
                .expect("normalized url checked");
            options.path = resolve(normalized_url, &options.path);
        }

        validate_cookie_size(name, value, &options)?;

        let key = generate_cookie_key(options.domain.as_deref(), &options.path, name);
        self.new_cookies.insert(
            key,
            Cookie {
                name: name.to_string(),
                value: value.to_string(),
                options,
            },
        );

        Ok(())
    }

    pub fn set_trailing_slash(&mut self, trailing_slash: &str) -> Result<()> {
        self.normalized_url = Some(normalize_path(self.url.path(), trailing_slash));
        let pending = std::mem::take(&mut self.pending);
        for (name, value, options) in pending {
            self.set_internal(&name, &value, options)?;
        }
        Ok(())
    }

    pub fn get_cookie_header(&self, destination: &Url, header: Option<&str>) -> String {
        let mut combined = self.initial_cookies.clone();

        for cookie in self.new_cookies.values() {
            if !domain_matches(host_name(destination), cookie.options.domain.as_deref()) {
                continue;
            }
            if !path_matches(destination.path(), Some(cookie.options.path.as_str())) {
                continue;
            }

            if cookie.options.max_age == Some(0) {
                combined.remove(&cookie.name);
            } else {
                combined.insert(
                    cookie.name.clone(),
                    encode_cookie_value_with(cookie.options.encode, &cookie.value),
                );
            }
        }

        if let Some(header) = header {
            for (name, value) in parse_cookie_header(header) {
                combined.insert(name, value);
            }
        }

        combined
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn new_cookies(&self) -> &BTreeMap<String, Cookie> {
        &self.new_cookies
    }

    pub fn set_cookie_headers(&self) -> Vec<String> {
        let mut headers = Vec::new();

        for cookie in self.new_cookies.values() {
            headers.push(serialize_cookie(
                &cookie.name,
                &cookie.value,
                &cookie.options,
            ));

            if cookie.options.path.ends_with(".html") {
                let mut data_options = cookie.options.clone();
                data_options.path = add_data_suffix(&cookie.options.path);
                headers.push(serialize_cookie(&cookie.name, &cookie.value, &data_options));
            }
        }

        headers
    }

    fn best_matching_cookie(&self, destination: &Url, name: &str) -> Option<&Cookie> {
        self.new_cookies
            .values()
            .filter(|cookie| {
                cookie.name == name
                    && domain_matches(host_name(destination), cookie.options.domain.as_deref())
                    && path_matches(destination.path(), Some(cookie.options.path.as_str()))
            })
            .max_by_key(|cookie| cookie.options.path.len())
    }
}

pub fn domain_matches(hostname: &str, constraint: Option<&str>) -> bool {
    let Some(constraint) = constraint else {
        return true;
    };

    let normalized = constraint.strip_prefix('.').unwrap_or(constraint);
    hostname == normalized || hostname.ends_with(&format!(".{normalized}"))
}

pub fn path_matches(path: &str, constraint: Option<&str>) -> bool {
    let Some(constraint) = constraint else {
        return true;
    };

    let normalized = constraint.strip_suffix('/').unwrap_or(constraint);
    path == normalized || path.starts_with(&format!("{normalized}/"))
}

fn parse_cookie_header(header: &str) -> BTreeMap<String, String> {
    let mut cookies = BTreeMap::new();

    for part in header.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        cookies.insert(name.trim().to_string(), value.trim().to_string());
    }

    cookies
}

fn generate_cookie_key(domain: Option<&str>, path: &str, name: &str) -> String {
    format!(
        "{}{}?{}",
        domain.unwrap_or_default(),
        path,
        utf8_percent_encode(name, COOKIE_VALUE_ENCODE_SET)
    )
}

fn default_cookie_encode(value: &str) -> String {
    utf8_percent_encode(value, COOKIE_VALUE_ENCODE_SET).to_string()
}

fn default_cookie_decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

fn encoder_matches(left: Option<CookieEncoder>, right: Option<CookieEncoder>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left as usize == right as usize,
        (None, None) => true,
        _ => false,
    }
}

fn decoder_matches(left: Option<CookieDecoder>, right: Option<CookieDecoder>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left as usize == right as usize,
        (None, None) => true,
        _ => false,
    }
}

fn decode_cookie_value_with(decoder: Option<CookieDecoder>, value: &str) -> String {
    decoder.unwrap_or(default_cookie_decode)(value)
}

fn encode_cookie_value_with(encoder: Option<CookieEncoder>, value: &str) -> String {
    encoder.unwrap_or(default_cookie_encode)(value)
}

fn serialize_cookie(name: &str, value: &str, options: &ResolvedCookieOptions) -> String {
    let mut parts = vec![format!(
        "{name}={}",
        encode_cookie_value_with(options.encode, value)
    )];
    parts.push(format!("Path={}", options.path));

    if let Some(domain) = &options.domain {
        parts.push(format!("Domain={domain}"));
    }
    if let Some(max_age) = options.max_age {
        parts.push(format!("Max-Age={max_age}"));
    }
    if options.http_only {
        parts.push("HttpOnly".to_string());
    }
    if options.secure {
        parts.push("Secure".to_string());
    }
    parts.push(format!(
        "SameSite={}",
        match options.same_site {
            SameSite::Lax => "Lax",
            SameSite::Strict => "Strict",
            SameSite::None => "None",
        }
    ));

    parts.join("; ")
}

fn validate_cookie_size(name: &str, value: &str, options: &ResolvedCookieOptions) -> Result<()> {
    if cfg!(debug_assertions) && serialize_cookie(name, value, options).len() > 4129 {
        return Err(CookieError::OversizedCookie {
            name: name.to_string(),
        }
        .into());
    }

    Ok(())
}

fn host_name(url: &Url) -> &str {
    url.host_str().unwrap_or_default()
}
