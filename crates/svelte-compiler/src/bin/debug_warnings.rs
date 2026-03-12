use std::path::PathBuf;

use svelte_compiler::{CompileOptions, ErrorMode, FragmentStrategy, GenerateTarget, compile};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: debug_warnings <input.svelte>");
        std::process::exit(2);
    };

    let source = std::fs::read_to_string(&path).expect("read source");
    let options = CompileOptions {
        filename: Some(PathBuf::from(&path).try_into().expect("utf8 path")),
        generate: GenerateTarget::None,
        error_mode: ErrorMode::Error,
        fragments: FragmentStrategy::Html,
        ..CompileOptions::default()
    };

    match compile(&source, options) {
        Ok(result) => {
            println!("warnings: {}", result.warnings.len());
            for warning in result.warnings.iter() {
                let start = warning
                    .start
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |s| format!("{}:{}", s.line, s.column));
                let end = warning
                    .end
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |s| format!("{}:{}", s.line, s.column));
                println!("{} {}..{} {}", warning.code, start, end, warning.message);
            }
        }
        Err(err) => {
            eprintln!("error {} {}", err.code, err.message);
            std::process::exit(1);
        }
    }
}
