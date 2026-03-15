use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::{Captures, Regex, RegexBuilder};

fn entity_map() -> &'static BTreeMap<&'static str, &'static str> {
    static ENTITY_MAP: OnceLock<BTreeMap<&'static str, &'static str>> = OnceLock::new();
    ENTITY_MAP.get_or_init(|| {
        BTreeMap::from([
            ("AMP", "&"),
            ("AMP;", "&"),
            ("amp", "&"),
            ("amp;", "&"),
            ("GT", ">"),
            ("GT;", ">"),
            ("LT", "<"),
            ("LT;", "<"),
            ("gt", ">"),
            ("gt;", ">"),
            ("lt", "<"),
            ("lt;", "<"),
            ("nbsp", "\u{00A0}"),
            ("nbsp;", "\u{00A0}"),
            ("quot", "\""),
            ("quot;", "\""),
            ("times", "\u{00D7}"),
            ("times;", "\u{00D7}"),
        ])
    })
}

fn numeric_regex() -> &'static Regex {
    static NUMERIC: OnceLock<Regex> = OnceLock::new();
    NUMERIC.get_or_init(|| {
        RegexBuilder::new(r"&#(x)?([0-9a-f]+);")
            .case_insensitive(true)
            .build()
            .expect("valid numeric regex")
    })
}

fn named_regex() -> &'static Regex {
    static NAMED: OnceLock<Regex> = OnceLock::new();
    NAMED.get_or_init(|| {
        let mut names = entity_map().keys().copied().collect::<Vec<_>>();
        names.sort_by(|left, right| right.len().cmp(&left.len()));
        let pattern = names.join("|");
        Regex::new(&format!(r"&({pattern})")).expect("valid named entity regex")
    })
}

pub fn decode_entities(value: &str) -> String {
    let numeric = numeric_regex().replace_all(value, |captures: &Captures<'_>| {
        let hex = captures.get(1).is_some();
        let code = captures
            .get(2)
            .map(|capture| capture.as_str())
            .unwrap_or_default();
        let parsed = if hex {
            u32::from_str_radix(code, 16).ok()
        } else {
            code.parse::<u32>().ok()
        };
        parsed
            .and_then(char::from_u32)
            .unwrap_or('\u{FFFD}')
            .to_string()
    });

    named_regex()
        .replace_all(&numeric, |captures: &Captures<'_>| {
            let entity = captures
                .get(1)
                .map(|capture| capture.as_str())
                .unwrap_or_default();
            entity_map()
                .get(entity)
                .copied()
                .unwrap_or_default()
                .to_string()
        })
        .into_owned()
}
