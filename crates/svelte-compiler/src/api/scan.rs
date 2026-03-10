use std::sync::Arc;

pub(crate) fn parse_svelte_ignores(comment_data: &str) -> Vec<Arc<str>> {
    let trimmed = comment_data.trim_start();
    let Some(rest) = trimmed.strip_prefix("svelte-ignore") else {
        return Vec::new();
    };

    let mut ignores = Vec::new();
    let mut token = String::new();
    for ch in rest.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '$' {
            token.push(ch);
            continue;
        }
        if !token.is_empty() {
            push_ignore_code_variants(&token, &mut ignores);
            token.clear();
        }
    }
    if !token.is_empty() {
        push_ignore_code_variants(&token, &mut ignores);
    }

    ignores
}

pub(crate) fn migrate_svelte_ignore(text: &str) -> Option<String> {
    let prefix_len = svelte_ignore_directive_prefix_len(text)?;
    let payload = &text[prefix_len..];

    let mut output = String::with_capacity(text.len());
    output.push_str(&text[..prefix_len]);

    let mut cursor = 0usize;
    let mut changed = false;

    while let Some((start, end)) = find_next_hyphenated_ignore_code(payload, cursor) {
        output.push_str(&payload[cursor..start]);

        let code = &payload[start..end];
        let mut replacement = legacy_ignore_replacement(code)
            .map(str::to_string)
            .unwrap_or_else(|| code.replace('-', "_"));

        if find_next_hyphenated_ignore_code(payload, end).is_some() {
            replacement.push(',');
        }

        changed |= replacement != code;
        output.push_str(&replacement);
        cursor = end;
    }

    if !changed {
        return None;
    }

    output.push_str(&payload[cursor..]);
    Some(output)
}

fn push_ignore_code_variants(code: &str, ignores: &mut Vec<Arc<str>>) {
    push_unique_ignore(code, ignores);

    if let Some(replacement) = legacy_ignore_replacement(code) {
        push_unique_ignore(replacement, ignores);
        return;
    }

    if code.contains('-') {
        let normalized = code.replace('-', "_");
        push_unique_ignore(&normalized, ignores);
    }
}

fn legacy_ignore_replacement(code: &str) -> Option<&'static str> {
    match code {
        "non-top-level-reactive-declaration" => Some("reactive_declaration_invalid_placement"),
        "module-script-reactive-declaration" => Some("reactive_declaration_module_script"),
        "empty-block" => Some("block_empty"),
        "avoid-is" => Some("attribute_avoid_is"),
        "invalid-html-attribute" => Some("attribute_invalid_property_name"),
        "a11y-structure" => Some("a11y_figcaption_parent"),
        "illegal-attribute-character" => Some("attribute_illegal_colon"),
        "invalid-rest-eachblock-binding" => Some("bind_invalid_each_rest"),
        "unused-export-let" => Some("export_let_unused"),
        _ => None,
    }
}

fn push_unique_ignore(code: &str, ignores: &mut Vec<Arc<str>>) {
    if ignores.iter().any(|existing| existing.as_ref() == code) {
        return;
    }
    ignores.push(Arc::from(code));
}

fn svelte_ignore_directive_prefix_len(text: &str) -> Option<usize> {
    let trimmed = text.trim_start_matches(char::is_whitespace);
    let rest = trimmed.strip_prefix("svelte-ignore")?;
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some(text.len() - rest.len() + 1)
}

fn find_next_hyphenated_ignore_code(text: &str, mut cursor: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();

    while cursor < bytes.len() {
        if !is_ignore_word_char(bytes[cursor]) {
            cursor += 1;
            continue;
        }

        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && (is_ignore_word_char(bytes[cursor]) || bytes[cursor] == b'-')
        {
            cursor += 1;
        }

        if is_hyphenated_ignore_code(&text[start..cursor]) {
            return Some((start, cursor));
        }
    }

    None
}

fn is_hyphenated_ignore_code(token: &str) -> bool {
    let mut saw_hyphen = false;
    let mut segment_len = 0usize;

    for byte in token.bytes() {
        if byte == b'-' {
            if segment_len == 0 {
                return false;
            }
            saw_hyphen = true;
            segment_len = 0;
            continue;
        }

        if !is_ignore_word_char(byte) {
            return false;
        }
        segment_len += 1;
    }

    saw_hyphen && segment_len > 0
}

fn is_ignore_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(crate) fn find_valid_legacy_closing_tag_start(
    source: &str,
    start: usize,
    end: usize,
    tag_name: &str,
) -> Option<usize> {
    if start >= end || tag_name.is_empty() {
        return None;
    }

    let needle = format!("</{tag_name}");
    let mut search_from = start;

    while search_from < end {
        let rel = source.get(search_from..end)?.find(&needle)?;
        let candidate_start = search_from + rel;
        let next = source
            .as_bytes()
            .get(candidate_start + needle.len())
            .copied();
        if next.is_none_or(|byte| matches!(byte, b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r')) {
            return Some(candidate_start);
        }
        search_from = candidate_start.saturating_add(1);
    }

    None
}

pub(crate) fn find_matching_paren(source: &str, open_index: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open_index).copied() != Some(b'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut cursor = open_index + 1;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    while cursor < bytes.len() {
        let ch = bytes[cursor] as char;
        if escaped {
            escaped = false;
            cursor += 1;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            cursor += 1;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            cursor += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            cursor += 1;
            continue;
        }
        if in_single || in_double {
            cursor += 1;
            continue;
        }
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return Some(cursor);
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        cursor += 1;
    }

    None
}

pub(crate) fn next_char_boundary(source: &str, index: usize) -> usize {
    if index >= source.len() {
        return source.len();
    }
    let mut next = index + 1;
    while next < source.len() && !source.is_char_boundary(next) {
        next += 1;
    }
    next
}

#[cfg(test)]
mod tests {
    use super::{migrate_svelte_ignore, parse_svelte_ignores};

    #[test]
    fn parse_svelte_ignores_keeps_legacy_and_normalized_codes() {
        let ignores = parse_svelte_ignores(
            " svelte-ignore non-top-level-reactive-declaration a11y-something-something ",
        );

        let codes = ignores.iter().map(|code| code.as_ref()).collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec![
                "non-top-level-reactive-declaration",
                "reactive_declaration_invalid_placement",
                "a11y-something-something",
                "a11y_something_something",
            ]
        );
    }

    #[test]
    fn migrate_svelte_ignore_rewrites_legacy_codes() {
        assert_eq!(
            migrate_svelte_ignore(
                " svelte-ignore non-top-level-reactive-declaration a11y-something-something a11y-something-something2 ",
            ),
            Some(
                " svelte-ignore reactive_declaration_invalid_placement, a11y_something_something, a11y_something_something2 "
                    .to_string(),
            )
        );
    }

    #[test]
    fn migrate_svelte_ignore_ignores_non_directives() {
        assert_eq!(migrate_svelte_ignore(" not-a-directive a-b "), None);
        assert_eq!(migrate_svelte_ignore(" svelte-ignore already_valid "), None);
    }
}
