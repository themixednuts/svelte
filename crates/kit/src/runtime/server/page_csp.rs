use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::{Result, RuntimeCspError};

use super::sha256;

const EMPTY_COMMENT_HASH: &str = "sha256-9OlNO0DNEeaVzHL4RZwCLsBHA8WBQ8toBp/4F5XV2nc=";
const QUOTED_VALUES: &[&str] = &[
    "self",
    "unsafe-eval",
    "unsafe-hashes",
    "unsafe-inline",
    "none",
    "strict-dynamic",
    "report-sample",
    "wasm-unsafe-eval",
    "script",
];

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCspMode {
    Auto,
    Hash,
    Nonce,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeCspDirectives {
    pub entries: Vec<(String, Vec<String>)>,
}

impl RuntimeCspDirectives {
    pub fn new(
        entries: impl IntoIterator<Item = (impl Into<String>, Vec<impl Into<String>>)>,
    ) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(key, values)| {
                    (
                        key.into(),
                        values.into_iter().map(Into::into).collect::<Vec<_>>(),
                    )
                })
                .collect(),
        }
    }

    fn get(&self, key: &str) -> Option<&[String]> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, values)| values.as_slice())
    }

    fn set(&mut self, key: &str, values: Vec<String>) {
        if let Some((_, existing)) = self.entries.iter_mut().find(|(name, _)| name == key) {
            *existing = values;
            return;
        }

        self.entries.push((key.to_string(), values));
    }

    fn is_empty(&self) -> bool {
        self.entries.iter().all(|(_, values)| values.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCspConfig {
    pub mode: RuntimeCspMode,
    pub directives: RuntimeCspDirectives,
    pub report_only: RuntimeCspDirectives,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCspOptions {
    pub prerender: bool,
    pub dev: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CspProvider {
    use_hashes: bool,
    nonce: String,
    directives: RuntimeCspDirectives,
    script_src_needs_csp: bool,
    script_src_elem_needs_csp: bool,
    style_src_needs_csp: bool,
    style_src_attr_needs_csp: bool,
    style_src_elem_needs_csp: bool,
    script_src: Vec<String>,
    script_src_elem: Vec<String>,
    style_src: Vec<String>,
    style_src_attr: Vec<String>,
    style_src_elem: Vec<String>,
    pub script_needs_nonce: bool,
    pub style_needs_nonce: bool,
    pub script_needs_hash: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Csp {
    pub nonce: String,
    pub csp_provider: CspProvider,
    pub report_only_provider: CspProvider,
}

impl Csp {
    pub fn new(config: RuntimeCspConfig, options: RuntimeCspOptions) -> Result<Self> {
        let nonce = generate_nonce();
        let use_hashes = matches!(config.mode, RuntimeCspMode::Hash)
            || matches!(config.mode, RuntimeCspMode::Auto) && options.prerender;

        if !config.report_only.is_empty()
            && config.report_only.get("report-uri").is_none()
            && config.report_only.get("report-to").is_none()
        {
            return Err(RuntimeCspError::MissingReportOnlySink.into());
        }

        let csp_provider =
            CspProvider::new(use_hashes, config.directives, nonce.clone(), options.dev);
        let report_only_provider =
            CspProvider::new(use_hashes, config.report_only, nonce.clone(), options.dev);

        Ok(Self {
            nonce,
            csp_provider,
            report_only_provider,
        })
    }

    pub fn script_needs_hash(&self) -> bool {
        self.csp_provider.script_needs_hash || self.report_only_provider.script_needs_hash
    }

    pub fn script_needs_nonce(&self) -> bool {
        self.csp_provider.script_needs_nonce || self.report_only_provider.script_needs_nonce
    }

    pub fn style_needs_nonce(&self) -> bool {
        self.csp_provider.style_needs_nonce || self.report_only_provider.style_needs_nonce
    }

    pub fn add_script(&mut self, content: &str) {
        self.csp_provider.add_script(content);
        self.report_only_provider.add_script(content);
    }

    pub fn add_script_hashes(&mut self, hashes: &[&str]) {
        self.csp_provider.add_script_hashes(hashes);
        self.report_only_provider.add_script_hashes(hashes);
    }

    pub fn add_style(&mut self, content: &str) {
        self.csp_provider.add_style(content);
        self.report_only_provider.add_style(content);
    }
}

impl CspProvider {
    fn new(
        use_hashes: bool,
        mut directives: RuntimeCspDirectives,
        nonce: String,
        dev: bool,
    ) -> Self {
        let effective_style_src = directives
            .get("style-src")
            .or_else(|| directives.get("default-src"))
            .map(|values| values.to_vec());
        let style_src_attr = directives
            .get("style-src-attr")
            .map(|values| values.to_vec());
        let style_src_elem = directives
            .get("style-src-elem")
            .map(|values| values.to_vec());

        if dev {
            if let Some(values) = effective_style_src
                && !values.iter().any(|value| value == "unsafe-inline")
            {
                directives.set("style-src", dev_style_values(values));
            }

            if let Some(values) = style_src_attr
                && !values.iter().any(|value| value == "unsafe-inline")
            {
                directives.set("style-src-attr", dev_style_values(values));
            }

            if let Some(values) = style_src_elem
                && !values.iter().any(|value| value == "unsafe-inline")
            {
                directives.set("style-src-elem", dev_style_values(values));
            }
        }

        let effective_script_src = directives
            .get("script-src")
            .or_else(|| directives.get("default-src"));
        let script_src_elem = directives.get("script-src-elem");
        let effective_style_src = directives
            .get("style-src")
            .or_else(|| directives.get("default-src"));
        let style_src_attr = directives.get("style-src-attr");
        let style_src_elem = directives.get("style-src-elem");

        let script_src_needs_csp = script_needs_csp(effective_script_src);
        let script_src_elem_needs_csp = script_needs_csp(script_src_elem);
        let style_src_needs_csp = style_needs_csp(effective_style_src);
        let style_src_attr_needs_csp = style_needs_csp(style_src_attr);
        let style_src_elem_needs_csp = style_needs_csp(style_src_elem);

        let script_needs_csp = script_src_needs_csp || script_src_elem_needs_csp;
        let style_needs_csp =
            !dev && (style_src_needs_csp || style_src_attr_needs_csp || style_src_elem_needs_csp);

        Self {
            use_hashes,
            nonce,
            directives,
            script_src_needs_csp,
            script_src_elem_needs_csp,
            style_src_needs_csp,
            style_src_attr_needs_csp,
            style_src_elem_needs_csp,
            script_src: Vec::new(),
            script_src_elem: Vec::new(),
            style_src: Vec::new(),
            style_src_attr: Vec::new(),
            style_src_elem: Vec::new(),
            script_needs_nonce: script_needs_csp && !use_hashes,
            style_needs_nonce: style_needs_csp && !use_hashes,
            script_needs_hash: script_needs_csp && use_hashes,
        }
    }

    pub fn add_script(&mut self, content: &str) {
        if !(self.script_src_needs_csp || self.script_src_elem_needs_csp) {
            return;
        }

        let source = if self.use_hashes {
            format!("sha256-{}", sha256(content))
        } else {
            format!("nonce-{}", self.nonce)
        };

        if self.script_src_needs_csp {
            push_unique(&mut self.script_src, source.clone());
        }

        if self.script_src_elem_needs_csp {
            push_unique(&mut self.script_src_elem, source);
        }
    }

    pub fn add_script_hashes(&mut self, hashes: &[&str]) {
        for hash in hashes {
            if self.script_src_needs_csp {
                push_unique(&mut self.script_src, (*hash).to_string());
            }

            if self.script_src_elem_needs_csp {
                push_unique(&mut self.script_src_elem, (*hash).to_string());
            }
        }
    }

    pub fn add_style(&mut self, content: &str) {
        if !(self.style_src_needs_csp
            || self.style_src_attr_needs_csp
            || self.style_src_elem_needs_csp)
        {
            return;
        }

        let source = if self.use_hashes {
            format!("sha256-{}", sha256(content))
        } else {
            format!("nonce-{}", self.nonce)
        };

        if self.style_src_needs_csp {
            push_unique(&mut self.style_src, source.clone());
        }

        if self.style_src_attr_needs_csp {
            push_unique(&mut self.style_src_attr, source.clone());
        }

        if self.style_src_elem_needs_csp {
            let existing = self
                .directives
                .get("style-src-elem")
                .is_some_and(|values| values.iter().any(|value| value == EMPTY_COMMENT_HASH));
            if !existing {
                push_unique(&mut self.style_src_elem, EMPTY_COMMENT_HASH.to_string());
            }

            if source != EMPTY_COMMENT_HASH {
                push_unique(&mut self.style_src_elem, source);
            }
        }
    }

    pub fn get_header(&self) -> String {
        self.get_header_internal(false)
    }

    pub fn get_meta(&self) -> Option<String> {
        let content = self.get_header_internal(true);
        (!content.is_empty()).then(|| {
            format!(
                "<meta http-equiv=\"content-security-policy\" content=\"{}\">",
                escape_html_attribute(&content)
            )
        })
    }

    fn get_header_internal(&self, is_meta: bool) -> String {
        let mut directives = self.directives.clone();

        if !self.style_src.is_empty() {
            directives.set(
                "style-src",
                with_generated(
                    directives
                        .get("style-src")
                        .or_else(|| directives.get("default-src"))
                        .unwrap_or(&[]),
                    &self.style_src,
                ),
            );
        }

        if !self.style_src_attr.is_empty() {
            directives.set(
                "style-src-attr",
                with_generated(
                    directives.get("style-src-attr").unwrap_or(&[]),
                    &self.style_src_attr,
                ),
            );
        }

        if !self.style_src_elem.is_empty() {
            directives.set(
                "style-src-elem",
                with_generated(
                    directives.get("style-src-elem").unwrap_or(&[]),
                    &self.style_src_elem,
                ),
            );
        }

        if !self.script_src.is_empty() {
            directives.set(
                "script-src",
                with_generated(
                    directives
                        .get("script-src")
                        .or_else(|| directives.get("default-src"))
                        .unwrap_or(&[]),
                    &self.script_src,
                ),
            );
        }

        if !self.script_src_elem.is_empty() {
            directives.set(
                "script-src-elem",
                with_generated(
                    directives.get("script-src-elem").unwrap_or(&[]),
                    &self.script_src_elem,
                ),
            );
        }

        directives
            .entries
            .iter()
            .filter_map(|(key, values)| {
                if is_meta && matches!(key.as_str(), "frame-ancestors" | "report-uri" | "sandbox") {
                    return None;
                }

                if values.is_empty() {
                    return None;
                }

                let mut directive = vec![key.clone()];
                for value in values {
                    directive.push(quote_source(value));
                }
                Some(directive.join(" "))
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn generate_nonce() -> String {
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&counter.to_le_bytes());
    bytes[8..].copy_from_slice(&(counter.wrapping_mul(0x9E37_79B9_7F4A_7C15)).to_le_bytes());
    STANDARD.encode(bytes)
}

fn dev_style_values(values: Vec<String>) -> Vec<String> {
    let mut filtered = values
        .into_iter()
        .filter(|value| !(value.starts_with("sha256-") || value.starts_with("nonce-")))
        .collect::<Vec<_>>();
    filtered.push("unsafe-inline".to_string());
    filtered
}

fn style_needs_csp(values: Option<&[String]>) -> bool {
    values.is_some_and(|values| !values.iter().any(|value| value == "unsafe-inline"))
}

fn script_needs_csp(values: Option<&[String]>) -> bool {
    values.is_some_and(|values| {
        !values.iter().any(|value| value == "unsafe-inline")
            || values.iter().any(|value| value == "strict-dynamic")
    })
}

fn with_generated(existing: &[String], generated: &[String]) -> Vec<String> {
    let mut combined = existing.to_vec();
    for value in generated {
        push_unique(&mut combined, value.clone());
    }
    combined
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn quote_source(value: &str) -> String {
    let needs_quotes = QUOTED_VALUES.iter().any(|quoted| quoted == &value)
        || value.starts_with("nonce-")
        || value.starts_with("sha256-")
        || value.starts_with("sha384-")
        || value.starts_with("sha512-");

    if needs_quotes {
        format!("'{value}'")
    } else {
        value.to_string()
    }
}

fn escape_html_attribute(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;")
}
