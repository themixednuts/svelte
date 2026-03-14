use std::sync::Arc;

pub fn parse_svelte_ignores(comment_data: &str) -> Box<[Arc<str>]> {
    let trimmed = comment_data.trim_start();
    let Some(rest) = trimmed.strip_prefix("svelte-ignore") else {
        return Box::default();
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

    ignores.into_boxed_slice()
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

fn push_unique_ignore(code: &str, ignores: &mut Vec<Arc<str>>) {
    if ignores.iter().any(|existing| existing.as_ref() == code) {
        return;
    }
    ignores.push(Arc::from(code));
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

#[cfg(test)]
mod tests {
    use super::parse_svelte_ignores;

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
}
