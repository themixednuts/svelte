//! CSS selector/keyframe rewrites and output generation.

use std::collections::BTreeMap;

use camino::Utf8Path;

use crate::ast::modern::{CssBlockChild, CssNode};
use crate::compiler::phases::transform::output::OutputContext;
use crate::compiler::phases::transform::sourcemap::SparseMappingHint;

use super::scoping;
use super::usage::build_css_usage_context;
use super::{GeneratedCssOutput, TextReplacement};

pub(crate) fn generate_component_css_output(
    ctx: OutputContext<'_>,
    root: &crate::ast::modern::Root,
    css_hash: Option<&str>,
    dev: bool,
    output_filename: Option<&Utf8Path>,
) -> Option<GeneratedCssOutput> {
    let source = ctx.source.text;
    let css_ctx = ctx.with_output_filename(output_filename);
    let include_map = ctx.include_map(output_filename);
    let (_, _, content_start, content_end) =
        crate::compiler::phases::parse::style_block_ranges(root)
            .into_iter()
            .next()?;
    let css_source = source.get(content_start..content_end)?;
    let Some(hash) = css_hash else {
        let map =
            include_map.then(|| css_ctx.build_sparse_sourcemap(css_source, "input.svelte", vec![]));
        return Some(GeneratedCssOutput {
            code: css_source.to_string(),
            map,
        });
    };
    let children =
        crate::compiler::phases::parse::parse_modern_css_nodes(source, content_start, content_end);

    let mut keyframes = BTreeMap::<String, String>::new();
    let mut replacements = Vec::new();
    let usage = build_css_usage_context(root, dev);

    let mut rewrite_ctx =
        scoping::RewriteContext::new(source, hash, &mut keyframes, &mut replacements, &usage);
    collect_css_selector_and_keyframe_rewrites(&children, &mut rewrite_ctx);
    collect_css_animation_value_rewrites(source, &children, &keyframes, &mut replacements);

    if replacements.is_empty() {
        let code = source.get(content_start..content_end)?.to_string();
        let map =
            include_map.then(|| css_ctx.build_sparse_sourcemap(&code, "input.svelte", vec![]));
        return Some(GeneratedCssOutput { code, map });
    }

    let mapping_hint_pairs = replacements
        .iter()
        .filter_map(|replacement| {
            let original = source.get(replacement.start..replacement.end)?;
            Some((original.to_string(), replacement.text.clone()))
        })
        .collect::<Vec<_>>();

    replacements.sort_by(|left, right| {
        right
            .start
            .cmp(&left.start)
            .then_with(|| right.end.cmp(&left.end))
    });

    let mut output = source.get(content_start..content_end)?.to_string();
    let mut min_applied_start = usize::MAX;

    for replacement in replacements {
        if replacement.start < content_start || replacement.end > content_end {
            continue;
        }
        if replacement.end > min_applied_start {
            continue;
        }
        let rel_start = replacement.start - content_start;
        let rel_end = replacement.end - content_start;
        if rel_start > rel_end || rel_end > output.len() {
            continue;
        }
        output.replace_range(rel_start..rel_end, &replacement.text);
        min_applied_start = replacement.start;
    }

    output = compact_escaped_id_scope_spacing(&output, hash);

    let map = include_map.then(|| {
        let mut hints = mapping_hint_pairs
            .iter()
            .map(|(original, generated)| SparseMappingHint {
                original: original.as_str(),
                generated: generated.as_str(),
                name: None,
            })
            .collect::<Vec<_>>();
        hints.extend(collect_simple_css_scope_hints(css_source, hash));
        css_ctx.build_sparse_sourcemap(&output, "input.svelte", hints)
    });

    Some(GeneratedCssOutput { code: output, map })
}

fn collect_simple_css_scope_hints<'a>(
    css_source: &'a str,
    hash: &'a str,
) -> Vec<SparseMappingHint<'a>> {
    let mut hints = Vec::new();

    for line in css_source.lines() {
        let trimmed = line.trim();
        let Some(selector) = trimmed.strip_suffix('{').map(str::trim) else {
            continue;
        };
        if selector.is_empty()
            || selector.starts_with('@')
            || selector.contains(' ')
            || selector.contains('>')
            || selector.contains('+')
            || selector.contains('~')
            || selector.contains(':')
            || selector.contains('[')
            || selector.contains(',')
        {
            continue;
        }

        hints.push(SparseMappingHint {
            original: selector,
            generated: Box::leak(format!("{selector}.{hash}").into_boxed_str()),
            name: None,
        });
    }

    hints
}

pub(crate) fn compact_escaped_id_scope_spacing(css: &str, hash: &str) -> String {
    let needle = format!(" .{hash} {{");
    let replacement = format!(" .{hash}{{");

    let mut out = css.to_string();
    let mut search_from = 0usize;
    while let Some(rel) = out.get(search_from..).and_then(|tail| tail.find(&needle)) {
        let absolute = search_from + rel;
        let line_start = out
            .get(..absolute)
            .and_then(|prefix| prefix.rfind('\n').map(|index| index + 1))
            .unwrap_or(0);
        let left = out.get(line_start..absolute).unwrap_or_default().trim_end();
        let left_trimmed = left.trim_start();

        let escaped_id_only = left_trimmed.starts_with("#\\")
            && !left_trimmed.contains(' ')
            && !left_trimmed.contains('>')
            && !left_trimmed.contains('+')
            && !left_trimmed.contains('~')
            && !left_trimmed.contains('.')
            && !left_trimmed.contains(':')
            && !left_trimmed.contains('[');

        if escaped_id_only {
            let end = absolute + needle.len();
            out.replace_range(absolute..end, &replacement);
            search_from = absolute + replacement.len();
        } else {
            search_from = absolute + needle.len();
        }
    }

    out
}

pub(crate) fn collect_css_selector_and_keyframe_rewrites(
    nodes: &[CssNode],
    ctx: &mut scoping::RewriteContext<'_>,
) {
    scoping::collect_css_selector_and_keyframe_rewrites(nodes, ctx, scoping::RewriteFrame::root());
}

pub(crate) fn collect_css_animation_value_rewrites(
    source: &str,
    nodes: &[CssNode],
    keyframes: &BTreeMap<String, String>,
    replacements: &mut Vec<TextReplacement>,
) {
    for node in nodes {
        match node {
            CssNode::Rule(rule) => {
                collect_css_block_animation_value_rewrites(
                    source,
                    &rule.block,
                    keyframes,
                    replacements,
                );
            }
            CssNode::Atrule(atrule) => {
                if let Some(block) = &atrule.block {
                    collect_css_block_animation_value_rewrites(
                        source,
                        block,
                        keyframes,
                        replacements,
                    );
                }
            }
        }
    }
}

fn collect_css_block_animation_value_rewrites(
    source: &str,
    block: &crate::ast::modern::CssBlock,
    keyframes: &BTreeMap<String, String>,
    replacements: &mut Vec<TextReplacement>,
) {
    for child in block.children.iter() {
        match child {
            CssBlockChild::Declaration(declaration) => {
                let property = declaration.property.trim();
                if !(property == "animation"
                    || property == "animation-name"
                    || property.ends_with("-animation")
                    || property.ends_with("-animation-name"))
                {
                    continue;
                }

                let Some((value_start, value_end)) = declaration_value_span(source, declaration)
                else {
                    continue;
                };
                let Some(value) = source.get(value_start..value_end) else {
                    continue;
                };
                let rewritten = rewrite_animation_value(value, keyframes);
                if rewritten != value {
                    replacements.push(TextReplacement {
                        start: value_start,
                        end: value_end,
                        text: rewritten,
                    });
                }
            }
            CssBlockChild::Rule(rule) => {
                collect_css_block_animation_value_rewrites(
                    source,
                    &rule.block,
                    keyframes,
                    replacements,
                );
            }
            CssBlockChild::Atrule(atrule) => {
                if let Some(block) = &atrule.block {
                    collect_css_block_animation_value_rewrites(
                        source,
                        block,
                        keyframes,
                        replacements,
                    );
                }
            }
        }
    }
}

fn declaration_value_span(
    source: &str,
    declaration: &crate::ast::modern::CssDeclaration,
) -> Option<(usize, usize)> {
    let raw = source.get(declaration.start..declaration.end)?;
    let colon = raw.find(':')?;
    let mut value_start = declaration.start + colon + 1;
    while value_start < declaration.end
        && source
            .as_bytes()
            .get(value_start)
            .copied()
            .is_some_and(|ch| (ch as char).is_ascii_whitespace())
    {
        value_start += 1;
    }

    let mut value_end = declaration.end;
    while value_end > value_start {
        let ch = source.as_bytes()[value_end - 1] as char;
        if ch.is_ascii_whitespace() || ch == ';' {
            value_end -= 1;
        } else {
            break;
        }
    }

    (value_start <= value_end).then_some((value_start, value_end))
}

fn rewrite_animation_value(value: &str, keyframes: &BTreeMap<String, String>) -> String {
    if keyframes.is_empty() {
        return value.to_string();
    }

    let mut out = String::new();
    let bytes = value.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let ch = bytes[index] as char;
        if ch.is_ascii_alphabetic() || ch == '_' || ch == '-' {
            let start = index;
            index += 1;
            while index < bytes.len() {
                let next = bytes[index] as char;
                if next.is_ascii_alphanumeric() || next == '_' || next == '-' {
                    index += 1;
                } else {
                    break;
                }
            }
            let token = value.get(start..index).unwrap_or_default();
            if let Some(replacement) = keyframes.get(token) {
                out.push_str(replacement);
            } else {
                out.push_str(token);
            }
            continue;
        }

        out.push(ch);
        index += 1;
    }

    out
}
