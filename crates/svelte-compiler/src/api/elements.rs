use std::str::FromStr;
use unicode_ident::{is_xid_continue, is_xid_start};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElementKind {
    Script,
    Style,
    Slot,
    Template,
    Textarea,
    Svelte(SvelteElementKind),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttributeKind {
    This,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SvelteElementKind {
    Head,
    Options,
    Window,
    Document,
    Body,
    Element,
    Component,
    SelfTag,
    Fragment,
    Boundary,
    Unknown,
}

impl SvelteElementKind {
    pub(crate) fn is_known(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

impl FromStr for SvelteElementKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "head" => Ok(Self::Head),
            "options" => Ok(Self::Options),
            "window" => Ok(Self::Window),
            "document" => Ok(Self::Document),
            "body" => Ok(Self::Body),
            "element" => Ok(Self::Element),
            "component" => Ok(Self::Component),
            "self" => Ok(Self::SelfTag),
            "fragment" => Ok(Self::Fragment),
            "boundary" => Ok(Self::Boundary),
            _ => Err(()),
        }
    }
}

pub(crate) fn classify_element_name(name: &str) -> ElementKind {
    match name {
        "script" => ElementKind::Script,
        "style" => ElementKind::Style,
        "slot" => ElementKind::Slot,
        "template" => ElementKind::Template,
        "textarea" => ElementKind::Textarea,
        _ => match name.strip_prefix("svelte:") {
            Some(name) => ElementKind::Svelte(name.parse().unwrap_or(SvelteElementKind::Unknown)),
            None => ElementKind::Other,
        },
    }
}

pub(crate) fn classify_attribute_name(name: &str) -> AttributeKind {
    match name {
        "this" => AttributeKind::This,
        _ => AttributeKind::Other,
    }
}

pub(crate) fn is_component_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    first.is_uppercase() || (is_ident_start(first) && name.contains('.'))
}

pub(crate) fn is_custom_element_name(name: &str) -> bool {
    name.contains('-')
}

pub(crate) fn is_valid_component_name(name: &str) -> bool {
    let Some((head, tail)) = name.split_once('.') else {
        let mut chars = name.chars();
        let Some(first) = chars.next() else {
            return false;
        };

        return first.is_uppercase() && chars.all(is_component_char);
    };

    is_identifier(head)
        && !tail.is_empty()
        && tail
            .split('.')
            .all(|segment| !segment.is_empty() && segment.chars().all(is_component_char))
}

pub(crate) fn is_valid_element_name(name: &str) -> bool {
    is_doctype_name(name) || is_meta_name(name) || is_tag_name(name)
}

pub(crate) fn is_void_element_name(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "keygen"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_doctype_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('!') else {
        return false;
    };

    !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn is_meta_name(name: &str) -> bool {
    let Some((namespace, local)) = name.split_once(':') else {
        return false;
    };

    is_ascii_alnum_ident(namespace) && is_meta_local_name(local)
}

fn is_tag_name(name: &str) -> bool {
    let mut chars = name.chars().peekable();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }

    while chars.next_if(|ch| ch.is_ascii_alphanumeric()).is_some() {}

    while chars.next_if_eq(&'-').is_some() {
        let mut segment = 0;
        while chars.next_if(|ch| is_tag_char(*ch)).is_some() {
            segment += 1;
        }
        if segment == 0 {
            return false;
        }
    }

    chars.next().is_none()
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    is_ident_start(first) && chars.all(is_component_char)
}

fn is_ascii_alnum_ident(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    first.is_ascii_alphabetic() && chars.all(|ch| ch.is_ascii_alphanumeric())
}

fn is_meta_local_name(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }

    let mut last = first;
    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '-') {
            return false;
        }
        last = ch;
    }

    last.is_ascii_alphanumeric()
}

fn is_ident_start(ch: char) -> bool {
    ch == '$' || ch == '_' || is_xid_start(ch)
}

fn is_component_char(ch: char) -> bool {
    ch == '$' || ch == '\u{200c}' || ch == '\u{200d}' || is_xid_continue(ch)
}

fn is_tag_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(ch, '.' | '_' | '-')
        || matches!(
            ch,
            '\u{00b7}'
                | '\u{00c0}'..='\u{00d6}'
                | '\u{00d8}'..='\u{00f6}'
                | '\u{00f8}'..='\u{037d}'
                | '\u{037f}'..='\u{1fff}'
                | '\u{200c}'..='\u{200d}'
                | '\u{203f}'..='\u{2040}'
                | '\u{2070}'..='\u{218f}'
                | '\u{2c00}'..='\u{2fef}'
                | '\u{3001}'..='\u{d7ff}'
                | '\u{f900}'..='\u{fdcf}'
                | '\u{fdf0}'..='\u{fffd}'
                | '\u{10000}'..='\u{effff}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_component_names() {
        assert!(is_valid_component_name("Component"));
        assert!(is_valid_component_name("Wunderschön"));
        assert!(is_valid_component_name("Namespace.Schön"));
        assert!(is_valid_component_name("namespace.1"));
    }

    #[test]
    fn rejects_invalid_component_names() {
        assert!(!is_valid_component_name("Components[1]"));
        assert!(!is_valid_component_name("Namespace."));
        assert!(!is_valid_component_name(".Component"));
    }

    #[test]
    fn accepts_valid_element_names() {
        assert!(is_valid_element_name("div"));
        assert!(is_valid_element_name("foreignObject"));
        assert!(is_valid_element_name("math-α"));
        assert!(is_valid_element_name("svelte:head"));
        assert!(is_valid_element_name("!DOCTYPE"));
    }

    #[test]
    fn rejects_invalid_element_names() {
        assert!(!is_valid_element_name("yes[no]"));
        assert!(!is_valid_element_name("svelte:"));
        assert!(!is_valid_element_name("1div"));
    }
}
