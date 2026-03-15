use std::collections::BTreeSet;

use serde_json::{Map, Value};

const GENERATED_COMMENT: &str = "// this file is generated — do not edit it\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvKind {
    Public,
    Private,
}

impl EnvKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

pub fn create_static_module(module_id: &str, env: &Map<String, Value>) -> String {
    let mut declarations = Vec::new();

    for (key, value) in env {
        let Some(value) = value.as_str() else {
            continue;
        };
        if !is_valid_identifier(key) || reserved_identifiers().contains(key.as_str()) {
            continue;
        }

        declarations.push(format!(
            "/** @type {{import({module_id:?}).{key}}} */\nexport const {key} = {};",
            serde_json::to_string(value).expect("env value json serialization"),
        ));
    }

    format!("{GENERATED_COMMENT}{}", declarations.join("\n\n"))
}

pub fn create_dynamic_module(
    kind: EnvKind,
    dev_values: Option<&Map<String, Value>>,
    runtime_base: &str,
) -> String {
    if let Some(dev_values) = dev_values {
        let keys = dev_values
            .iter()
            .filter_map(|(key, value)| {
                value.as_str().map(|value| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(key).expect("env key json serialization"),
                        serde_json::to_string(value).expect("env value json serialization"),
                    )
                })
            })
            .collect::<Vec<_>>();
        return format!("export const env = {{\n{}\n}}", keys.join(",\n"));
    }

    format!(
        "export {{ {}_env as env }} from '{runtime_base}/shared-server.js';",
        kind.as_str()
    )
}

pub fn is_valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !matches!(first, 'a'..='z' | 'A'..='Z' | '_' | '$') {
        return false;
    }

    chars.all(|char| matches!(char, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '$'))
}

pub fn reserved_identifiers() -> BTreeSet<&'static str> {
    [
        "do",
        "if",
        "in",
        "for",
        "let",
        "new",
        "try",
        "var",
        "case",
        "else",
        "enum",
        "eval",
        "null",
        "this",
        "true",
        "void",
        "with",
        "await",
        "break",
        "catch",
        "class",
        "const",
        "false",
        "super",
        "throw",
        "while",
        "yield",
        "delete",
        "export",
        "import",
        "public",
        "return",
        "static",
        "switch",
        "typeof",
        "default",
        "extends",
        "finally",
        "package",
        "private",
        "continue",
        "debugger",
        "function",
        "arguments",
        "interface",
        "protected",
        "implements",
        "instanceof",
    ]
    .into_iter()
    .collect()
}
