use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::{Result, SyntaxError};

pub fn parse_module_syntax(source: &str) -> Result<()> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::mjs()).parse();

    if let Some(error) = parsed.errors.into_iter().next() {
        let message = error.to_string();
        let normalized = if message.contains("EOF") {
            "Unexpected end of input".to_string()
        } else {
            message
        };
        return Err(SyntaxError::ParseModule {
            message: normalized,
        }
        .into());
    }

    Ok(())
}
