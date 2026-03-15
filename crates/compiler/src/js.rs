use crate::ast::modern::Expression;
use oxc_codegen::{Codegen, CodegenOptions};

pub(crate) trait Render {
    fn render(&self) -> Option<String>;
}

pub(crate) fn codegen_options() -> CodegenOptions {
    CodegenOptions {
        single_quote: true,
        ..CodegenOptions::default()
    }
}

pub(crate) fn render<T: Render + ?Sized>(value: &T) -> Option<String> {
    value.render()
}

impl Render for Expression {
    fn render(&self) -> Option<String> {
        let mut rendered = if let Some(expression) = self.oxc_expression() {
            let mut codegen = Codegen::new().with_options(codegen_options());
            codegen.print_expression(expression);
            codegen.into_source_text()
        } else if let Some(source) = self.source_snippet() {
            source.trim().to_string()
        } else {
            return None;
        };
        // Strip TypeScript non-null assertions (expr! → expr)
        rendered = strip_ts_non_null_assertions(&rendered);
        for _ in 0..self.parens() {
            rendered = format!("({rendered})");
        }
        Some(rendered)
    }
}

/// Strip TypeScript non-null assertion operators (`!`) from rendered JS output.
/// TS non-null assertion: `expr!` where `!` follows `)`, `]`, or identifier char,
/// AND is followed by `;`, `)`, `,`, `.`, `[`, `\n`, whitespace, or end-of-string.
/// Must NOT be `!=` or `!==`.
fn strip_ts_non_null_assertions(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'!' && i > 0 {
            let prev = bytes[i - 1];
            let next = bytes.get(i + 1).copied();
            // Not `!=` or `!==`
            if next == Some(b'=') {
                result.push('!');
            }
            // TS non-null: after `)`, `]`, or identifier char
            // AND before `;`, `)`, `,`, `.`, `[`, newline, space, or end
            else if (prev == b')' || prev == b']' || prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'$')
                && (next.is_none() || matches!(next, Some(b';' | b')' | b',' | b'.' | b'[' | b'\n' | b'\r' | b' ' | b'\t')))
            {
                // Skip — TS non-null assertion
            } else {
                result.push('!');
            }
        } else {
            result.push(bytes[i] as char);
        }
        i += 1;
    }
    result
}
