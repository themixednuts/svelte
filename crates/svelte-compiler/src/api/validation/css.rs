use super::*;
use crate::ast::modern::{
    Css, CssBlock, CssBlockChild, CssDeclaration, CssNameSelector, CssNode, CssPseudoClassSelector,
    CssRelativeSelector, CssRule, CssSelectorList, CssSimpleSelector,
};

pub(super) fn detect_css_compiler_errors(source: &str, root: &Root) -> Option<CompileError> {
    if let Some(css) = root.css.as_ref()
        && let Some(error) = detect_css_high_priority_structure_errors(source, css)
    {
        return Some(error);
    }

    let mut deferred_parse_error = None::<(CompilerDiagnosticKind, usize, usize)>;
    for (_style_start, style_end, content_start, content_end) in
        crate::compiler::phases::parse::style_block_ranges(root)
    {
        let content = source.get(content_start..content_end).unwrap_or_default();
        if let Err(error) =
            lightningcss::stylesheet::StyleSheet::parse(content, LightningParserOptions::default())
        {
            let parser_offset = error
                .loc
                .as_ref()
                .map(|loc| css_error_offset(content, loc.line, loc.column))
                .unwrap_or(content.len());
            let mut start = (content_start + parser_offset).min(content_end);
            let is_bad_string = matches!(
                &error.kind,
                lightningcss::error::ParserError::UnexpectedToken(
                    lightningcss::properties::custom::Token::BadString(_)
                        | lightningcss::properties::custom::Token::BadUrl(_),
                )
            );
            if is_bad_string {
                start = style_end;
            }

            let kind = match &error.kind {
                lightningcss::error::ParserError::EndOfInput => {
                    CompilerDiagnosticKind::CssExpectedIdentifier
                }
                lightningcss::error::ParserError::UnexpectedToken(
                    lightningcss::properties::custom::Token::BadString(_)
                    | lightningcss::properties::custom::Token::BadUrl(_),
                ) => CompilerDiagnosticKind::UnexpectedEof,
                lightningcss::error::ParserError::SelectorError(
                    lightningcss::error::SelectorError::DanglingCombinator
                    | lightningcss::error::SelectorError::EmptySelector,
                ) => CompilerDiagnosticKind::CssSelectorInvalid,
                _ => CompilerDiagnosticKind::CssExpectedIdentifier,
            };
            let end = if kind == CompilerDiagnosticKind::CssSelectorInvalid
                && source
                    .as_bytes()
                    .get(start)
                    .is_some_and(|byte| matches!(byte, b'>' | b'+' | b'~' | b'|'))
            {
                start.saturating_add(1).min(style_end)
            } else {
                start
            };

            if root.css.is_some() && !is_bad_string {
                deferred_parse_error.get_or_insert((kind, start, end));
                continue;
            }

            return Some(compile_error_with_range(source, kind, start, end));
        }
    }

    if let Some(css) = root.css.as_ref() {
        if let Some((kind, start, end)) = deferred_parse_error
            && !css_uses_extended_syntax(css)
        {
            return Some(compile_error_with_range(source, kind, start, end));
        }
        if let Some(error) = detect_css_selector_structure_errors(source, css) {
            return Some(error);
        }
    }

    None
}

fn detect_css_high_priority_structure_errors(source: &str, css: &Css) -> Option<CompileError> {
    for node in css.children.iter() {
        if let Some(error) = detect_css_high_priority_errors_in_node(source, node) {
            return Some(error);
        }
    }
    None
}

fn detect_css_high_priority_errors_in_node(source: &str, node: &CssNode) -> Option<CompileError> {
    match node {
        CssNode::Rule(rule) => {
            if let Some(error) =
                detect_css_high_priority_errors_in_selector_list(source, &rule.prelude)
            {
                return Some(error);
            }
            detect_css_high_priority_errors_in_block(source, &rule.block)
        }
        CssNode::Atrule(at_rule) => at_rule
            .block
            .as_ref()
            .and_then(|block| detect_css_high_priority_errors_in_block(source, block)),
    }
}

fn detect_css_high_priority_errors_in_block(
    source: &str,
    block: &CssBlock,
) -> Option<CompileError> {
    for child in block.children.iter() {
        match child {
            CssBlockChild::Rule(rule) => {
                if let Some(error) =
                    detect_css_high_priority_errors_in_selector_list(source, &rule.prelude)
                {
                    return Some(error);
                }
                if let Some(error) = detect_css_high_priority_errors_in_block(source, &rule.block) {
                    return Some(error);
                }
            }
            CssBlockChild::Atrule(at_rule) => {
                if let Some(inner) = at_rule.block.as_ref()
                    && let Some(error) = detect_css_high_priority_errors_in_block(source, inner)
                {
                    return Some(error);
                }
            }
            CssBlockChild::Declaration(_) => {}
        }
    }
    None
}

fn detect_css_high_priority_errors_in_selector_list(
    source: &str,
    list: &CssSelectorList,
) -> Option<CompileError> {
    for complex in list.children.iter() {
        for (relative_idx, relative) in complex.children.iter().enumerate() {
            if let Some((global_idx, pseudo)) = find_global_selector_in_relative(relative) {
                if global_idx == 0
                    && pure_global_function_is_in_local_middle(&complex.children, relative_idx)
                {
                    return Some(compile_error_with_range(
                        source,
                        CompilerDiagnosticKind::CssGlobalInvalidPlacement,
                        pseudo.start,
                        pseudo.end,
                    ));
                }

                if let Some(type_selector) = first_type_selector_after(relative, global_idx + 1) {
                    return Some(compile_error_with_range(
                        source,
                        CompilerDiagnosticKind::CssTypeSelectorInvalidPlacement,
                        type_selector.start,
                        type_selector.end,
                    ));
                }
            }

            for selector in relative.selectors.iter() {
                if let CssSimpleSelector::PseudoClassSelector(pseudo) = selector
                    && let Some(args) = pseudo.args.as_ref()
                    && let Some(error) =
                        detect_css_high_priority_errors_in_selector_list(source, args)
                {
                    return Some(error);
                }
            }
        }
    }
    None
}

fn css_uses_extended_syntax(css: &Css) -> bool {
    css.children.iter().any(css_node_uses_extended_syntax)
}

fn css_node_uses_extended_syntax(node: &CssNode) -> bool {
    match node {
        CssNode::Rule(rule) => {
            selector_list_uses_extended_syntax(&rule.prelude)
                || block_uses_extended_syntax(&rule.block)
        }
        CssNode::Atrule(at_rule) => {
            matches!(at_rule.name.as_ref(), "media" | "container")
                || at_rule
                    .block
                    .as_ref()
                    .is_some_and(block_uses_extended_syntax)
        }
    }
}

fn block_uses_extended_syntax(block: &CssBlock) -> bool {
    block.children.iter().any(|child| match child {
        CssBlockChild::Rule(rule) => {
            selector_list_uses_extended_syntax(&rule.prelude)
                || block_uses_extended_syntax(&rule.block)
        }
        CssBlockChild::Atrule(at_rule) => {
            matches!(at_rule.name.as_ref(), "media" | "container")
                || at_rule
                    .block
                    .as_ref()
                    .is_some_and(block_uses_extended_syntax)
        }
        CssBlockChild::Declaration(declaration) => {
            is_empty_custom_property_declaration(declaration)
        }
    })
}

fn selector_list_uses_extended_syntax(list: &CssSelectorList) -> bool {
    for complex in list.children.iter() {
        for relative in complex.children.iter() {
            if is_global_relative_selector(relative)
                || relative.selectors.iter().any(|selector| match selector {
                    CssSimpleSelector::NestingSelector(_) => true,
                    CssSimpleSelector::PseudoClassSelector(pseudo) => pseudo
                        .args
                        .as_ref()
                        .is_some_and(selector_list_uses_extended_syntax),
                    _ => false,
                })
            {
                return true;
            }
        }
    }
    false
}

fn css_error_offset(content: &str, zero_based_line: u32, one_based_column: u32) -> usize {
    let target_line = zero_based_line as usize;
    let target_column = one_based_column.saturating_sub(1) as usize;

    let mut line_start = 0usize;
    for _ in 0..target_line {
        let Some(rel_newline) = content.get(line_start..).and_then(|tail| tail.find('\n')) else {
            return content.len();
        };
        line_start += rel_newline + 1;
    }

    let mut offset = line_start;
    let mut remaining = target_column;
    while remaining > 0 {
        let Some(ch) = content.get(offset..).and_then(|tail| tail.chars().next()) else {
            return content.len();
        };
        if ch == '\n' {
            return offset;
        }
        offset += ch.len_utf8();
        remaining -= 1;
    }

    offset
}

#[derive(Clone, Copy)]
struct CssRuleContext {
    parent_rule_exists: bool,
    parent_rule_is_top_level_global_block: bool,
}

fn detect_css_selector_structure_errors(source: &str, css: &Css) -> Option<CompileError> {
    for node in css.children.iter() {
        if let Some(error) = detect_css_selector_errors_in_node(
            source,
            node,
            CssRuleContext {
                parent_rule_exists: false,
                parent_rule_is_top_level_global_block: false,
            },
        ) {
            return Some(error);
        }
    }
    None
}

fn detect_css_selector_errors_in_node(
    source: &str,
    node: &CssNode,
    context: CssRuleContext,
) -> Option<CompileError> {
    match node {
        CssNode::Rule(rule) => detect_css_selector_errors_in_rule(source, rule, context),
        CssNode::Atrule(at_rule) => {
            let block = at_rule.block.as_ref()?;
            detect_css_selector_errors_in_block(source, block, context)
        }
    }
}

fn detect_css_selector_errors_in_block(
    source: &str,
    block: &CssBlock,
    context: CssRuleContext,
) -> Option<CompileError> {
    for child in block.children.iter() {
        match child {
            CssBlockChild::Rule(rule) => {
                if let Some(error) = detect_css_selector_errors_in_rule(source, rule, context) {
                    return Some(error);
                }
            }
            CssBlockChild::Atrule(at_rule) => {
                if let Some(inner) = at_rule.block.as_ref()
                    && let Some(error) = detect_css_selector_errors_in_block(source, inner, context)
                {
                    return Some(error);
                }
            }
            CssBlockChild::Declaration(declaration) => {
                if declaration.value.trim().is_empty()
                    && !is_empty_custom_property_declaration(declaration)
                {
                    return Some(compile_error_custom_css(
                        source,
                        "css_empty_declaration",
                        "Declaration cannot be empty",
                        declaration.start,
                        declaration.end,
                    ));
                }
            }
        }
    }
    None
}

fn detect_css_selector_errors_in_rule(
    source: &str,
    rule: &CssRule,
    context: CssRuleContext,
) -> Option<CompileError> {
    let mut rule_is_global_block = false;

    for complex in rule.prelude.children.iter() {
        let mut complex_is_global_block = false;

        for (selector_idx, relative) in complex.children.iter().enumerate() {
            if let Some(global_idx) = relative.selectors.iter().position(is_global_block_selector) {
                if global_idx == 0 {
                    if relative.selectors.len() > 1
                        && selector_idx == 0
                        && !context.parent_rule_exists
                    {
                        let (start, end) = css_simple_selector_span(&relative.selectors[1]);
                        return Some(compile_error_with_range(
                            source,
                            CompilerDiagnosticKind::CssGlobalBlockInvalidModifierStart,
                            start,
                            end,
                        ));
                    }

                    rule_is_global_block = true;
                    complex_is_global_block = true;

                    if let Some(combinator) = relative.combinator.as_ref()
                        && combinator.name.as_ref() != " "
                    {
                        return Some(compile_error_with_range(
                            source,
                            CompilerDiagnosticKind::CssGlobalBlockInvalidCombinator,
                            relative.start,
                            relative.end,
                        ));
                    }

                    let is_lone_global =
                        complex.children.len() == 1 && complex.children[0].selectors.len() == 1;
                    if is_lone_global && rule.prelude.children.len() > 1 {
                        return Some(compile_error_with_range(
                            source,
                            CompilerDiagnosticKind::CssGlobalBlockInvalidList,
                            rule.prelude.start,
                            rule.prelude.end,
                        ));
                    }
                    if is_lone_global
                        && rule.prelude.children.len() == 1
                        && let Some(declaration) = first_css_declaration_in_block(&rule.block)
                    {
                        return Some(compile_error_with_range(
                            source,
                            CompilerDiagnosticKind::CssGlobalBlockInvalidDeclaration,
                            declaration.start,
                            declaration.end,
                        ));
                    }
                } else {
                    let (start, end) = css_simple_selector_span(&relative.selectors[global_idx]);
                    return Some(compile_error_with_range(
                        source,
                        CompilerDiagnosticKind::CssGlobalBlockInvalidModifier,
                        start,
                        end,
                    ));
                }
            }
        }

        if rule_is_global_block && !complex_is_global_block {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::CssGlobalBlockInvalidList,
                rule.prelude.start,
                rule.prelude.end,
            ));
        }
    }

    if let Some(error) =
        detect_css_errors_in_selector_list(source, &rule.prelude, false, context.parent_rule_exists)
    {
        return Some(error);
    }

    if let Some(nesting) = find_first_nesting_selector_in_list(&rule.prelude) {
        if !context.parent_rule_exists {
            if !top_level_nesting_selector_allowed(rule, nesting) {
                return Some(compile_error_with_range(
                    source,
                    CompilerDiagnosticKind::CssNestingSelectorInvalidPlacement,
                    nesting.start,
                    nesting.end,
                ));
            }
        } else if context.parent_rule_is_top_level_global_block {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::CssGlobalBlockInvalidModifierStart,
                nesting.start,
                nesting.end,
            ));
        }
    }

    let child_context = CssRuleContext {
        parent_rule_exists: true,
        parent_rule_is_top_level_global_block: !context.parent_rule_exists
            && rule_is_global_block
            && rule.prelude.children.len() == 1
            && rule.prelude.children[0].children.len() == 1
            && rule.prelude.children[0].children[0].selectors.len() == 1,
    };
    detect_css_selector_errors_in_block(source, &rule.block, child_context)
}

fn detect_css_errors_in_selector_list(
    source: &str,
    list: &CssSelectorList,
    inside_pseudo_class: bool,
    allow_leading_combinator: bool,
) -> Option<CompileError> {
    for complex in list.children.iter() {
        if !inside_pseudo_class
            && !allow_leading_combinator
            && let Some(relative) = complex.children.first()
            && let Some(combinator) = relative.combinator.as_ref()
        {
            let combinator_end = combinator.end.max(combinator.start.saturating_add(1));
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::CssSelectorInvalid,
                combinator.start,
                combinator_end,
            ));
        }

        if inside_pseudo_class
            && let Some(global_relative) = complex
                .children
                .iter()
                .find(|relative| is_global_relative_selector(relative))
            && let Some(CssSimpleSelector::PseudoClassSelector(pseudo)) =
                global_relative.selectors.first()
            && pseudo.args.is_none()
        {
            return Some(compile_error_with_range(
                source,
                CompilerDiagnosticKind::CssGlobalBlockInvalidPlacement,
                pseudo.start,
                pseudo.end,
            ));
        }

        for (relative_idx, relative) in complex.children.iter().enumerate() {
            if relative.selectors.is_empty() {
                return Some(compile_error_with_range(
                    source,
                    CompilerDiagnosticKind::CssSelectorInvalid,
                    relative.end,
                    relative.end,
                ));
            }

            if let Some((global_idx, pseudo)) = find_global_selector_in_relative(relative) {
                let Some(args) = pseudo.args.as_ref() else {
                    continue;
                };

                let global_is_standalone =
                    complex.children.len() == 1 && global_idx == 0 && relative.selectors.len() == 1;
                if !css_global_selector_is_single_selector(args) && !global_is_standalone {
                    return Some(compile_error_with_range(
                        source,
                        CompilerDiagnosticKind::CssGlobalInvalidSelector,
                        pseudo.start,
                        pseudo.end,
                    ));
                }

                if global_idx == 0
                    && pure_global_function_is_in_local_middle(&complex.children, relative_idx)
                {
                    return Some(compile_error_with_range(
                        source,
                        CompilerDiagnosticKind::CssGlobalInvalidPlacement,
                        pseudo.start,
                        pseudo.end,
                    ));
                }

                if global_idx > 0 && css_global_selector_contains_type(args) {
                    return Some(compile_error_with_range(
                        source,
                        CompilerDiagnosticKind::CssGlobalInvalidSelectorList,
                        pseudo.start,
                        pseudo.end,
                    ));
                }

                if let Some(type_selector) = first_type_selector_after(relative, global_idx + 1) {
                    return Some(compile_error_with_range(
                        source,
                        CompilerDiagnosticKind::CssTypeSelectorInvalidPlacement,
                        type_selector.start,
                        type_selector.end,
                    ));
                }
            }

            for selector in relative.selectors.iter() {
                if let CssSimpleSelector::PseudoClassSelector(pseudo) = selector
                    && let Some(args) = pseudo.args.as_ref()
                    && let Some(error) =
                        detect_css_errors_in_selector_list(source, args, true, true)
                {
                    return Some(error);
                }
            }
        }
    }
    None
}

fn is_global_relative_selector(relative: &CssRelativeSelector) -> bool {
    matches!(
        relative.selectors.first(),
        Some(CssSimpleSelector::PseudoClassSelector(pseudo)) if pseudo.name.as_ref() == "global"
    )
}

fn find_global_selector_in_relative(
    relative: &CssRelativeSelector,
) -> Option<(usize, &CssPseudoClassSelector)> {
    for (index, selector) in relative.selectors.iter().enumerate() {
        if let CssSimpleSelector::PseudoClassSelector(pseudo) = selector
            && pseudo.name.as_ref() == "global"
            && pseudo.args.is_some()
        {
            return Some((index, pseudo));
        }
    }
    None
}

fn css_global_selector_is_single_selector(args: &CssSelectorList) -> bool {
    args.children.len() == 1
}

fn pure_global_function_is_in_local_middle(
    relatives: &[CssRelativeSelector],
    index: usize,
) -> bool {
    let has_local_before = relatives[..index]
        .iter()
        .any(|relative| !is_pure_global_function_relative(relative));
    let has_local_after = relatives[index + 1..]
        .iter()
        .any(|relative| !is_pure_global_function_relative(relative));
    has_local_before && has_local_after
}

fn is_pure_global_function_relative(relative: &CssRelativeSelector) -> bool {
    matches!(
        find_global_selector_in_relative(relative),
        Some((0, _)) if relative.selectors.len() == 1
    )
}

fn is_empty_custom_property_declaration(declaration: &CssDeclaration) -> bool {
    declaration.value.trim().is_empty() && declaration.property.trim_start().starts_with("--")
}

fn css_global_selector_contains_type(args: &CssSelectorList) -> bool {
    args.children
        .first()
        .and_then(|complex| complex.children.first())
        .map(|relative| {
            relative
                .selectors
                .iter()
                .any(|selector| matches!(selector, CssSimpleSelector::TypeSelector(_)))
        })
        .unwrap_or(false)
}

fn first_type_selector_after(
    relative: &CssRelativeSelector,
    start_index: usize,
) -> Option<&CssNameSelector> {
    for selector in relative.selectors.iter().skip(start_index) {
        if let CssSimpleSelector::TypeSelector(selector) = selector {
            return Some(selector);
        }
    }
    None
}

fn find_first_nesting_selector_in_list(list: &CssSelectorList) -> Option<&CssNameSelector> {
    for complex in list.children.iter() {
        for relative in complex.children.iter() {
            for selector in relative.selectors.iter() {
                match selector {
                    CssSimpleSelector::NestingSelector(nesting) => return Some(nesting),
                    CssSimpleSelector::PseudoClassSelector(pseudo) => {
                        if let Some(args) = pseudo.args.as_ref()
                            && let Some(nesting) = find_first_nesting_selector_in_list(args)
                        {
                            return Some(nesting);
                        }
                    }
                    CssSimpleSelector::TypeSelector(_)
                    | CssSimpleSelector::IdSelector(_)
                    | CssSimpleSelector::ClassSelector(_)
                    | CssSimpleSelector::PseudoElementSelector(_)
                    | CssSimpleSelector::AttributeSelector(_)
                    | CssSimpleSelector::Nth(_)
                    | CssSimpleSelector::Percentage(_) => {}
                }
            }
        }
    }
    None
}

fn top_level_nesting_selector_allowed(rule: &CssRule, nesting: &CssNameSelector) -> bool {
    if rule.prelude.children.len() != 1 {
        return false;
    }
    let Some(first_relative) = rule.prelude.children[0].children.first() else {
        return false;
    };
    if first_relative.selectors.len() != 1 {
        return false;
    }
    let Some(CssSimpleSelector::PseudoClassSelector(pseudo)) = first_relative.selectors.first()
    else {
        return false;
    };
    if pseudo.name.as_ref() != "global" {
        return false;
    }
    let Some(args) = pseudo.args.as_ref() else {
        return false;
    };
    let Some(CssSimpleSelector::NestingSelector(first_in_global)) = args
        .children
        .first()
        .and_then(|complex| complex.children.first())
        .and_then(|relative| relative.selectors.first())
    else {
        return false;
    };
    first_in_global.start == nesting.start && first_in_global.end == nesting.end
}

fn is_global_block_selector(selector: &CssSimpleSelector) -> bool {
    matches!(
        selector,
        CssSimpleSelector::PseudoClassSelector(pseudo)
            if pseudo.name.as_ref() == "global" && pseudo.args.is_none()
    )
}

fn first_css_declaration_in_block(block: &CssBlock) -> Option<&CssDeclaration> {
    for child in block.children.iter() {
        if let CssBlockChild::Declaration(declaration) = child {
            return Some(declaration);
        }
    }
    None
}

fn css_simple_selector_span(selector: &CssSimpleSelector) -> (usize, usize) {
    match selector {
        CssSimpleSelector::TypeSelector(selector)
        | CssSimpleSelector::IdSelector(selector)
        | CssSimpleSelector::ClassSelector(selector)
        | CssSimpleSelector::PseudoElementSelector(selector)
        | CssSimpleSelector::NestingSelector(selector) => (selector.start, selector.end),
        CssSimpleSelector::PseudoClassSelector(selector) => (selector.start, selector.end),
        CssSimpleSelector::AttributeSelector(selector) => (selector.start, selector.end),
        CssSimpleSelector::Nth(selector) | CssSimpleSelector::Percentage(selector) => {
            (selector.start, selector.end)
        }
    }
}

pub(super) fn detect_multiple_top_level_styles(source: &str, root: &Root) -> Option<CompileError> {
    let duplicate = root.styles.get(1)?;
    Some(compile_error_with_range(
        source,
        CompilerDiagnosticKind::StyleDuplicate,
        duplicate.start,
        duplicate.start,
    ))
}

fn compile_error_custom_css(
    source: &str,
    code: &'static str,
    message: impl Into<Arc<str>>,
    start: usize,
    end: usize,
) -> CompileError {
    let (start_line, start_column) = line_column_at_offset(source, start);
    let (end_line, end_column) = line_column_at_offset(source, end);

    CompileError {
        code: Arc::from(code),
        message: message.into(),
        position: Some(Box::new(SourcePosition { start, end })),
        start: Some(Box::new(SourceLocation {
            line: start_line,
            column: start_column,
            character: start,
        })),
        end: Some(Box::new(SourceLocation {
            line: end_line,
            column: end_column,
            character: end,
        })),
        filename: None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::validate_component_css;
    use crate::compiler::phases::parse::parse_component_for_compile;

    fn validate(source: &str) -> Option<crate::error::CompileError> {
        let parsed = parse_component_for_compile(source).expect("parse component");
        validate_component_css(source, parsed.root())
    }

    #[test]
    fn rejects_duplicate_top_level_styles_from_ast() {
        let error = validate("<style>.a{color:red}</style><style>.b{color:blue}</style>")
            .expect("expected validation error");
        assert_eq!(error.code.as_ref(), "style_duplicate");
    }
}
