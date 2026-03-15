use std::collections::BTreeSet;

use regex::Regex;

use crate::escape_for_interpolation;

pub struct CssUrlRewriteOptions<'a> {
    pub css: &'a str,
    pub vite_assets: &'a BTreeSet<String>,
    pub static_assets: &'a BTreeSet<String>,
    pub paths_assets: &'a str,
    pub base: &'a str,
    pub static_asset_prefix: &'a str,
}

pub fn tippex_comments_and_strings(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.char_indices().peekable();
    let mut escaped = false;
    let mut in_comment = false;
    let mut quote_mark = None::<char>;

    while let Some((index, ch)) = chars.next() {
        let next = chars.peek().map(|(_, next)| *next);

        if in_comment {
            if ch == '*' && next == Some('/') {
                in_comment = false;
                result.push('*');
            } else {
                result.push(' ');
            }
        } else if quote_mark.is_none() && ch == '/' && next == Some('*') {
            in_comment = true;
            result.push('/');
            result.push('*');
            chars.next();
            continue;
        } else if escaped {
            result.push(' ');
            escaped = false;
        } else if quote_mark.is_some() && ch == '\\' {
            escaped = true;
            result.push(' ');
        } else if Some(ch) == quote_mark {
            quote_mark = None;
            result.push(ch);
        } else if quote_mark.is_some() {
            result.push(' ');
        } else if ch == '"' || ch == '\'' {
            quote_mark = Some(ch);
            result.push(ch);
        } else {
            let _ = index;
            result.push(ch);
        }
    }

    result
}

pub fn fix_css_urls(options: CssUrlRewriteOptions<'_>) -> String {
    let skip_parsing = Regex::new("(?i)url\\(").expect("valid css url regex");
    if !skip_parsing.is_match(options.css) {
        return options.css.to_string();
    }

    let url_function = Regex::new("(?i)url\\(\\s*.*?\\)").expect("valid url function regex");
    let css = escape_for_interpolation(options.css);
    let cleaned = tippex_comments_and_strings(&css);
    let mut output = css.clone();
    let mut replacements = Vec::new();

    let paths_assets = options.paths_assets.trim_end_matches('/');
    let base = options.base.trim_end_matches('/');

    for found in url_function.find_iter(&cleaned) {
        let original = &css[found.start()..found.end()];
        let Some(url) = parse_url_parameter(original) else {
            continue;
        };
        let split_at = url.find(['#', '?']).unwrap_or(url.len());
        let url_without_hash_or_query = &url[..split_at];

        let mut replacement = None::<String>;
        if let Some(filename) = url_without_hash_or_query.strip_prefix("./") {
            if options.vite_assets.contains(filename) {
                replacement = Some(format!("{paths_assets}/{filename}"));
            }
        } else if let Some(filename) =
            url_without_hash_or_query.strip_prefix(options.static_asset_prefix)
        {
            if options.static_assets.contains(filename) {
                replacement = Some(format!("{base}/{filename}"));
            }
        }

        if let Some(prefix) = replacement {
            replacements.push((
                found.start(),
                found.end(),
                original.replacen(url_without_hash_or_query, &prefix, 1),
            ));
        }
    }

    for (start, end, replacement) in replacements.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }

    output
}

fn parse_url_parameter(value: &str) -> Option<&str> {
    let open = value.find('(')?;
    let close = value.rfind(')')?;
    let inner = value[open + 1..close].trim();

    if inner.len() >= 2 {
        let bytes = inner.as_bytes();
        let first = bytes[0];
        let last = *bytes.last()?;
        if (first == b'\'' || first == b'"') && first == last {
            return Some(&inner[1..inner.len() - 1]);
        }
    }

    Some(inner)
}
