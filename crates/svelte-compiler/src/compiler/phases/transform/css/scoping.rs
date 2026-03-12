//! CSS selector/keyframe rewrite and scoping logic.

use std::{collections::BTreeMap, sync::Arc};

use lightningcss::selector::{
    Component as LightningComponent, Selector as LightningSelector,
    SelectorList as LightningSelectorList,
};
use lightningcss::stylesheet::ParserOptions as LightningParserOptions;
use lightningcss::traits::ParseWithOptions as LightningParseWithOptions;

use crate::api::{find_matching_paren, next_char_boundary};
use crate::ast::modern::{CssAtrule, CssBlock, CssBlockChild, CssNode, CssRule};

use super::TextReplacement;
use super::usage::{
    BoundaryCandidates, CssAttributeFilter, CssAttributeMatchKind, CssElementUsage,
    CssUsageContext, EachBoundaryKind,
};

pub(crate) struct RewriteContext<'a> {
    source: &'a str,
    hash: &'a str,
    keyframes: &'a mut BTreeMap<String, String>,
    replacements: &'a mut Vec<TextReplacement>,
    usage: &'a CssUsageContext,
}

impl<'a> RewriteContext<'a> {
    pub(crate) fn new(
        source: &'a str,
        hash: &'a str,
        keyframes: &'a mut BTreeMap<String, String>,
        replacements: &'a mut Vec<TextReplacement>,
        usage: &'a CssUsageContext,
    ) -> Self {
        Self {
            source,
            hash,
            keyframes,
            replacements,
            usage,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RewriteFrame<'a> {
    inside_keyframes: bool,
    nested_in_rule: bool,
    inside_global_block: bool,
    parent_selector: Option<&'a str>,
}

impl RewriteFrame<'_> {
    pub(crate) fn root() -> Self {
        Self::default()
    }
}

pub(crate) fn collect_css_selector_and_keyframe_rewrites(
    nodes: &[CssNode],
    ctx: &mut RewriteContext<'_>,
    frame: RewriteFrame<'_>,
) {
    for node in nodes {
        match node {
            CssNode::Rule(rule) => {
                collect_css_rule_selector_and_keyframe_rewrites(rule, ctx, frame);
            }
            CssNode::Atrule(atrule) => {
                collect_css_atrule_selector_and_keyframe_rewrites(atrule, ctx, frame);
            }
        }
    }
}

fn collect_css_rule_selector_and_keyframe_rewrites(
    rule: &CssRule,
    ctx: &mut RewriteContext<'_>,
    frame: RewriteFrame<'_>,
) {
    let prelude = ctx
        .source
        .get(rule.prelude.start..rule.prelude.end)
        .unwrap_or_default();
    let is_global_block_rule = prelude.trim() == ":global";
    let children_in_global = frame.inside_global_block
        || is_global_block_rule
        || find_bare_global_pseudo(prelude).is_some();
    let children_nested_in_rule = frame.nested_in_rule || selector_has_local_subject(prelude);

    if is_global_block_rule {
        if let Some(opening) = ctx
            .source
            .get(rule.prelude.start..rule.block.start.saturating_add(1))
        {
            ctx.replacements.push(TextReplacement {
                start: rule.prelude.start,
                end: rule.block.start.saturating_add(1),
                text: format!("/* {opening}*/"),
            });
        }
        if rule.block.end > rule.block.start {
            let close_start = rule.block.end - 1;
            ctx.replacements.push(TextReplacement {
                start: close_start,
                end: rule.block.end,
                text: "/*}*/".to_string(),
            });
        }
    }

    if !frame.inside_keyframes && !frame.inside_global_block {
        let can_prune_to_empty = !prelude.contains(":global");
        if (css_rule_is_empty(rule)
            || (can_prune_to_empty
                && css_rule_will_be_empty_after_pruning(
                    ctx.source,
                    rule,
                    ctx.usage,
                    frame.parent_selector,
                )))
            && !ctx.usage.dev
        {
            if let Some(raw_rule) = ctx.source.get(rule.start..rule.end) {
                ctx.replacements.push(TextReplacement {
                    start: rule.start,
                    end: rule.end,
                    text: format!("/* (empty) {}*/", escape_css_comment_body(raw_rule)),
                });
            }
            return;
        }

        if css_rule_is_unused_with_parent(ctx.source, rule, ctx.usage, frame.parent_selector) {
            if let Some(raw_rule) = ctx.source.get(rule.start..rule.end) {
                ctx.replacements.push(TextReplacement {
                    start: rule.start,
                    end: rule.end,
                    text: format!("/* (unused) {}*/", escape_css_comment_body(raw_rule)),
                });
            }
            return;
        }
    }

    if !frame.inside_keyframes && !frame.inside_global_block && !is_global_block_rule {
        let scoped =
            scope_selector_list_text_with_mode(prelude, ctx.hash, frame.nested_in_rule, false);
        let scoped = comment_unused_selectors_after_scoping(prelude, &scoped, ctx.usage);
        if scoped != prelude {
            ctx.replacements.push(TextReplacement {
                start: rule.prelude.start,
                end: rule.prelude.end,
                text: scoped,
            });
        }
    }

    collect_css_block_selector_and_keyframe_rewrites(
        &rule.block,
        ctx,
        RewriteFrame {
            inside_keyframes: frame.inside_keyframes,
            nested_in_rule: children_nested_in_rule,
            inside_global_block: children_in_global,
            parent_selector: Some(prelude),
        },
    );
}

fn selector_has_local_subject(selector: &str) -> bool {
    let without_global_functions = remove_global_selector_regions(selector);
    let without_bare_global = remove_bare_global_tokens(&without_global_functions);
    let without_root = without_bare_global.replace(":root", "");
    !without_root.trim().is_empty()
}

fn collect_css_atrule_selector_and_keyframe_rewrites(
    atrule: &CssAtrule,
    ctx: &mut RewriteContext<'_>,
    frame: RewriteFrame<'_>,
) {
    let is_keyframes = atrule.name.as_ref().ends_with("keyframes");
    if is_keyframes && !frame.inside_global_block {
        let trimmed = atrule.prelude.trim();
        if !trimmed.is_empty()
            && let Some((next_name, mapped)) = map_keyframe_name(trimmed, ctx.hash)
        {
            ctx.keyframes.insert(trimmed.to_string(), mapped);
            if let Some((prelude_start, prelude_end)) = css_at_rule_prelude_span(ctx.source, atrule)
                && let Some(raw_prelude) = ctx.source.get(prelude_start..prelude_end)
            {
                let rewritten = replace_first_css_identifier(raw_prelude, &next_name);
                if rewritten != raw_prelude {
                    ctx.replacements.push(TextReplacement {
                        start: prelude_start,
                        end: prelude_end,
                        text: rewritten,
                    });
                }
            }
        }
    }

    if let Some(block) = &atrule.block {
        collect_css_block_selector_and_keyframe_rewrites(
            block,
            ctx,
            RewriteFrame {
                inside_keyframes: is_keyframes,
                nested_in_rule: frame.nested_in_rule,
                inside_global_block: frame.inside_global_block,
                parent_selector: frame.parent_selector,
            },
        );
    }
}

fn css_at_rule_prelude_span(source: &str, atrule: &CssAtrule) -> Option<(usize, usize)> {
    let mut index = atrule.start;
    if source.as_bytes().get(index).copied() != Some(b'@') {
        return None;
    }
    index += 1;
    while index < atrule.end {
        let ch = source.as_bytes()[index] as char;
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            index += 1;
        } else {
            break;
        }
    }
    let prelude_start = index;

    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    while index < atrule.end {
        let ch = source.as_bytes()[index] as char;
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            index += 1;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            index += 1;
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            index += 1;
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            '{' | ';' if depth_paren == 0 && depth_bracket == 0 => {
                return Some((prelude_start, index));
            }
            _ => {}
        }

        index += 1;
    }

    Some((prelude_start, atrule.end))
}

fn collect_css_block_selector_and_keyframe_rewrites(
    block: &CssBlock,
    ctx: &mut RewriteContext<'_>,
    frame: RewriteFrame<'_>,
) {
    for child in block.children.iter() {
        match child {
            CssBlockChild::Rule(rule) => {
                collect_css_rule_selector_and_keyframe_rewrites(rule, ctx, frame);
            }
            CssBlockChild::Atrule(atrule) => {
                collect_css_atrule_selector_and_keyframe_rewrites(atrule, ctx, frame);
            }
            CssBlockChild::Declaration(_) => {}
        }
    }
}

fn css_rule_is_empty(rule: &CssRule) -> bool {
    rule.block.children.is_empty()
}

fn css_rule_will_be_empty_after_pruning(
    source: &str,
    rule: &CssRule,
    usage: &CssUsageContext,
    _parent_selector: Option<&str>,
) -> bool {
    if rule.block.children.is_empty() {
        return true;
    }

    for child in &rule.block.children {
        match child {
            CssBlockChild::Declaration(_) => return false,
            CssBlockChild::Atrule(_) => return false,
            CssBlockChild::Rule(nested_rule) => {
                let nested_prelude = source
                    .get(nested_rule.prelude.start..nested_rule.prelude.end)
                    .unwrap_or_default();
                if nested_prelude.contains(":global") {
                    return false;
                }
                let current_prelude = source
                    .get(rule.prelude.start..rule.prelude.end)
                    .unwrap_or_default();
                if css_rule_is_unused_with_parent(source, nested_rule, usage, Some(current_prelude))
                    || css_rule_will_be_empty_after_pruning(
                        source,
                        nested_rule,
                        usage,
                        Some(current_prelude),
                    )
                {
                    continue;
                }
                return false;
            }
        }
    }

    true
}

fn css_rule_is_unused_with_parent(
    source: &str,
    rule: &CssRule,
    usage: &CssUsageContext,
    parent_selector: Option<&str>,
) -> bool {
    if !usage.allow_pruning {
        return false;
    }

    if css_rule_is_in_no_match_section(source, rule) {
        return true;
    }

    let Some(prelude) = source.get(rule.prelude.start..rule.prelude.end) else {
        return false;
    };
    let selectors = split_selectors_top_level(prelude);
    if selectors.is_empty() {
        return false;
    }
    selectors
        .iter()
        .all(|selector| css_selector_segment_is_unused(selector, usage, parent_selector))
}

fn split_selectors_top_level(prelude: &str) -> Vec<String> {
    split_selectors_top_level_ranges(prelude)
        .into_iter()
        .map(|(start, end)| prelude.get(start..end).unwrap_or_default().to_string())
        .collect()
}

fn split_selectors_top_level_ranges(prelude: &str) -> Vec<(usize, usize)> {
    let mut selectors = Vec::new();
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut start = 0usize;

    for (idx, ch) in prelude.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            ',' if depth_paren == 0 && depth_bracket == 0 => {
                selectors.push((start, idx));
                start = idx + 1;
            }
            _ => {}
        }
    }

    selectors.push((start, prelude.len()));
    selectors
}

fn comment_unused_selectors_after_scoping(
    original_prelude: &str,
    scoped_prelude: &str,
    usage: &CssUsageContext,
) -> String {
    if original_prelude.contains("/*") || scoped_prelude.contains("/*") {
        return scoped_prelude.to_string();
    }

    let original_ranges = split_selectors_top_level_ranges(original_prelude);
    if original_ranges.len() <= 1 {
        return annotate_unused_functional_selector_options(
            original_prelude,
            scoped_prelude,
            usage,
        );
    }
    let scoped_ranges = split_selectors_top_level_ranges(scoped_prelude);
    if original_ranges.len() != scoped_ranges.len() {
        return annotate_unused_functional_selector_options(
            original_prelude,
            scoped_prelude,
            usage,
        );
    }

    let mut unused_flags = Vec::with_capacity(original_ranges.len());
    for (start, end) in &original_ranges {
        let selector = original_prelude.get(*start..*end).unwrap_or_default();
        unused_flags.push(css_selector_segment_is_unused(selector, usage, None));
    }

    if unused_flags.iter().all(|flag| !*flag) || unused_flags.iter().all(|flag| *flag) {
        return annotate_unused_functional_selector_options(
            original_prelude,
            scoped_prelude,
            usage,
        );
    }

    rewrite_selector_list_with_unused_comments(
        original_prelude,
        scoped_prelude,
        &original_ranges,
        &scoped_ranges,
        &unused_flags,
        usage,
    )
}

fn rewrite_selector_list_with_unused_comments(
    original_text: &str,
    scoped_text: &str,
    original_ranges: &[(usize, usize)],
    scoped_ranges: &[(usize, usize)],
    unused_flags: &[bool],
    usage: &CssUsageContext,
) -> String {
    let mut out = String::new();
    let mut index = 0usize;
    while index < scoped_ranges.len() {
        let (scoped_start, scoped_end) = scoped_ranges[index];
        if index > 0 {
            let previous_end = scoped_ranges[index - 1].1;
            let separator = scoped_text
                .get(previous_end..scoped_start)
                .unwrap_or_default();
            let previous_unused = *unused_flags.get(index - 1).unwrap_or(&false);
            let current_unused = *unused_flags.get(index).unwrap_or(&false);
            let has_used_before = unused_flags
                .get(..index)
                .is_some_and(|flags| flags.iter().any(|flag| !*flag));
            let should_strip_comma = current_unused || (previous_unused && !has_used_before);
            if should_strip_comma {
                if let Some(comma_index) = separator.find(',') {
                    out.push_str(separator.get(comma_index + 1..).unwrap_or_default());
                } else {
                    out.push_str(separator);
                }
            } else {
                out.push_str(separator);
            }
        }

        let scoped_selector = scoped_text
            .get(scoped_start..scoped_end)
            .unwrap_or_default();

        if !*unused_flags.get(index).unwrap_or(&false) {
            let (orig_start, orig_end) = original_ranges[index];
            let original_selector = original_text.get(orig_start..orig_end).unwrap_or_default();
            out.push_str(&annotate_unused_functional_selector_options(
                original_selector,
                scoped_selector,
                usage,
            ));
            index += 1;
            continue;
        }

        let run_start = index;
        let mut run_end = index;
        while run_end + 1 < scoped_ranges.len() && *unused_flags.get(run_end + 1).unwrap_or(&false)
        {
            run_end += 1;
        }

        let previous_is_used = run_start > 0 && !*unused_flags.get(run_start - 1).unwrap_or(&false);
        if previous_is_used {
            out.push(' ');
        } else {
            let leading_len = scoped_selector
                .len()
                .saturating_sub(scoped_selector.trim_start().len());
            out.push_str(scoped_selector.get(..leading_len).unwrap_or_default());
        }

        let mut originals = Vec::<String>::new();
        for &(orig_start, orig_end) in &original_ranges[run_start..=run_end] {
            let original_selector = original_text.get(orig_start..orig_end).unwrap_or_default();
            let normalized_original =
                remove_bare_global_tokens(&remove_global_selector_regions(original_selector));
            let trimmed = normalized_original.trim();
            if !trimmed.is_empty() {
                originals.push(trimmed.to_string());
            }
        }
        let joined = originals.join(", ");
        let trailing_comma_in_comment = run_start == 0 && run_end + 1 < scoped_ranges.len();
        if trailing_comma_in_comment {
            out.push_str(&format!("/* (unused) {joined},*/"));
        } else {
            out.push_str(&format!("/* (unused) {joined}*/"));
        }

        index = run_end + 1;
    }

    out
}

fn annotate_unused_functional_selector_options(
    original_selector: &str,
    scoped_selector: &str,
    usage: &CssUsageContext,
) -> String {
    let with_is = annotate_unused_functional_options_by_pseudo(
        original_selector,
        scoped_selector,
        ":is(",
        usage,
    );
    annotate_unused_functional_options_by_pseudo(original_selector, &with_is, ":has(", usage)
}

fn annotate_unused_functional_options_by_pseudo(
    original_selector: &str,
    scoped_selector: &str,
    pseudo: &str,
    usage: &CssUsageContext,
) -> String {
    let original_spans = functional_pseudo_inner_spans(original_selector, pseudo);
    if original_spans.is_empty() {
        return scoped_selector.to_string();
    }
    let scoped_spans = functional_pseudo_inner_spans(scoped_selector, pseudo);
    if original_spans.len() != scoped_spans.len() {
        return scoped_selector.to_string();
    }

    let mut replacements = Vec::<(usize, usize, String)>::new();
    for (index, (orig_start, orig_end)) in original_spans.iter().enumerate() {
        let Some((scoped_start, scoped_end)) = scoped_spans.get(index).copied() else {
            continue;
        };
        let original_inner = original_selector
            .get(*orig_start..*orig_end)
            .unwrap_or_default();
        let scoped_inner = scoped_selector
            .get(scoped_start..scoped_end)
            .unwrap_or_default();

        if original_inner.contains("/*") || scoped_inner.contains("/*") {
            continue;
        }

        let original_ranges = split_selectors_top_level_ranges(original_inner);
        let scoped_ranges = split_selectors_top_level_ranges(scoped_inner);
        if original_ranges.len() <= 1 || original_ranges.len() != scoped_ranges.len() {
            continue;
        }

        let mut unused_flags = Vec::with_capacity(original_ranges.len());
        for (start, end) in &original_ranges {
            let selector = original_inner.get(*start..*end).unwrap_or_default();
            unused_flags.push(css_selector_segment_is_unused(selector, usage, None));
        }
        if unused_flags.iter().all(|flag| !*flag) || unused_flags.iter().all(|flag| *flag) {
            continue;
        }

        let rewritten_inner = rewrite_selector_list_with_unused_comments(
            original_inner,
            scoped_inner,
            &original_ranges,
            &scoped_ranges,
            &unused_flags,
            usage,
        );

        replacements.push((scoped_start, scoped_end, rewritten_inner));
    }

    if replacements.is_empty() {
        return scoped_selector.to_string();
    }

    let mut out = scoped_selector.to_string();
    replacements.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (start, end, replacement) in replacements {
        if start <= end && end <= out.len() {
            out.replace_range(start..end, &replacement);
        }
    }
    out
}

fn functional_pseudo_inner_spans(selector: &str, pseudo: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = selector
        .get(search_from..)
        .and_then(|tail| tail.find(pseudo))
    {
        let pseudo_start = search_from + rel;
        let open_index = pseudo_start + pseudo.len() - 1;
        let Some(close_index) = find_matching_paren(selector, open_index) else {
            break;
        };
        if open_index < close_index {
            out.push((open_index + 1, close_index));
        }
        search_from = close_index.saturating_add(1);
    }
    out
}

fn css_selector_segment_is_unused(
    selector: &str,
    usage: &CssUsageContext,
    parent_selector: Option<&str>,
) -> bool {
    if let Some(unused) = bare_global_sibling_selector_is_unused(selector, usage) {
        return unused;
    }

    if let Some(index) = find_bare_global_pseudo(selector) {
        let prefix = selector.get(..index).unwrap_or_default();
        let prefix_without_global = normalize_selector_for_usage(prefix);
        let prefix_normalized = remove_bare_global_tokens(&prefix_without_global);
        let prefix_classes = extract_selector_class_names(&prefix_normalized);
        if prefix_classes
            .iter()
            .any(|class| !usage.classes.contains(class.as_str()))
        {
            return true;
        }

        let prefix_tags = extract_selector_tag_names(&prefix_normalized);
        if !usage.has_dynamic_svelte_element
            && prefix_tags
                .iter()
                .any(|tag| !usage.tags.contains(tag.as_str()))
        {
            return true;
        }

        return false;
    }

    let without_global = normalize_selector_for_usage(selector);
    let normalized = remove_bare_global_tokens(&without_global);
    if normalized.trim().is_empty() {
        return false;
    }

    if let Some(parent_selector) = parent_selector {
        let trimmed = normalized.trim();
        if trimmed.starts_with("&.") {
            let parent_normalized =
                remove_bare_global_tokens(&normalize_selector_for_usage(parent_selector));
            let mut required = extract_selector_class_names(&parent_normalized);
            required.extend(extract_selector_class_names(trimmed));
            required.sort_unstable();
            required.dedup();
            if !required.is_empty() && !any_element_has_all_classes(&required, usage) {
                return true;
            }
        }

        if trimmed == "& &" {
            let parent_normalized =
                remove_bare_global_tokens(&normalize_selector_for_usage(parent_selector));
            let parent_classes = extract_selector_class_names(&parent_normalized);
            if !parent_classes.is_empty()
                && !any_class_has_descendant_with_same_class(&parent_classes, usage)
            {
                return true;
            }
        }
    }

    if normalized.contains("selectedcontent") {
        return false;
    }

    let class_selector_is_bounded = !usage.class_name_unbounded || !normalized.contains('.');
    let respects_component_boundaries = !usage.has_component_like_elements
        || !selector_maybe_cross_component_boundary(&normalized, usage);

    if !usage.has_dynamic_markup
        && let Some(matches) = host_direct_child_selector_matches(&normalized, usage)
    {
        return !matches;
    }

    if !usage.has_dynamic_markup
        && selector_is_simple_sibling_pattern(&normalized)
        && class_selector_is_bounded
        && respects_component_boundaries
    {
        return !simple_structure_selector_matches(&normalized, usage);
    }

    if usage.has_dynamic_markup
        && usage.has_component_like_elements
        && selector_is_simple_sibling_pattern(&normalized)
        && class_selector_is_bounded
        && selector_targets_only_deep_component_tags(&normalized, usage)
        && !selector_maybe_cross_component_boundary(&normalized, usage)
    {
        return !simple_structure_selector_matches(&normalized, usage);
    }

    if usage.has_each_blocks
        && !usage.has_non_each_dynamic_markup
        && selector_is_simple_sibling_pattern(&normalized)
        && class_selector_is_bounded
        && respects_component_boundaries
        && split_selector_parts(&normalized).len() == 2
    {
        return !simple_sibling_selector_matches_with_dynamic_boundaries(&normalized, usage);
    }

    let has_global_pseudo = selector.contains(":global");

    if selector_is_simple_structure_pattern(&normalized)
        && !selector_structure_has_render_uncertainty(&normalized, usage)
        && !usage.has_dynamic_svelte_element
        && class_selector_is_bounded
        && (!has_global_pseudo || normalized.contains('+') || normalized.contains('~'))
        && respects_component_boundaries
    {
        return !simple_structure_selector_matches(&normalized, usage);
    }

    if !usage.has_dynamic_markup
        && normalized.contains(":not(")
        && normalized.contains(' ')
        && !normalized.contains(',')
        && let Some((left, _)) = normalized.split_once(' ')
        && simple_selector_part_is_static(left)
        && !any_matching_element_has_descendant(left.trim(), usage)
    {
        return true;
    }

    let classes = extract_selector_class_names(&normalized);
    let ids = extract_selector_id_names(&normalized);
    let tags = extract_selector_tag_names(&normalized);
    let attributes = extract_selector_attribute_filters(&normalized);

    if classes.is_empty() && ids.is_empty() && tags.is_empty() && attributes.is_empty() {
        return false;
    }

    let has_not = normalized.contains(":not(");
    let has_root = normalized.contains(":root");
    let has_has = normalized.contains(":has(");
    let has_combinator = normalized.contains(' ')
        || normalized.contains('>')
        || normalized.contains('+')
        || normalized.contains('~');
    let root_only_selector = has_root && !has_has && !has_combinator;

    if has_has && normalized.contains("selectedcontent:has(") {
        return false;
    }

    if normalized.contains(":is(")
        && let Some(inner) = functional_pseudo_inner(&normalized, ":is(")
    {
        let options = split_selectors_top_level(inner);
        if !options.is_empty() {
            let any_possible = options.iter().any(|option| {
                let option_without_global = normalize_selector_for_usage(option);
                let option_has_structure = option_without_global.contains(' ')
                    || option_without_global.contains('>')
                    || option_without_global.contains('+')
                    || option_without_global.contains('~')
                    || option_without_global.contains('*');
                if option_has_structure {
                    return true;
                }
                let option_classes = extract_selector_class_names(&option_without_global);
                let option_tags = extract_selector_tag_names(&option_without_global);
                let class_possible = option_classes
                    .iter()
                    .all(|class| usage.classes.contains(class.as_str()));
                let tag_possible = usage.has_dynamic_svelte_element
                    || option_tags.is_empty()
                    || option_tags
                        .iter()
                        .any(|tag| usage.tags.contains(tag.as_str()));
                class_possible && tag_possible
            });
            if !any_possible {
                return true;
            }
        }
    }

    if has_has
        && let Some(has_match) = selector_has_pseudo_matches(&normalized, usage)
        && !has_match
    {
        return true;
    }

    let skip_class_mismatch =
        has_not || has_has || usage.class_name_unbounded || normalized.contains(":is(");
    let should_check_class_mismatch =
        !(skip_class_mismatch || (root_only_selector && tags.is_empty() && attributes.is_empty()));
    if should_check_class_mismatch
        && classes
            .iter()
            .any(|class| !usage.classes.contains(class.as_str()))
    {
        return true;
    }

    if !ids.is_empty() && ids.iter().any(|id| !usage.ids.contains(id.as_str())) {
        return true;
    }

    if !attributes.is_empty()
        && !usage.has_dynamic_svelte_element
        && !selector_attributes_have_any_match(&tags, &attributes, usage)
    {
        return true;
    }

    if usage.has_dynamic_svelte_element && !tags.is_empty() {
        return false;
    }

    if root_only_selector && tags.is_empty() {
        return false;
    }

    tags.iter().any(|tag| !usage.tags.contains(tag.as_str()))
}

fn bare_global_sibling_selector_is_unused(selector: &str, usage: &CssUsageContext) -> Option<bool> {
    if usage.has_render_tags || usage.has_slot_tags || usage.has_component_like_elements {
        return None;
    }

    if !selector.contains(":global(") {
        return None;
    }

    let normalized = remove_bare_global_tokens(&normalize_selector_for_usage(selector));
    if !selector_is_simple_sibling_pattern(&normalized) {
        return None;
    }

    let parts = split_selector_parts(&normalized);
    if parts.len() != 2 {
        return None;
    }

    let right_part = parts[1].1.as_str();
    let has_root_candidate = usage.elements.iter().enumerate().any(|(index, element)| {
        element.parent.is_none() && simple_selector_part_matches_element(right_part, index, usage)
    });

    Some(!has_root_candidate)
}

fn simple_sibling_selector_matches_with_dynamic_boundaries(
    selector: &str,
    usage: &CssUsageContext,
) -> bool {
    if simple_structure_selector_matches(selector, usage) {
        return true;
    }

    if !usage.has_dynamic_markup {
        return false;
    }

    let parts = split_selector_parts(selector);
    if parts.len() != 2 {
        return false;
    }

    let left_part = parts[0].1.as_str();
    let right_part = parts[1].1.as_str();
    if !simple_selector_part_is_static(left_part) || !simple_selector_part_is_static(right_part) {
        return true;
    }

    let relation = normalize_simple_combinator(parts[1].0.as_str());
    boundary_pairs_for_relation(relation)
        .iter()
        .copied()
        .any(|(left_boundary, right_boundary)| {
            let left_candidates = usage.each_boundary_candidates(left_boundary);
            let right_candidates = usage.each_boundary_candidates(right_boundary);
            boundary_pair_matches(left_part, right_part, left_candidates, right_candidates)
        })
}

fn boundary_pairs_for_relation(
    relation: SimpleCombinator,
) -> &'static [(EachBoundaryKind, EachBoundaryKind)] {
    const ADJACENT_PAIRS: &[(EachBoundaryKind, EachBoundaryKind)] = &[
        (EachBoundaryKind::Before, EachBoundaryKind::First),
        (EachBoundaryKind::Last, EachBoundaryKind::First),
        (EachBoundaryKind::Last, EachBoundaryKind::After),
        (EachBoundaryKind::Before, EachBoundaryKind::After),
    ];

    const GENERAL_PAIRS: &[(EachBoundaryKind, EachBoundaryKind)] = &[
        (EachBoundaryKind::Before, EachBoundaryKind::First),
        (EachBoundaryKind::Before, EachBoundaryKind::Last),
        (EachBoundaryKind::Before, EachBoundaryKind::After),
        (EachBoundaryKind::First, EachBoundaryKind::Last),
        (EachBoundaryKind::First, EachBoundaryKind::After),
        (EachBoundaryKind::Last, EachBoundaryKind::After),
        (EachBoundaryKind::Last, EachBoundaryKind::First),
    ];

    match relation {
        SimpleCombinator::Adjacent => ADJACENT_PAIRS,
        SimpleCombinator::GeneralSibling => GENERAL_PAIRS,
        _ => &[],
    }
}

fn selector_targets_only_deep_component_tags(selector: &str, usage: &CssUsageContext) -> bool {
    let tags = extract_selector_tag_names(selector);
    if tags.is_empty() {
        return false;
    }

    tags.iter().any(|tag| {
        let mut has_deep = false;
        let mut has_non_deep = false;
        for element in usage
            .elements
            .iter()
            .filter(|element| element.tag.as_ref() == tag)
        {
            match element.component_depth {
                Some(depth) if depth >= 2 => has_deep = true,
                _ => has_non_deep = true,
            }
        }
        has_deep && !has_non_deep
    })
}

fn boundary_pair_matches(
    left_part: &str,
    right_part: &str,
    left: BoundaryCandidates<'_>,
    right: BoundaryCandidates<'_>,
) -> bool {
    simple_selector_part_matches_boundary(left_part, left)
        && simple_selector_part_matches_boundary(right_part, right)
}

fn simple_selector_part_matches_boundary(part: &str, candidates: BoundaryCandidates<'_>) -> bool {
    let part = part.trim();
    if part.is_empty() || part == "*" {
        return true;
    }
    if part.contains(':') || part.contains('[') || part.contains('&') {
        return true;
    }

    let required_tags = extract_selector_tag_names(part);
    if !required_tags.is_empty()
        && !required_tags
            .iter()
            .any(|tag| candidates.tags.contains(tag.as_str()))
    {
        return false;
    }

    let required_classes = extract_selector_class_names(part);
    required_classes
        .iter()
        .all(|class| candidates.classes.contains(class.as_str()))
}

fn find_bare_global_pseudo(selector: &str) -> Option<usize> {
    let mut search_from = 0usize;
    while let Some(rel) = selector
        .get(search_from..)
        .and_then(|tail| tail.find(":global"))
    {
        let index = search_from + rel;
        let next = selector.as_bytes().get(index + ":global".len()).copied();
        if next != Some(b'(') {
            return Some(index);
        }
        search_from = index + ":global(".len();
    }
    None
}

fn selector_is_simple_sibling_pattern(selector: &str) -> bool {
    (selector.contains('+') || selector.contains('~'))
        && !selector.contains(':')
        && !selector.contains('[')
        && !selector.contains('&')
        && !selector.contains("slot")
        && !selector.chars().any(|ch| ch.is_ascii_uppercase())
}

fn selector_is_simple_structure_pattern(selector: &str) -> bool {
    let parts = split_selector_parts(selector);
    let has_relation = parts.iter().skip(1).any(|(combinator, _)| {
        matches!(
            normalize_simple_combinator(combinator),
            SimpleCombinator::Descendant | SimpleCombinator::Child
        )
    });

    has_relation
        && !selector.contains('+')
        && !selector.contains('~')
        && !selector.contains(':')
        && !selector.contains('[')
        && !selector.contains('&')
        && !selector.contains("slot")
        && !selector.chars().any(|ch| ch.is_ascii_uppercase())
}

fn selector_structure_has_render_uncertainty(selector: &str, usage: &CssUsageContext) -> bool {
    if !usage.has_render_tags {
        return false;
    }

    let parts = split_selector_parts(selector);
    let Some((_, leftmost)) = parts.first() else {
        return false;
    };

    usage
        .render_parent_elements
        .iter()
        .any(|index| simple_selector_part_matches_element(leftmost, *index, usage))
}

fn selector_maybe_cross_component_boundary(selector: &str, usage: &CssUsageContext) -> bool {
    if !usage.has_component_like_elements {
        return false;
    }

    if (selector.contains('+') || selector.contains('~'))
        && extract_selector_tag_names(selector).is_empty()
    {
        return true;
    }

    let tags = extract_selector_tag_names(selector);
    tags.iter().any(|tag| {
        usage
            .elements
            .iter()
            .any(|element| element.tag.as_ref() == tag && element.component_depth == Some(1))
    })
}

fn host_direct_child_selector_matches(selector: &str, usage: &CssUsageContext) -> Option<bool> {
    let trimmed = selector.trim();
    if !trimmed.starts_with(":host") {
        return None;
    }
    let mut rest = trimmed
        .get(":host".len()..)
        .unwrap_or_default()
        .trim_start();
    if !rest.starts_with('>') {
        return None;
    }
    rest = rest.get(1..).unwrap_or_default().trim_start();
    if rest.is_empty() {
        return Some(false);
    }
    if split_selector_parts(rest).len() != 1 {
        return None;
    }

    let matches = usage.elements.iter().enumerate().any(|(index, element)| {
        element.parent.is_none() && simple_selector_part_matches_element(rest, index, usage)
    });
    Some(matches)
}

fn simple_structure_selector_matches(selector: &str, usage: &CssUsageContext) -> bool {
    let parts = split_selector_parts(selector);
    if parts.is_empty() {
        return true;
    }

    let Some((_, rightmost)) = parts.last() else {
        return true;
    };
    let mut candidates = usage
        .elements
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            simple_selector_part_matches_element(rightmost, index, usage).then_some(index)
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return false;
    }

    for part_index in (1..parts.len()).rev() {
        let combinator = normalize_simple_combinator(parts[part_index].0.as_str());
        let left_part = parts[part_index - 1].1.as_str();
        let mut next = Vec::<usize>::new();

        for right_index in candidates {
            match combinator {
                SimpleCombinator::Descendant => {
                    let mut cursor = usage.elements[right_index].parent;
                    while let Some(parent) = cursor {
                        if simple_selector_part_matches_element(left_part, parent, usage)
                            && !next.contains(&parent)
                        {
                            next.push(parent);
                        }
                        cursor = usage.elements[parent].parent;
                    }
                }
                SimpleCombinator::Child => {
                    if let Some(parent) = usage.elements[right_index].parent
                        && simple_selector_part_matches_element(left_part, parent, usage)
                        && !next.contains(&parent)
                    {
                        next.push(parent);
                    }
                }
                SimpleCombinator::Adjacent => {
                    for prev in possible_previous_siblings(right_index, usage) {
                        if simple_selector_part_matches_element(left_part, prev, usage)
                            && !next.contains(&prev)
                        {
                            next.push(prev);
                        }
                    }
                }
                SimpleCombinator::GeneralSibling => {
                    for prev in all_previous_siblings(right_index, usage) {
                        if simple_selector_part_matches_element(left_part, prev, usage)
                            && !next.contains(&prev)
                        {
                            next.push(prev);
                        }
                    }
                }
                SimpleCombinator::Column => {}
            }
        }

        if next.is_empty() {
            return false;
        }
        candidates = next;
    }

    !candidates.is_empty()
}

fn selector_has_pseudo_matches(selector: &str, usage: &CssUsageContext) -> Option<bool> {
    let first_has = selector.find(":has(")?;
    let raw_subject_selector = selector.get(..first_has).unwrap_or_default().trim();
    let subject_selector = trim_leading_universal_subject(raw_subject_selector);
    let mut subject_candidates = if subject_selector.is_empty() {
        (0..usage.elements.len()).collect::<Vec<_>>()
    } else {
        candidate_elements_for_structure_selector(subject_selector, usage)
    };
    if !selector.contains(":global(") {
        let refined_subject_selector = selector_without_has_pseudos(selector);
        let refined_subject_selector = refined_subject_selector.trim();
        let has_structure = refined_subject_selector.contains(' ')
            || refined_subject_selector.contains('>')
            || refined_subject_selector.contains('+')
            || refined_subject_selector.contains('~');
        if !refined_subject_selector.is_empty() && !has_structure {
            let refined_candidates =
                candidate_elements_for_structure_selector(refined_subject_selector, usage);
            if !refined_candidates.is_empty() {
                subject_candidates.retain(|candidate| refined_candidates.contains(candidate));
            }
        }
    }
    if subject_candidates.is_empty() {
        return Some(false);
    }

    let mut search_from = first_has;
    while let Some(rel) = selector
        .get(search_from..)
        .and_then(|tail| tail.find(":has("))
    {
        let pseudo_start = search_from + rel;
        let open_index = pseudo_start + ":has(".len() - 1;
        let close_index = find_matching_paren(selector, open_index)?;
        let inner = selector
            .get(open_index + 1..close_index)
            .unwrap_or_default();
        let options = split_selectors_top_level(inner);
        if options.is_empty() {
            search_from = close_index.saturating_add(1);
            continue;
        }

        let mut filtered_subjects = Vec::new();
        for subject_index in subject_candidates.iter().copied() {
            let has_match = options.iter().any(|option| {
                relative_selector_matches_subject(subject_index, option, usage)
                    || has_render_uncertainty_for_has_option(subject_index, option, usage)
            });
            if has_match {
                filtered_subjects.push(subject_index);
            }
        }

        if filtered_subjects.is_empty() {
            return Some(false);
        }
        subject_candidates = filtered_subjects;
        search_from = close_index.saturating_add(1);
    }

    Some(true)
}

fn selector_without_has_pseudos(selector: &str) -> String {
    let mut out = String::new();
    let mut index = 0usize;

    while index < selector.len() {
        if selector
            .get(index..)
            .is_some_and(|tail| tail.starts_with(":has("))
        {
            let open_index = index + ":has(".len() - 1;
            if let Some(close_index) = find_matching_paren(selector, open_index) {
                index = close_index.saturating_add(1);
                continue;
            }
        }

        let next = next_char_boundary(selector, index);
        out.push_str(selector.get(index..next).unwrap_or_default());
        index = next;
    }

    out
}

fn trim_leading_universal_subject(selector: &str) -> &str {
    let mut current = selector.trim_start();
    loop {
        let Some(stripped_star) = current.strip_prefix('*') else {
            return current;
        };
        let stripped = stripped_star.trim_start();
        if stripped.is_empty() {
            return stripped;
        }
        let next = stripped.as_bytes()[0] as char;
        if next == '>' || next == '+' || next == '~' {
            return stripped;
        }
        current = stripped;
    }
}

fn candidate_elements_for_structure_selector(
    selector: &str,
    usage: &CssUsageContext,
) -> Vec<usize> {
    let parts = split_selector_parts(selector);
    if parts.is_empty() {
        return (0..usage.elements.len()).collect();
    }

    usage
        .elements
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            simple_structure_selector_matches_at(&parts, index, usage).then_some(index)
        })
        .collect()
}

fn simple_structure_selector_matches_at(
    parts: &[(String, String)],
    right_index: usize,
    usage: &CssUsageContext,
) -> bool {
    let Some((_, rightmost)) = parts.last() else {
        return true;
    };
    if !simple_selector_part_matches_element(rightmost, right_index, usage) {
        return false;
    }

    let mut candidates = vec![right_index];
    for part_index in (1..parts.len()).rev() {
        let combinator = normalize_simple_combinator(parts[part_index].0.as_str());
        let left_part = parts[part_index - 1].1.as_str();
        let mut next = Vec::<usize>::new();

        for right in candidates {
            match combinator {
                SimpleCombinator::Descendant => {
                    let mut cursor = usage.elements[right].parent;
                    while let Some(parent) = cursor {
                        if simple_selector_part_matches_element(left_part, parent, usage)
                            && !next.contains(&parent)
                        {
                            next.push(parent);
                        }
                        cursor = usage.elements[parent].parent;
                    }
                }
                SimpleCombinator::Child => {
                    if let Some(parent) = usage.elements[right].parent
                        && simple_selector_part_matches_element(left_part, parent, usage)
                        && !next.contains(&parent)
                    {
                        next.push(parent);
                    }
                }
                SimpleCombinator::Adjacent => {
                    for prev in possible_previous_siblings(right, usage) {
                        if simple_selector_part_matches_element(left_part, prev, usage)
                            && !next.contains(&prev)
                        {
                            next.push(prev);
                        }
                    }
                }
                SimpleCombinator::GeneralSibling => {
                    for prev in all_previous_siblings(right, usage) {
                        if simple_selector_part_matches_element(left_part, prev, usage)
                            && !next.contains(&prev)
                        {
                            next.push(prev);
                        }
                    }
                }
                SimpleCombinator::Column => {}
            }
        }

        if next.is_empty() {
            return false;
        }
        candidates = next;
    }

    !candidates.is_empty()
}

fn relative_selector_matches_subject(
    subject_index: usize,
    option: &str,
    usage: &CssUsageContext,
) -> bool {
    let parts = split_selector_parts(option.trim());
    if parts.is_empty() {
        return false;
    }

    let mut current = vec![subject_index];
    for (index, (raw_combinator, part)) in parts.iter().enumerate() {
        let relation = if index == 0 {
            if raw_combinator.trim().is_empty() {
                SimpleCombinator::Descendant
            } else {
                normalize_simple_combinator(raw_combinator)
            }
        } else {
            normalize_simple_combinator(raw_combinator)
        };

        let mut next = Vec::<usize>::new();
        for anchor in current.iter().copied() {
            for candidate in related_elements(anchor, relation, usage) {
                if simple_selector_part_matches_element(part, candidate, usage)
                    && !next.contains(&candidate)
                {
                    next.push(candidate);
                }
            }
        }

        if next.is_empty() {
            return false;
        }
        current = next;
    }

    !current.is_empty()
}

fn related_elements(
    index: usize,
    relation: SimpleCombinator,
    usage: &CssUsageContext,
) -> Vec<usize> {
    match relation {
        SimpleCombinator::Descendant => usage
            .elements
            .iter()
            .enumerate()
            .filter_map(|(candidate, _)| {
                is_descendant_of(candidate, index, usage).then_some(candidate)
            })
            .collect(),
        SimpleCombinator::Child => usage
            .elements
            .iter()
            .enumerate()
            .filter_map(|(candidate, element)| (element.parent == Some(index)).then_some(candidate))
            .collect(),
        SimpleCombinator::Adjacent => possible_next_siblings(index, usage),
        SimpleCombinator::GeneralSibling => all_next_siblings(index, usage),
        SimpleCombinator::Column => Vec::new(),
    }
}

fn has_render_uncertainty_for_has_option(
    subject_index: usize,
    option: &str,
    usage: &CssUsageContext,
) -> bool {
    if !usage.has_render_tags {
        return false;
    }

    let parts = split_selector_parts(option.trim());
    let Some((raw_combinator, _)) = parts.first() else {
        return false;
    };
    let first_relation = if raw_combinator.trim().is_empty() {
        SimpleCombinator::Descendant
    } else {
        normalize_simple_combinator(raw_combinator)
    };

    match first_relation {
        SimpleCombinator::Descendant | SimpleCombinator::Child => {
            usage.render_parent_elements.contains(&subject_index)
        }
        SimpleCombinator::Adjacent | SimpleCombinator::GeneralSibling => {
            let parent = usage
                .elements
                .get(subject_index)
                .and_then(|element| element.parent);
            if let Some(parent) = parent {
                usage.render_parent_elements.contains(&parent)
            } else {
                usage.root_has_render
            }
        }
        SimpleCombinator::Column => false,
    }
}

#[derive(Clone, Copy)]
enum SimpleCombinator {
    Descendant,
    Child,
    Adjacent,
    GeneralSibling,
    Column,
}

fn normalize_simple_combinator(raw: &str) -> SimpleCombinator {
    if raw.contains("||") {
        return SimpleCombinator::Column;
    }
    if raw.contains('>') {
        return SimpleCombinator::Child;
    }
    if raw.contains('+') {
        return SimpleCombinator::Adjacent;
    }
    if raw.contains('~') {
        return SimpleCombinator::GeneralSibling;
    }
    SimpleCombinator::Descendant
}

fn simple_selector_part_is_static(part: &str) -> bool {
    !part.contains(':') && !part.contains('[') && !part.contains('&')
}

fn simple_selector_part_matches_element(
    part: &str,
    element_index: usize,
    usage: &CssUsageContext,
) -> bool {
    let part = part.trim();
    if part.is_empty() || part == "*" {
        return true;
    }
    if part.contains(':') || part.contains('[') || part.contains('&') {
        return true;
    }
    let element = &usage.elements[element_index];
    let tags = extract_selector_tag_names(part);
    if !tags.is_empty() && !tags.iter().any(|tag| tag == element.tag.as_ref()) {
        return false;
    }
    let classes = extract_selector_class_names(part);
    if classes.is_empty() {
        return true;
    }
    let class_value = element
        .attributes
        .get("class")
        .map(Arc::as_ref)
        .unwrap_or("");
    classes.iter().all(|class| {
        class_value
            .split_ascii_whitespace()
            .any(|token| token == class)
    })
}

fn possible_previous_siblings(index: usize, usage: &CssUsageContext) -> Vec<usize> {
    let Some(element) = usage.elements.get(index) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for candidate in (0..index).rev() {
        let Some(other) = usage.elements.get(candidate) else {
            continue;
        };
        if other.parent != element.parent || other.depth != element.depth {
            continue;
        }
        out.push(candidate);
        if !other.optional {
            break;
        }
    }
    out
}

fn all_previous_siblings(index: usize, usage: &CssUsageContext) -> Vec<usize> {
    let Some(element) = usage.elements.get(index) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for candidate in 0..index {
        if usage
            .elements
            .get(candidate)
            .is_some_and(|other| other.parent == element.parent && other.depth == element.depth)
        {
            out.push(candidate);
        }
    }
    out
}

fn possible_next_siblings(index: usize, usage: &CssUsageContext) -> Vec<usize> {
    let Some(element) = usage.elements.get(index) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for candidate in (index + 1)..usage.elements.len() {
        let Some(other) = usage.elements.get(candidate) else {
            continue;
        };
        if other.parent != element.parent || other.depth != element.depth {
            continue;
        }
        out.push(candidate);
        if !other.optional {
            break;
        }
    }
    out
}

fn all_next_siblings(index: usize, usage: &CssUsageContext) -> Vec<usize> {
    let Some(element) = usage.elements.get(index) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for candidate in index + 1..usage.elements.len() {
        if usage
            .elements
            .get(candidate)
            .is_some_and(|other| other.parent == element.parent && other.depth == element.depth)
        {
            out.push(candidate);
        }
    }
    out
}

fn any_matching_element_has_descendant(left_part: &str, usage: &CssUsageContext) -> bool {
    for (index, _) in usage.elements.iter().enumerate() {
        if !simple_selector_part_matches_element(left_part, index, usage) {
            continue;
        }
        if usage
            .elements
            .iter()
            .enumerate()
            .any(|(candidate, _)| is_descendant_of(candidate, index, usage))
        {
            return true;
        }
    }
    false
}

fn is_descendant_of(index: usize, ancestor: usize, usage: &CssUsageContext) -> bool {
    let mut cursor = usage.elements.get(index).and_then(|element| element.parent);
    while let Some(parent) = cursor {
        if parent == ancestor {
            return true;
        }
        cursor = usage
            .elements
            .get(parent)
            .and_then(|element| element.parent);
    }
    false
}

fn any_element_has_all_classes(classes: &[String], usage: &CssUsageContext) -> bool {
    usage.elements.iter().any(|element| {
        let class_value = element
            .attributes
            .get("class")
            .map(Arc::as_ref)
            .unwrap_or("");
        classes.iter().all(|class| {
            class_value
                .split_ascii_whitespace()
                .any(|token| token == class)
        })
    })
}

fn any_class_has_descendant_with_same_class(classes: &[String], usage: &CssUsageContext) -> bool {
    for class in classes {
        for (index, element) in usage.elements.iter().enumerate() {
            let class_value = element
                .attributes
                .get("class")
                .map(Arc::as_ref)
                .unwrap_or("");
            if !class_value
                .split_ascii_whitespace()
                .any(|token| token == class)
            {
                continue;
            }

            if usage
                .elements
                .iter()
                .enumerate()
                .any(|(candidate, descendant)| {
                    let descendant_class = descendant
                        .attributes
                        .get("class")
                        .map(Arc::as_ref)
                        .unwrap_or("");
                    descendant_class
                        .split_ascii_whitespace()
                        .any(|token| token == class)
                        && is_descendant_of(candidate, index, usage)
                })
            {
                return true;
            }
        }
    }
    false
}

fn selector_attribute_matches_element(
    filter: &CssAttributeFilter,
    element: &CssElementUsage,
) -> bool {
    let Some(value) = element.attributes.get(filter.name.as_ref()) else {
        return false;
    };

    let (value_cmp, filter_cmp);
    let (value_ref, filter_ref) = if filter.case_insensitive {
        value_cmp = value.to_ascii_lowercase();
        filter_cmp = filter.value.to_ascii_lowercase();
        (value_cmp.as_str(), filter_cmp.as_str())
    } else {
        (value.as_ref(), filter.value.as_ref())
    };

    match filter.match_kind {
        CssAttributeMatchKind::Exact => value_ref == filter_ref,
        CssAttributeMatchKind::Word => value_ref
            .split_ascii_whitespace()
            .any(|token| token == filter_ref),
        CssAttributeMatchKind::Prefix => value_ref.starts_with(filter_ref),
        CssAttributeMatchKind::Suffix => value_ref.ends_with(filter_ref),
        CssAttributeMatchKind::Contains => value_ref.contains(filter_ref),
        CssAttributeMatchKind::Dash => {
            value_ref == filter_ref || value_ref.starts_with(&format!("{filter_ref}-"))
        }
    }
}

fn functional_pseudo_inner<'a>(selector: &'a str, pseudo: &str) -> Option<&'a str> {
    let start = selector.find(pseudo)?;
    let open = start + pseudo.len() - 1;
    let close = find_matching_paren(selector, open)?;
    selector.get(open + 1..close)
}

fn selector_attributes_have_any_match(
    tags: &[String],
    attributes: &[CssAttributeFilter],
    usage: &CssUsageContext,
) -> bool {
    if attributes
        .iter()
        .any(|filter| usage.dynamic_attributes.contains(filter.name.as_ref()))
    {
        return true;
    }

    usage.elements.iter().any(|element| {
        if !tags.is_empty() && !tags.iter().any(|tag| tag == element.tag.as_ref()) {
            return false;
        }
        attributes
            .iter()
            .all(|filter| selector_attribute_matches_element(filter, element))
    })
}

fn css_rule_is_in_no_match_section(source: &str, rule: &CssRule) -> bool {
    source
        .get(..rule.start)
        .and_then(|prefix| prefix.rfind("/* no match */"))
        .is_some()
}

fn extract_selector_attribute_filters(selector: &str) -> Vec<CssAttributeFilter> {
    let mut filters = Vec::new();
    let bytes = selector.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let start = index + 1;
        let Some(end_rel) = selector.get(start..).and_then(|tail| tail.find(']')) else {
            break;
        };
        let end = start + end_rel;
        let inner = selector.get(start..end).unwrap_or_default().trim();
        if let Some(filter) = parse_selector_attribute_filter(inner) {
            filters.push(filter);
        }
        index = end + 1;
    }
    filters
}

fn parse_selector_attribute_filter(inner: &str) -> Option<CssAttributeFilter> {
    let (name, rhs) = inner.split_once('=')?;
    let match_kind = if name.trim_end().ends_with('~') {
        CssAttributeMatchKind::Word
    } else if name.trim_end().ends_with('^') {
        CssAttributeMatchKind::Prefix
    } else if name.trim_end().ends_with('$') {
        CssAttributeMatchKind::Suffix
    } else if name.trim_end().ends_with('*') {
        CssAttributeMatchKind::Contains
    } else if name.trim_end().ends_with('|') {
        CssAttributeMatchKind::Dash
    } else {
        CssAttributeMatchKind::Exact
    };
    let name = name
        .trim()
        .trim_end_matches(['~', '^', '$', '*', '|'])
        .to_ascii_lowercase();
    if !is_css_identifier_word(&name) {
        return None;
    }

    let mut rhs = rhs.trim();
    let mut case_insensitive = true;
    if let Some(stripped) = rhs.strip_suffix('i') {
        rhs = stripped.trim_end();
        case_insensitive = true;
    } else if let Some(stripped) = rhs.strip_suffix('s') {
        rhs = stripped.trim_end();
        case_insensitive = false;
    }

    let value = if (rhs.starts_with('\'') && rhs.ends_with('\''))
        || (rhs.starts_with('"') && rhs.ends_with('"'))
    {
        rhs.get(1..rhs.len().saturating_sub(1))
            .unwrap_or_default()
            .to_string()
    } else {
        rhs.to_string()
    };

    Some(CssAttributeFilter {
        name: Arc::from(name),
        value: Arc::from(value),
        case_insensitive,
        match_kind,
    })
}

fn remove_global_selector_regions(selector: &str) -> String {
    let mut out = String::new();
    let mut search_from = 0usize;
    while let Some(rel) = selector
        .get(search_from..)
        .and_then(|tail| tail.find(":global("))
    {
        let start = search_from + rel;
        out.push_str(selector.get(search_from..start).unwrap_or_default());
        let open = start + ":global".len();
        let Some(close) = find_matching_paren(selector, open) else {
            return selector.replace(":global", "");
        };
        search_from = close + 1;
    }
    out.push_str(selector.get(search_from..).unwrap_or_default());
    out.replace(":global", "")
}

fn normalize_selector_for_usage(selector: &str) -> String {
    let mut out = String::new();
    let mut search_from = 0usize;
    while let Some(rel) = selector
        .get(search_from..)
        .and_then(|tail| tail.find(":global("))
    {
        let start = search_from + rel;
        out.push_str(selector.get(search_from..start).unwrap_or_default());
        let open = start + ":global".len();
        let Some(close) = find_matching_paren(selector, open) else {
            return selector.replace(":global", "");
        };
        out.push('*');
        search_from = close + 1;
    }
    out.push_str(selector.get(search_from..).unwrap_or_default());
    out.replace(":global", "")
}

fn extract_selector_class_names(selector: &str) -> Vec<String> {
    if let Some(classes) = extract_selector_class_names_with_lightning(selector)
        && !classes.is_empty()
    {
        return classes;
    }

    let mut classes = Vec::new();
    let bytes = selector.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'.' {
            index += 1;
            continue;
        }
        index += 1;
        let start = index;
        while index < bytes.len() {
            let ch = bytes[index] as char;
            if ch == '\\' {
                index = consume_css_escape(selector, index);
                continue;
            }
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                index += 1;
            } else {
                break;
            }
        }
        if index > start {
            let raw = selector.get(start..index).unwrap_or_default();
            classes.push(raw.replace('\\', ""));
        }
    }
    classes
}

fn extract_selector_class_names_with_lightning(selector: &str) -> Option<Vec<String>> {
    let parsed = LightningSelectorList::parse_string_with_options(
        selector,
        LightningParserOptions::default(),
    )
    .ok()?;

    let mut classes = Vec::<String>::new();
    for selector in parsed.0.iter() {
        collect_lightning_selector_classes(selector, &mut classes);
    }
    classes.sort_unstable();
    classes.dedup();
    Some(classes)
}

fn collect_lightning_selector_classes(selector: &LightningSelector<'_>, classes: &mut Vec<String>) {
    for component in selector.iter_raw_match_order() {
        match component {
            LightningComponent::Class(name) => classes.push(name.0.to_string()),
            LightningComponent::Is(selectors)
            | LightningComponent::Where(selectors)
            | LightningComponent::Has(selectors)
            | LightningComponent::Any(_, selectors)
            | LightningComponent::Negation(selectors) => {
                for selector in selectors.iter() {
                    collect_lightning_selector_classes(selector, classes);
                }
            }
            LightningComponent::NthOf(nth_of) => {
                for selector in nth_of.selectors().iter() {
                    collect_lightning_selector_classes(selector, classes);
                }
            }
            LightningComponent::Slotted(selector) => {
                collect_lightning_selector_classes(selector, classes);
            }
            LightningComponent::Host(Some(selector)) => {
                collect_lightning_selector_classes(selector, classes);
            }
            _ => {}
        }
    }
}

fn extract_selector_id_names(selector: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = selector.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'#' {
            index += 1;
            continue;
        }
        index += 1;
        let (decoded, next_index) = read_css_identifier(selector, index);
        if !decoded.is_empty() {
            ids.push(decoded);
        }
        index = next_index.max(index);
    }
    ids
}

fn read_css_identifier(value: &str, start: usize) -> (String, usize) {
    let bytes = value.as_bytes();
    let mut index = start;
    let mut out = String::new();

    while index < bytes.len() {
        let ch = bytes[index] as char;
        if ch == '\\' {
            let (decoded, next_index) = decode_css_escape(value, index);
            out.push(decoded);
            if next_index <= index {
                index += 1;
            } else {
                index = next_index;
            }
            continue;
        }

        if ch.is_ascii_whitespace()
            || matches!(ch, '.' | '#' | ':' | '[' | ']' | '>' | '+' | '~' | ',')
        {
            break;
        }

        out.push(ch);
        index += 1;
    }

    (out, index)
}

fn decode_css_escape(value: &str, index: usize) -> (char, usize) {
    let bytes = value.as_bytes();
    if bytes.get(index).copied() != Some(b'\\') {
        return ('\\', index.saturating_add(1));
    }

    let mut cursor = index + 1;
    let mut hex = String::new();
    while cursor < bytes.len() && hex.len() < 6 {
        let ch = bytes[cursor] as char;
        if ch.is_ascii_hexdigit() {
            hex.push(ch);
            cursor += 1;
        } else {
            break;
        }
    }

    if !hex.is_empty() {
        if cursor < bytes.len() {
            let ch = bytes[cursor] as char;
            if ch.is_ascii_whitespace() {
                cursor += 1;
                if ch == '\r' && cursor < bytes.len() && bytes[cursor] == b'\n' {
                    cursor += 1;
                }
            }
        }
        let codepoint = u32::from_str_radix(&hex, 16).unwrap_or(0xfffd);
        let decoded = char::from_u32(codepoint).unwrap_or('\u{fffd}');
        return (decoded, cursor);
    }

    if cursor < bytes.len() {
        let decoded = bytes[cursor] as char;
        return (decoded, cursor + 1);
    }

    ('\\', cursor)
}

fn extract_selector_tag_names(selector: &str) -> Vec<String> {
    let marked = mark_global_selector_regions(selector);
    let mut tags = Vec::new();
    for (_, part) in split_selector_parts(&marked) {
        let clean = part
            .replace(GLOBAL_OPEN_MARKER, "")
            .replace(GLOBAL_CLOSE_MARKER, "")
            .trim()
            .to_string();
        if clean.is_empty() || clean.starts_with(':') || clean.starts_with('.') || clean == "*" {
            continue;
        }
        if clean.starts_with('#') || clean.starts_with('[') || clean.starts_with('&') {
            continue;
        }

        let mut end = 0usize;
        let bytes = clean.as_bytes();
        while end < bytes.len() {
            let ch = bytes[end] as char;
            if ch.is_ascii_alphanumeric() || ch == '-' {
                end += 1;
            } else {
                break;
            }
        }
        if end > 0 {
            let tag = clean.get(0..end).unwrap_or_default().to_ascii_lowercase();
            if is_css_identifier_word(&tag) {
                tags.push(tag);
            }
        }
    }
    tags
}

fn is_css_identifier_word(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '-') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn escape_css_comment_body(value: &str) -> String {
    value.replace("*/", "*\\/")
}

fn map_keyframe_name(name: &str, hash: &str) -> Option<(String, String)> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(global) = trimmed.strip_prefix("-global-") {
        return Some((global.to_string(), global.to_string()));
    }
    let mapped = format!("{hash}-{trimmed}");
    Some((mapped.clone(), mapped))
}

fn replace_first_css_identifier(raw: &str, replacement: &str) -> String {
    let mut start = None;
    let mut end = None;
    let mut idx = 0usize;
    let bytes = raw.as_bytes();
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            start = Some(idx);
            let mut cursor = idx;
            while cursor < bytes.len() {
                let next = bytes[cursor] as char;
                if next.is_ascii_alphanumeric() || next == '-' || next == '_' {
                    cursor += 1;
                } else {
                    break;
                }
            }
            end = Some(cursor);
            break;
        }
        idx += 1;
    }

    let (Some(start), Some(end)) = (start, end) else {
        return raw.to_string();
    };

    let mut out = String::new();
    out.push_str(&raw[..start]);
    out.push_str(replacement);
    out.push_str(&raw[end..]);
    out
}

const GLOBAL_OPEN_MARKER: &str = "__SVELTE_GLOBAL_OPEN__";
const GLOBAL_CLOSE_MARKER: &str = "__SVELTE_GLOBAL_CLOSE__";

fn scope_selector_list_text_with_mode(
    prelude: &str,
    hash: &str,
    prefer_where: bool,
    skip_simple_heads_when_prefer_where: bool,
) -> String {
    let mut out = String::new();
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut segment_start = 0usize;

    for (idx, ch) in prelude.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            ',' if depth_paren == 0 && depth_bracket == 0 => {
                let segment = prelude.get(segment_start..idx).unwrap_or_default();
                out.push_str(&scope_selector_segment_text(
                    segment,
                    hash,
                    prefer_where,
                    skip_simple_heads_when_prefer_where,
                ));
                out.push(',');
                let mut ws_end = idx + 1;
                while ws_end < prelude.len()
                    && prelude
                        .as_bytes()
                        .get(ws_end)
                        .copied()
                        .is_some_and(|ch| (ch as char).is_ascii_whitespace())
                {
                    ws_end += 1;
                }
                out.push_str(prelude.get(idx + 1..ws_end).unwrap_or_default());
                segment_start = ws_end;
            }
            _ => {}
        }
    }

    let trailing = prelude.get(segment_start..).unwrap_or_default();
    out.push_str(&scope_selector_segment_text(
        trailing,
        hash,
        prefer_where,
        skip_simple_heads_when_prefer_where,
    ));
    out
}

fn scope_selector_segment_text(
    segment: &str,
    hash: &str,
    prefer_where: bool,
    skip_simple_heads_when_prefer_where: bool,
) -> String {
    let (without_comments, comments) = mask_selector_comments(segment);
    let marked = mark_global_selector_regions(&without_comments);
    let parts = split_selector_parts(&marked);
    if parts.is_empty() {
        let out = marked
            .replace(GLOBAL_OPEN_MARKER, "")
            .replace(GLOBAL_CLOSE_MARKER, "");
        return unmask_selector_comments(out, &comments);
    }

    let mut out = String::new();
    let mut scoped_count = 0usize;
    let mut in_global = false;

    for (combinator, part) in parts {
        let combinator = combinator
            .replace(GLOBAL_OPEN_MARKER, "")
            .replace(GLOBAL_CLOSE_MARKER, "");

        let opens = part.matches(GLOBAL_OPEN_MARKER).count();
        let closes = part.matches(GLOBAL_CLOSE_MARKER).count();
        let trimmed_part = part.trim();
        let global_prefixed = trimmed_part.starts_with(GLOBAL_OPEN_MARKER);
        let global_only_wrapped = opens > 0
            && trimmed_part.starts_with(GLOBAL_OPEN_MARKER)
            && trimmed_part.ends_with(GLOBAL_CLOSE_MARKER);
        let mut clean = part
            .replace(GLOBAL_OPEN_MARKER, "")
            .replace(GLOBAL_CLOSE_MARKER, "");
        let clean_trimmed = clean.trim();
        let global_functional_prefixed = clean_trimmed.starts_with(":global:");
        let combinator_has_symbol = combinator.contains('>')
            || combinator.contains('+')
            || combinator.contains('~')
            || combinator.contains("||");
        let attach_global = clean_trimmed == ":global"
            || clean_trimmed.starts_with(":global:")
            || clean_trimmed.starts_with(":global.")
            || clean_trimmed.starts_with(":global#")
            || clean_trimmed.starts_with(":global[");
        if !attach_global || combinator_has_symbol {
            out.push_str(&combinator);
        }

        if clean_trimmed == ":global" {
            in_global = true;
            continue;
        }

        let mut is_global = in_global || global_only_wrapped || global_prefixed;
        if clean.contains(":global") {
            is_global = true;
            if clean_trimmed.starts_with(":global.") && combinator.is_empty() {
                clean = clean.replacen(":global", "&", 1);
            } else {
                clean = clean.replace(":global", "");
            }
        }

        if !is_global
            && is_scope_eligible_selector_part(
                &clean,
                hash,
                prefer_where,
                skip_simple_heads_when_prefer_where,
            )
        {
            let token = if scoped_count == 0 && !prefer_where {
                format!(".{hash}")
            } else {
                format!(":where(.{hash})")
            };
            let scoped = insert_scope_token_into_part_with_markers(&part, &token);
            let scoped =
                scope_pseudo_selector_arguments_with_context(&scoped, hash, scoped_count > 0);
            let scoped = scoped
                .replace(GLOBAL_OPEN_MARKER, "")
                .replace(GLOBAL_CLOSE_MARKER, "");
            out.push_str(&remove_bare_global_tokens(&scoped));
            scoped_count += 1;
        } else {
            let mut clean = if is_global {
                let global_clean = clean.clone();
                if global_functional_prefixed || global_only_wrapped {
                    global_clean
                } else {
                    scope_pseudo_selector_arguments_with_context(
                        &global_clean,
                        hash,
                        scoped_count > 0,
                    )
                }
            } else {
                scope_pseudo_selector_arguments_with_context(&part, hash, scoped_count > 0)
                    .replace(GLOBAL_OPEN_MARKER, "")
                    .replace(GLOBAL_CLOSE_MARKER, "")
            };

            if !is_global {
                let trimmed_start = clean.trim_start();
                if trimmed_start.starts_with(":has(") {
                    let leading = clean.len().saturating_sub(trimmed_start.len());
                    let token = format!(".{hash}");
                    clean = format!(
                        "{}{}{}",
                        clean.get(..leading).unwrap_or_default(),
                        token,
                        clean.get(leading..).unwrap_or_default()
                    );
                }
            }

            out.push_str(&remove_bare_global_tokens(&clean));
        }

        if opens > closes {
            in_global = true;
        } else if closes > 0 {
            in_global = false;
        }
    }

    unmask_selector_comments(out, &comments)
}

fn mark_global_selector_regions(selector: &str) -> String {
    let mut out = String::new();
    let mut search_from = 0usize;

    while let Some(rel) = selector
        .get(search_from..)
        .and_then(|tail| tail.find(":global("))
    {
        let start = search_from + rel;
        out.push_str(selector.get(search_from..start).unwrap_or_default());

        let open = start + ":global".len();
        let Some(close) = find_matching_paren(selector, open) else {
            out.push_str(selector.get(start..).unwrap_or_default());
            return out;
        };

        let inner = selector.get(open + 1..close).unwrap_or_default();
        out.push_str(GLOBAL_OPEN_MARKER);
        out.push_str(inner);
        out.push_str(GLOBAL_CLOSE_MARKER);

        search_from = close + 1;
    }

    out.push_str(selector.get(search_from..).unwrap_or_default());
    out
}

fn split_selector_parts(selector: &str) -> Vec<(String, String)> {
    let mut parts = Vec::new();
    let mut index = 0usize;
    let bytes = selector.as_bytes();

    while index < bytes.len() {
        let comb_start = index;
        while index < bytes.len() && (bytes[index] as char).is_ascii_whitespace() {
            index += 1;
        }
        if index + 1 < bytes.len() && selector.get(index..index + 2) == Some("||") {
            index += 2;
            while index < bytes.len() && (bytes[index] as char).is_ascii_whitespace() {
                index += 1;
            }
        } else if index < bytes.len() && matches!(bytes[index] as char, '>' | '+' | '~') {
            index += 1;
            while index < bytes.len() && (bytes[index] as char).is_ascii_whitespace() {
                index += 1;
            }
        }
        let combinator = selector
            .get(comb_start..index)
            .unwrap_or_default()
            .to_string();

        let part_start = index;
        let mut depth_paren = 0usize;
        let mut depth_bracket = 0usize;
        let mut in_single = false;
        let mut in_double = false;

        while index < bytes.len() {
            if selector
                .get(index..)
                .is_some_and(|tail| tail.starts_with(GLOBAL_OPEN_MARKER))
            {
                index += GLOBAL_OPEN_MARKER.len();
                continue;
            }
            if selector
                .get(index..)
                .is_some_and(|tail| tail.starts_with(GLOBAL_CLOSE_MARKER))
            {
                index += GLOBAL_CLOSE_MARKER.len();
                continue;
            }

            let ch = bytes[index] as char;
            if ch == '\\' {
                index = consume_css_escape(selector, index);
                continue;
            }
            if in_single {
                if ch == '\'' {
                    in_single = false;
                }
                index += 1;
                continue;
            }
            if in_double {
                if ch == '"' {
                    in_double = false;
                }
                index += 1;
                continue;
            }

            match ch {
                '\'' => {
                    in_single = true;
                    index += 1;
                }
                '"' => {
                    in_double = true;
                    index += 1;
                }
                '(' => {
                    depth_paren += 1;
                    index += 1;
                }
                ')' => {
                    depth_paren = depth_paren.saturating_sub(1);
                    index += 1;
                }
                '[' => {
                    depth_bracket += 1;
                    index += 1;
                }
                ']' => {
                    depth_bracket = depth_bracket.saturating_sub(1);
                    index += 1;
                }
                _ if depth_paren == 0
                    && depth_bracket == 0
                    && ((ch.is_ascii_whitespace())
                        || matches!(ch, '>' | '+' | '~')
                        || (index + 1 < bytes.len()
                            && selector.get(index..index + 2) == Some("||"))) =>
                {
                    break;
                }
                _ => index += 1,
            }
        }

        let part = selector
            .get(part_start..index)
            .unwrap_or_default()
            .to_string();
        if !part.is_empty() || !combinator.is_empty() {
            parts.push((combinator, part));
        }
    }

    parts
}

fn consume_css_escape(value: &str, index: usize) -> usize {
    let bytes = value.as_bytes();
    if bytes.get(index).copied() != Some(b'\\') {
        return index.saturating_add(1);
    }

    let mut cursor = index + 1;
    let mut hex_count = 0usize;
    while cursor < bytes.len() && hex_count < 6 {
        let ch = bytes[cursor] as char;
        if ch.is_ascii_hexdigit() {
            cursor += 1;
            hex_count += 1;
        } else {
            break;
        }
    }

    if hex_count > 0 {
        if cursor < bytes.len() {
            let ch = bytes[cursor] as char;
            if ch.is_ascii_whitespace() {
                cursor += 1;
                if ch == '\r' && cursor < bytes.len() && bytes[cursor] == b'\n' {
                    cursor += 1;
                }
            }
        }
        return cursor;
    }

    (index + 2).min(bytes.len())
}

fn is_scope_eligible_selector_part(
    part: &str,
    hash: &str,
    prefer_where: bool,
    skip_simple_heads_when_prefer_where: bool,
) -> bool {
    let trimmed = part.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with("__SVELTE_COMMENT_") {
        return false;
    }
    if selector_contains_top_level_ampersand(trimmed) {
        return false;
    }
    if trimmed.contains(":root") {
        return false;
    }
    if trimmed.starts_with(":host") {
        return false;
    }
    if trimmed.starts_with("::view-transition") {
        return false;
    }
    if prefer_where
        && skip_simple_heads_when_prefer_where
        && (trimmed.starts_with('.') || trimmed.starts_with('#') || trimmed.starts_with('['))
    {
        return false;
    }
    if trimmed.starts_with(":is(") || trimmed.starts_with(":has(") || trimmed.starts_with(":where(")
    {
        return false;
    }
    if trimmed.contains(&format!(".{hash}")) || trimmed.contains(&format!(":where(.{hash})")) {
        return false;
    }
    true
}

fn mask_selector_comments(selector: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut comments = Vec::new();
    let mut search_from = 0usize;

    while let Some(rel) = selector.get(search_from..).and_then(|tail| tail.find("/*")) {
        let start = search_from + rel;
        out.push_str(selector.get(search_from..start).unwrap_or_default());
        let Some(close_rel) = selector.get(start + 2..).and_then(|tail| tail.find("*/")) else {
            out.push_str(selector.get(start..).unwrap_or_default());
            return (out, comments);
        };
        let end = start + 2 + close_rel + 2;
        let comment = selector.get(start..end).unwrap_or_default().to_string();
        let marker = format!("__SVELTE_COMMENT_{}__", comments.len());
        comments.push(comment);
        out.push_str(&marker);
        search_from = end;
    }

    out.push_str(selector.get(search_from..).unwrap_or_default());
    (out, comments)
}

fn selector_contains_top_level_ampersand(selector: &str) -> bool {
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in selector.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            '&' if depth_paren == 0 && depth_bracket == 0 => return true,
            _ => {}
        }
    }

    false
}

fn unmask_selector_comments(mut selector: String, comments: &[String]) -> String {
    for (index, comment) in comments.iter().enumerate() {
        let marker = format!("__SVELTE_COMMENT_{index}__");
        selector = selector.replace(&marker, comment);
    }
    selector
}

fn insert_scope_token_into_part(part: &str, token: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum TokenKind {
        Simple,
        Pseudo,
        Nesting,
    }

    fn consume_identifier_like(value: &str, mut index: usize) -> usize {
        while index < value.len() {
            let rest = value.get(index..).unwrap_or_default();
            if rest.starts_with('\\') {
                index = consume_css_escape(value, index);
                continue;
            }

            let ch = value.as_bytes()[index] as char;
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') || ch as u32 >= 0x80 {
                index += 1;
                continue;
            }
            break;
        }

        index
    }

    fn consume_attribute_selector(value: &str, mut index: usize) -> usize {
        let mut depth = 0usize;
        let mut in_single = false;
        let mut in_double = false;

        while index < value.len() {
            let ch = value.as_bytes()[index] as char;
            if ch == '\\' {
                index = consume_css_escape(value, index);
                continue;
            }
            if in_single {
                if ch == '\'' {
                    in_single = false;
                }
                index += 1;
                continue;
            }
            if in_double {
                if ch == '"' {
                    in_double = false;
                }
                index += 1;
                continue;
            }

            match ch {
                '\'' => {
                    in_single = true;
                    index += 1;
                }
                '"' => {
                    in_double = true;
                    index += 1;
                }
                '[' => {
                    depth += 1;
                    index += 1;
                }
                ']' => {
                    depth = depth.saturating_sub(1);
                    index += 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => index += 1,
            }
        }

        index
    }

    let trimmed = part.trim();
    if trimmed == "*" {
        return token.to_string();
    }

    if part.starts_with("#\\")
        && !part.contains(':')
        && !part.contains('.')
        && !part.contains('[')
        && !part.contains(' ')
        && !part.contains('\t')
        && token.starts_with('.')
    {
        return format!("{part} {token}");
    }

    let mut tokens = Vec::<(usize, usize, TokenKind)>::new();
    let mut index = 0usize;
    while index < part.len() {
        let ch = part.as_bytes()[index] as char;
        let (end, kind) = match ch {
            '\\' => (consume_css_escape(part, index), TokenKind::Simple),
            '*' => (index + 1, TokenKind::Simple),
            '&' => (index + 1, TokenKind::Nesting),
            '.' | '#' => (consume_identifier_like(part, index + 1), TokenKind::Simple),
            '[' => (consume_attribute_selector(part, index), TokenKind::Simple),
            ':' => {
                let mut cursor = index + 1;
                if cursor < part.len() && part.as_bytes()[cursor] == b':' {
                    cursor += 1;
                }
                cursor = consume_identifier_like(part, cursor);
                if cursor < part.len()
                    && part.as_bytes()[cursor] == b'('
                    && let Some(close) = find_matching_paren(part, cursor)
                {
                    cursor = close + 1;
                }
                (cursor, TokenKind::Pseudo)
            }
            c if c.is_ascii_alphanumeric() || c == '_' || c == '-' || (c as u32) >= 0x80 => {
                (consume_identifier_like(part, index), TokenKind::Simple)
            }
            _ => (next_char_boundary(part, index), TokenKind::Simple),
        };

        tokens.push((index, end, kind));
        index = end.max(index + 1);
    }

    if let Some(&(start, end, _)) = tokens
        .iter()
        .rev()
        .find(|(_, _, kind)| matches!(kind, TokenKind::Simple))
    {
        if part.get(start..end) == Some("*") {
            let mut out = String::new();
            out.push_str(part.get(..start).unwrap_or_default());
            out.push_str(token);
            out.push_str(part.get(end..).unwrap_or_default());
            return out;
        }

        let mut out = String::new();
        out.push_str(part.get(..end).unwrap_or_default());
        out.push_str(token);
        out.push_str(part.get(end..).unwrap_or_default());
        return out;
    }

    if let Some(&(start, _, _)) = tokens
        .iter()
        .find(|(_, _, kind)| matches!(kind, TokenKind::Pseudo))
    {
        let mut out = String::new();
        out.push_str(part.get(..start).unwrap_or_default());
        out.push_str(token);
        out.push_str(part.get(start..).unwrap_or_default());
        return out;
    }

    format!("{part}{token}")
}

fn insert_scope_token_into_part_with_markers(part: &str, token: &str) -> String {
    if let Some(marker_index) = part.find(GLOBAL_OPEN_MARKER) {
        let close_index = part
            .find(GLOBAL_CLOSE_MARKER)
            .map(|idx| idx + GLOBAL_CLOSE_MARKER.len())
            .unwrap_or(marker_index);
        let trailing = part.get(close_index..).unwrap_or_default().trim();

        if marker_index > 0 && trailing.is_empty() {
            let mut out = String::new();
            out.push_str(part.get(..marker_index).unwrap_or_default());
            out.push_str(token);
            out.push_str(part.get(marker_index..).unwrap_or_default());
            return out;
        }
        return insert_scope_token_into_part(part, token);
    }
    insert_scope_token_into_part(part, token)
}

fn scope_pseudo_selector_arguments_with_context(
    selector: &str,
    hash: &str,
    scoped_before: bool,
) -> String {
    let mut out = String::new();
    let mut index = 0usize;
    while index < selector.len() {
        let rest = selector.get(index..).unwrap_or_default();
        let mut matched = None::<(&'static str, usize, usize)>;
        for pseudo in [":is(", ":where(", ":has(", ":not("] {
            if let Some(rel) = rest.find(pseudo) {
                let abs = index + rel;
                let choose = matched
                    .as_ref()
                    .is_none_or(|(_, current_abs, _)| abs < *current_abs);
                if choose {
                    matched = Some((pseudo, abs, abs + pseudo.len() - 1));
                }
            }
        }

        let Some((pseudo_name, pseudo_start, open_index)) = matched else {
            out.push_str(rest);
            break;
        };

        out.push_str(selector.get(index..pseudo_start).unwrap_or_default());
        let close_index =
            find_matching_paren(selector, open_index).unwrap_or(selector.len().saturating_sub(1));
        let inner = selector
            .get(open_index + 1..close_index)
            .unwrap_or_default();
        let prefix = selector.get(..pseudo_start).unwrap_or_default();
        let has_outer_scope = scoped_before
            || prefix.contains(&format!(".{hash}"))
            || prefix.contains(&format!(":where(.{hash})"));
        let scoped_inner = if pseudo_name == ":not(" {
            if let Some(unwrapped_global) = unwrap_pure_global_selector_option(inner) {
                unwrapped_global.to_string()
            } else if not_argument_should_be_scoped(inner) {
                scope_selector_list_preserving_global_options(inner, hash, has_outer_scope, true)
            } else {
                inner.to_string()
            }
        } else if pseudo_name == ":has(" {
            let has_nesting_prefix = prefix.trim_end().ends_with('&');
            let prefix_has_local_tags = !extract_selector_tag_names(&remove_bare_global_tokens(
                &normalize_selector_for_usage(prefix),
            ))
            .is_empty();
            let has_scope_like_prefix = has_outer_scope
                || selector.trim_start().starts_with(":has(")
                || prefix.contains(":root")
                || prefix_has_local_tags;
            scope_selector_list_preserving_global_options(
                inner,
                hash,
                has_scope_like_prefix && !has_nesting_prefix,
                true,
            )
        } else {
            scope_selector_list_preserving_global_options(inner, hash, has_outer_scope, true)
        };
        out.push_str(selector.get(pseudo_start..open_index).unwrap_or_default());
        out.push('(');
        out.push_str(&scoped_inner);
        out.push(')');
        index = close_index.saturating_add(1);
    }
    out
}

fn scope_selector_list_preserving_global_options(
    selector_list: &str,
    hash: &str,
    prefer_where: bool,
    skip_simple_heads_when_prefer_where: bool,
) -> String {
    let ranges = split_selectors_top_level_ranges(selector_list);
    if ranges.len() <= 1 {
        return scope_selector_option_preserving_global(
            selector_list,
            hash,
            prefer_where,
            skip_simple_heads_when_prefer_where,
        );
    }

    let mut out = String::new();
    for (index, (start, end)) in ranges.iter().copied().enumerate() {
        if index > 0 {
            let previous_end = ranges[index - 1].1;
            out.push_str(selector_list.get(previous_end..start).unwrap_or_default());
        }
        out.push_str(&scope_selector_option_preserving_global(
            selector_list.get(start..end).unwrap_or_default(),
            hash,
            prefer_where,
            skip_simple_heads_when_prefer_where,
        ));
    }

    out
}

fn scope_selector_option_preserving_global(
    selector: &str,
    hash: &str,
    prefer_where: bool,
    skip_simple_heads_when_prefer_where: bool,
) -> String {
    let trimmed = selector.trim();
    if let Some(unwrapped) = unwrap_pure_global_selector_option(trimmed) {
        let leading_len = selector.len().saturating_sub(selector.trim_start().len());
        let trailing_start = selector.trim_end().len();
        let mut out = String::new();
        out.push_str(selector.get(..leading_len).unwrap_or_default());
        out.push_str(unwrapped);
        out.push_str(selector.get(trailing_start..).unwrap_or_default());
        return out;
    }

    scope_selector_list_text_with_mode(
        selector,
        hash,
        prefer_where,
        skip_simple_heads_when_prefer_where,
    )
}

fn unwrap_pure_global_selector_option(selector: &str) -> Option<&str> {
    let trimmed = selector.trim();
    if trimmed.starts_with(GLOBAL_OPEN_MARKER) && trimmed.ends_with(GLOBAL_CLOSE_MARKER) {
        return trimmed.get(
            GLOBAL_OPEN_MARKER.len()..trimmed.len().saturating_sub(GLOBAL_CLOSE_MARKER.len()),
        );
    }
    if !trimmed.starts_with(":global(") {
        return None;
    }

    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut close = None;

    for (index, ch) in trimmed.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }

    let close = close?;
    if close + 1 != trimmed.len() {
        return None;
    }

    trimmed.get(":global(".len()..close)
}

fn not_argument_should_be_scoped(inner: &str) -> bool {
    let mut depth_paren = 0usize;
    let mut depth_bracket = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in inner.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if ch == '"' {
                in_double = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth_paren += 1,
            ')' => depth_paren = depth_paren.saturating_sub(1),
            '[' => depth_bracket += 1,
            ']' => depth_bracket = depth_bracket.saturating_sub(1),
            '>' | '+' | '~' if depth_paren == 0 && depth_bracket == 0 => return true,
            c if c.is_ascii_whitespace() && depth_paren == 0 && depth_bracket == 0 => {
                return inner.split_whitespace().count() > 1;
            }
            _ => {}
        }
    }

    false
}

fn remove_bare_global_tokens(selector: &str) -> String {
    let mut out = String::new();
    let mut index = 0usize;

    while index < selector.len() {
        if selector
            .get(index..)
            .is_some_and(|tail| tail.starts_with(":global"))
        {
            let next = selector.as_bytes().get(index + ":global".len()).copied();
            if next != Some(b'(') {
                index += ":global".len();
                continue;
            }
        }
        let next = next_char_boundary(selector, index);
        out.push_str(selector.get(index..next).unwrap_or_default());
        index = next;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{
        GLOBAL_CLOSE_MARKER, GLOBAL_OPEN_MARKER, scope_pseudo_selector_arguments_with_context,
        scope_selector_segment_text, unwrap_pure_global_selector_option,
    };

    #[test]
    fn unwraps_pure_global_selector_option() {
        assert_eq!(
            unwrap_pure_global_selector_option(":global(p span)"),
            Some("p span")
        );
    }

    #[test]
    fn preserves_global_not_arguments() {
        assert_eq!(
            scope_pseudo_selector_arguments_with_context(
                "span.svelte-xyz:not(:global(p span))",
                "svelte-xyz",
                true,
            ),
            "span.svelte-xyz:not(p span)"
        );
    }

    #[test]
    fn preserves_marker_wrapped_global_not_arguments() {
        let selector =
            format!("span.svelte-xyz:not({GLOBAL_OPEN_MARKER}p span{GLOBAL_CLOSE_MARKER})");
        assert_eq!(
            scope_pseudo_selector_arguments_with_context(&selector, "svelte-xyz", false),
            "span.svelte-xyz:not(p span)"
        );
    }

    #[test]
    fn scopes_segment_with_global_not_argument() {
        assert_eq!(
            scope_selector_segment_text("span:not(:global(p span))", "svelte-xyz", false, false),
            "span.svelte-xyz:not(p span)"
        );
    }
}
