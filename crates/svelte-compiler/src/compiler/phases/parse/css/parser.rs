use std::sync::Arc;

use crate::ast::modern::{
    CssAtrule, CssAttributeSelector, CssBlock, CssBlockChild, CssBlockType, CssCombinator,
    CssCombinatorType, CssComplexSelector, CssComplexSelectorType, CssDeclaration, CssNameSelector,
    CssNode, CssPseudoClassSelector, CssRelativeSelector, CssRelativeSelectorType, CssRule,
    CssSelectorList, CssSelectorListType, CssSimpleSelector, CssValueSelector,
};
pub(crate) struct CssParser<'a> {
    source: &'a str,
    index: usize,
    end: usize,
}

impl<'a> CssParser<'a> {
    pub(crate) fn new(source: &'a str, start: usize, end: usize) -> Self {
        Self {
            source,
            index: start,
            end,
        }
    }

    pub(crate) fn read_body(&mut self) -> Vec<CssNode> {
        let mut children = Vec::new();
        while self.index < self.end {
            self.allow_comment_or_whitespace();
            if self.index >= self.end {
                break;
            }
            if self.match_str("@") {
                children.push(CssNode::Atrule(self.read_at_rule()));
            } else {
                children.push(CssNode::Rule(self.read_rule()));
            }
        }
        children
    }

    fn read_at_rule(&mut self) -> CssAtrule {
        let start = self.index;
        self.eat_str("@");
        let name = Arc::from(self.read_identifier());
        let prelude = Arc::from(self.read_value());

        let block = if self.match_str("{") {
            Some(self.read_block())
        } else {
            self.eat_str(";");
            None
        };

        CssAtrule {
            start,
            end: self.index,
            name,
            prelude,
            block,
        }
    }

    fn read_rule(&mut self) -> CssRule {
        let start = self.index;
        CssRule {
            prelude: self.read_selector_list(false),
            block: self.read_block(),
            start,
            end: self.index,
        }
    }

    fn read_selector_list(&mut self, inside_pseudo_class: bool) -> CssSelectorList {
        let mut children = Vec::new();
        self.allow_comment_or_whitespace();
        let start = self.index;

        while self.index < self.end {
            children.push(self.read_selector(inside_pseudo_class));
            let selector_end = self.index;
            self.allow_comment_or_whitespace();

            let done = if inside_pseudo_class {
                self.match_str(")")
            } else {
                self.match_str("{")
            };
            if done {
                return CssSelectorList {
                    r#type: CssSelectorListType::SelectorList,
                    start,
                    end: selector_end,
                    children: children.into_boxed_slice(),
                };
            }

            if !self.eat_str(",") {
                return CssSelectorList {
                    r#type: CssSelectorListType::SelectorList,
                    start,
                    end: selector_end,
                    children: children.into_boxed_slice(),
                };
            }
            self.allow_comment_or_whitespace();
        }

        CssSelectorList {
            r#type: CssSelectorListType::SelectorList,
            start,
            end: self.index,
            children: children.into_boxed_slice(),
        }
    }

    fn read_selector(&mut self, inside_pseudo_class: bool) -> CssComplexSelector {
        let list_start = self.index;
        let mut children = Vec::new();
        let mut relative = CssRelativeSelector {
            r#type: CssRelativeSelectorType::RelativeSelector,
            combinator: None,
            selectors: Vec::new().into_boxed_slice(),
            start: self.index,
            end: self.index,
        };
        let mut selectors = Vec::new();

        while self.index < self.end {
            let start = self.index;
            let mut parsed = false;

            if self.eat_str("&") {
                selectors.push(CssSimpleSelector::NestingSelector(CssNameSelector {
                    name: Arc::from("&"),
                    start,
                    end: self.index,
                }));
                parsed = true;
            } else if self.eat_str("*") {
                let mut name = Arc::from("*");
                if self.eat_str("|") {
                    name = Arc::from(self.read_identifier());
                }
                selectors.push(CssSimpleSelector::TypeSelector(CssNameSelector {
                    name,
                    start,
                    end: self.index,
                }));
                parsed = true;
            } else if self.eat_str("#") {
                selectors.push(CssSimpleSelector::IdSelector(CssNameSelector {
                    name: Arc::from(self.read_identifier()),
                    start,
                    end: self.index,
                }));
                parsed = true;
            } else if self.eat_str(".") {
                selectors.push(CssSimpleSelector::ClassSelector(CssNameSelector {
                    name: Arc::from(self.read_identifier()),
                    start,
                    end: self.index,
                }));
                parsed = true;
            } else if self.eat_str("::") {
                selectors.push(CssSimpleSelector::PseudoElementSelector(CssNameSelector {
                    name: Arc::from(self.read_identifier()),
                    start,
                    end: self.index,
                }));
                if self.eat_str("(") {
                    let _ = self.read_selector_list(true);
                    self.eat_str(")");
                }
                parsed = true;
            } else if self.eat_str(":") {
                let name = Arc::from(self.read_identifier());
                let mut args = None;
                if self.eat_str("(") {
                    args = Some(self.read_selector_list(true));
                    self.eat_str(")");
                }
                selectors.push(CssSimpleSelector::PseudoClassSelector(
                    CssPseudoClassSelector {
                        name,
                        args,
                        start,
                        end: self.index,
                    },
                ));
                parsed = true;
            } else if self.eat_str("[") {
                self.allow_whitespace();
                let name = Arc::from(self.read_identifier());
                self.allow_whitespace();

                let matcher = self.read_attribute_matcher().map(Arc::from);
                let value = if matcher.is_some() {
                    self.allow_whitespace();
                    Some(Arc::from(self.read_attribute_value()))
                } else {
                    None
                };

                self.allow_whitespace();
                let flags = self.read_alpha_identifier().map(Arc::from);
                self.allow_whitespace();
                self.eat_str("]");

                selectors.push(CssSimpleSelector::AttributeSelector(CssAttributeSelector {
                    start,
                    end: self.index,
                    name,
                    matcher,
                    value,
                    flags,
                }));
                parsed = true;
            } else if inside_pseudo_class && let Some(value) = self.read_nth_value() {
                selectors.push(CssSimpleSelector::Nth(CssValueSelector {
                    value: Arc::from(value),
                    start,
                    end: self.index,
                }));
                parsed = true;
            }

            if !parsed && let Some(value) = self.read_percentage_value() {
                selectors.push(CssSimpleSelector::Percentage(CssValueSelector {
                    value: Arc::from(value),
                    start,
                    end: self.index,
                }));
                parsed = true;
            }

            if !parsed && !self.matches_combinator() {
                let mut name = self.read_identifier();
                if !name.is_empty() {
                    if self.eat_str("|") {
                        name = self.read_identifier();
                    }
                    selectors.push(CssSimpleSelector::TypeSelector(CssNameSelector {
                        name: Arc::from(name),
                        start,
                        end: self.index,
                    }));
                    parsed = true;
                }
            }

            let index = self.index;
            self.allow_comment_or_whitespace();
            let done = self.match_str(",")
                || (inside_pseudo_class && self.match_str(")"))
                || (!inside_pseudo_class && self.match_str("{"));
            if done {
                self.index = index;
                relative.selectors = selectors.into_boxed_slice();
                relative.end = index;
                children.push(relative);
                return CssComplexSelector {
                    r#type: CssComplexSelectorType::ComplexSelector,
                    start: list_start,
                    end: index,
                    children: children.into_boxed_slice(),
                };
            }

            self.index = index;
            if let Some(combinator) = self.read_combinator() {
                if !selectors.is_empty() {
                    relative.selectors = selectors.into_boxed_slice();
                    relative.end = index;
                    children.push(relative);
                }

                let comb_start = combinator.start;
                relative = CssRelativeSelector {
                    r#type: CssRelativeSelectorType::RelativeSelector,
                    combinator: Some(combinator),
                    selectors: Vec::new().into_boxed_slice(),
                    start: comb_start,
                    end: comb_start,
                };
                selectors = Vec::new();
                self.allow_whitespace();
            }

            if !parsed && self.index == start {
                break;
            }
        }

        relative.selectors = selectors.into_boxed_slice();
        relative.end = self.index;
        children.push(relative);
        CssComplexSelector {
            r#type: CssComplexSelectorType::ComplexSelector,
            start: list_start,
            end: self.index,
            children: children.into_boxed_slice(),
        }
    }

    fn read_combinator(&mut self) -> Option<CssCombinator> {
        let start = self.index;
        self.allow_whitespace();
        let whitespace_end = self.index;

        let (name, token_start, token_end) = if self.eat_str("||") {
            ("||", whitespace_end, self.index)
        } else if self.eat_str("+") {
            ("+", whitespace_end, self.index)
        } else if self.eat_str("~") {
            ("~", whitespace_end, self.index)
        } else if self.eat_str(">") {
            (">", whitespace_end, self.index)
        } else {
            ("", whitespace_end, whitespace_end)
        };

        if !name.is_empty() {
            self.allow_whitespace();
            return Some(CssCombinator {
                r#type: CssCombinatorType::Combinator,
                name: Arc::from(name),
                start: token_start,
                end: token_end,
            });
        }

        if whitespace_end != start {
            return Some(CssCombinator {
                r#type: CssCombinatorType::Combinator,
                name: Arc::from(" "),
                start,
                end: whitespace_end,
            });
        }

        None
    }

    fn read_block(&mut self) -> CssBlock {
        let start = self.index;
        self.eat_str("{");
        let mut children = Vec::new();

        while self.index < self.end {
            self.allow_comment_or_whitespace();
            if self.match_str("}") {
                break;
            }
            children.push(self.read_block_item());
        }

        self.eat_str("}");
        CssBlock {
            r#type: CssBlockType::Block,
            start,
            end: self.index,
            children: children.into_boxed_slice(),
        }
    }

    fn read_block_item(&mut self) -> CssBlockChild {
        if self.match_str("@") {
            return CssBlockChild::Atrule(self.read_at_rule());
        }

        let start = self.index;
        let _ = self.read_value();
        let ch = self.current_byte();
        self.index = start;

        if ch == Some(b'{') {
            CssBlockChild::Rule(self.read_rule())
        } else {
            CssBlockChild::Declaration(self.read_declaration())
        }
    }

    fn read_declaration(&mut self) -> CssDeclaration {
        let start = self.index;
        let property_start = self.index;
        while let Some(ch) = self.current_byte() {
            if ch == b':' || ch.is_ascii_whitespace() {
                break;
            }
            self.index += 1;
        }
        let property = Arc::from(
            self.source
                .get(property_start..self.index)
                .unwrap_or_default(),
        );
        self.allow_whitespace();
        self.eat_str(":");
        self.allow_whitespace();
        let value = Arc::from(self.read_value());
        let end = self.index;
        if !self.match_str("}") {
            self.eat_str(";");
        }

        CssDeclaration {
            start,
            end,
            property,
            value,
        }
    }

    fn read_value(&mut self) -> String {
        let mut value = String::new();
        let mut escaped = false;
        let mut in_url = false;
        let mut quote_mark: Option<u8> = None;

        while self.index < self.end {
            let ch = match self.current_byte() {
                Some(ch) => ch,
                None => break,
            };

            if escaped {
                value.push('\\');
                value.push(ch as char);
                escaped = false;
                self.index += 1;
                continue;
            }

            if ch == b'\\' {
                escaped = true;
                self.index += 1;
                continue;
            }

            if Some(ch) == quote_mark {
                quote_mark = None;
            } else if quote_mark.is_none() && (ch == b'\'' || ch == b'"') {
                quote_mark = Some(ch);
            } else if ch == b')' {
                in_url = false;
            } else if ch == b'(' && value.ends_with("url") {
                in_url = true;
            } else if (ch == b';' || ch == b'{' || ch == b'}') && !in_url && quote_mark.is_none() {
                return value.trim().to_string();
            }

            value.push(ch as char);
            self.index += 1;
        }

        value.trim().to_string()
    }

    fn read_attribute_value(&mut self) -> String {
        let mut value = String::new();
        let mut escaped = false;
        let quote_mark = if self.eat_str("\"") {
            Some(b'"')
        } else if self.eat_str("'") {
            Some(b'\'')
        } else {
            None
        };

        while self.index < self.end {
            let ch = match self.current_byte() {
                Some(ch) => ch,
                None => break,
            };

            if escaped {
                value.push('\\');
                value.push(ch as char);
                escaped = false;
            } else if ch == b'\\' {
                escaped = true;
            } else if let Some(quote) = quote_mark {
                if ch == quote {
                    self.index += 1;
                    return value.trim().to_string();
                }
                value.push(ch as char);
            } else if ch.is_ascii_whitespace() || ch == b']' {
                return value.trim().to_string();
            } else {
                value.push(ch as char);
            }
            self.index += 1;
        }

        value.trim().to_string()
    }

    fn read_identifier(&mut self) -> String {
        let start = self.index;
        let mut out = String::new();

        while let Some(ch) = self.current_byte() {
            if ch == b'\\' {
                if let Some(next) = self.byte_at(self.index + 1) {
                    out.push('\\');
                    out.push(next as char);
                    self.index += 2;
                    continue;
                }
                break;
            }

            if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-' {
                out.push(ch as char);
                self.index += 1;
            } else {
                break;
            }
        }

        if out.is_empty() {
            self.index = start;
        }
        out
    }

    fn read_attribute_matcher(&mut self) -> Option<&'static str> {
        if self.eat_str("~=") {
            Some("~=")
        } else if self.eat_str("^=") {
            Some("^=")
        } else if self.eat_str("$=") {
            Some("$=")
        } else if self.eat_str("*=") {
            Some("*=")
        } else if self.eat_str("|=") {
            Some("|=")
        } else if self.eat_str("=") {
            Some("=")
        } else {
            None
        }
    }

    fn read_alpha_identifier(&mut self) -> Option<String> {
        let start = self.index;
        let mut out = String::new();
        while let Some(ch) = self.current_byte() {
            if ch.is_ascii_alphabetic() {
                out.push(ch as char);
                self.index += 1;
            } else {
                break;
            }
        }
        if out.is_empty() {
            self.index = start;
            None
        } else {
            Some(out)
        }
    }

    fn read_percentage_value(&mut self) -> Option<String> {
        let start = self.index;
        let mut idx = self.index;
        let mut has_digit = false;
        while let Some(ch) = self.byte_at(idx) {
            if ch.is_ascii_digit() {
                has_digit = true;
                idx += 1;
            } else {
                break;
            }
        }
        if self.byte_at(idx) == Some(b'.') {
            idx += 1;
            while let Some(ch) = self.byte_at(idx) {
                if ch.is_ascii_digit() {
                    has_digit = true;
                    idx += 1;
                } else {
                    break;
                }
            }
        }
        if !has_digit || self.byte_at(idx) != Some(b'%') {
            self.index = start;
            return None;
        }
        idx += 1;
        self.index = idx;
        Some(self.source.get(start..idx).unwrap_or_default().to_string())
    }

    fn read_nth_value(&mut self) -> Option<String> {
        let start = self.index;
        let rest = self.source.get(start..self.end)?;
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            return None;
        }

        let bytes = trimmed.as_bytes();
        let first = bytes[0];
        let second = bytes.get(1).copied();
        let even_like = trimmed.starts_with("even")
            && bytes.get(4).copied().is_none_or(|ch| {
                ch.is_ascii_whitespace() || ch == b')' || ch == b',' || ch == b'o'
            });
        let odd_like = trimmed.starts_with("odd")
            && bytes.get(3).copied().is_none_or(|ch| {
                ch.is_ascii_whitespace() || ch == b')' || ch == b',' || ch == b'o'
            });
        let n_like = first == b'n'
            && second.is_none_or(|ch| {
                ch.is_ascii_whitespace()
                    || ch == b'+'
                    || ch == b'-'
                    || ch == b')'
                    || ch == b','
                    || ch == b'o'
            });
        let nth_like = even_like
            || odd_like
            || first == b'+'
            || first == b'-'
            || n_like
            || first.is_ascii_digit();
        if !nth_like {
            return None;
        }

        let leading = rest.len() - trimmed.len();
        let expr_start = start + leading;
        let expr_rest = self.source.get(expr_start..self.end)?;

        let mut expr_end = expr_start;
        for (i, ch) in expr_rest.char_indices() {
            if ch == ')' || ch == ',' {
                break;
            }
            expr_end = expr_start + i + ch.len_utf8();
        }

        let mut token_end = expr_end;
        let expr = self.source.get(expr_start..expr_end).unwrap_or_default();
        if let Some(of_pos) = expr.find(" of ") {
            token_end = expr_start + of_pos + 4;
        } else {
            while token_end > expr_start {
                let ch = self.source.as_bytes()[token_end - 1];
                if ch.is_ascii_whitespace() {
                    token_end -= 1;
                } else {
                    break;
                }
            }
        }

        if token_end <= expr_start {
            return None;
        }

        self.index = token_end;
        Some(
            self.source
                .get(expr_start..token_end)
                .unwrap_or_default()
                .to_string(),
        )
    }

    fn allow_comment_or_whitespace(&mut self) {
        self.allow_whitespace();
        loop {
            if self.eat_str("/*") {
                while self.index < self.end && !self.match_str("*/") {
                    self.index += 1;
                }
                let _ = self.eat_str("*/");
                self.allow_whitespace();
                continue;
            }

            if self.eat_str("<!--") {
                while self.index < self.end && !self.match_str("-->") {
                    self.index += 1;
                }
                let _ = self.eat_str("-->");
                self.allow_whitespace();
                continue;
            }

            break;
        }
    }

    fn allow_whitespace(&mut self) {
        while let Some(ch) = self.current_byte() {
            if ch.is_ascii_whitespace() {
                self.index += 1;
            } else {
                break;
            }
        }
    }

    fn matches_combinator(&self) -> bool {
        self.match_str("||") || self.match_str("+") || self.match_str("~") || self.match_str(">")
    }

    fn match_str(&self, token: &str) -> bool {
        self.source
            .get(self.index..self.end)
            .is_some_and(|rest| rest.starts_with(token))
    }

    fn eat_str(&mut self, token: &str) -> bool {
        if self.match_str(token) {
            self.index += token.len();
            true
        } else {
            false
        }
    }

    fn current_byte(&self) -> Option<u8> {
        self.byte_at(self.index)
    }

    fn byte_at(&self, index: usize) -> Option<u8> {
        self.source.as_bytes().get(index).copied()
    }
}
