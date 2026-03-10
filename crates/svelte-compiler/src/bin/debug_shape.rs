use std::env;
use std::fs;

use svelte_compiler::{ParseMode, ParseOptions, parse};

fn main() {
    let path = env::args()
        .nth(1)
        .expect("usage: debug_shape <path-to-svelte>");
    let source = fs::read_to_string(&path).expect("read source");
    let ast = parse(
        &source,
        ParseOptions {
            mode: ParseMode::Modern,
            loose: false,
            ..Default::default()
        },
    )
    .expect("parse");

    println!("{}", serde_json::to_string_pretty(&ast).expect("json"));
}
