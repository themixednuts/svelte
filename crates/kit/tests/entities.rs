use svelte_kit::decode_entities;

#[test]
fn decodes_upstream_entity_cases() {
    let tests = [
        ("&amp;amp;", "&amp;"),
        ("&amp;#38;", "&#38;"),
        ("&amp;#x26;", "&#x26;"),
        ("&amp;#X26;", "&#X26;"),
        ("&#38;#38;", "&#38;"),
        ("&#x26;#38;", "&#38;"),
        ("&#X26;#38;", "&#38;"),
        ("&#x3a;", ":"),
        ("&#x3A;", ":"),
        ("&#X3a;", ":"),
        ("&#X3A;", ":"),
        ("&>", "&>"),
        ("id=770&#anchor", "id=770&#anchor"),
    ];

    for (input, output) in tests {
        assert_eq!(decode_entities(input), output, "{input}");
    }
}

#[test]
fn decodes_partial_legacy_entities() {
    assert_eq!(decode_entities("&timesbar"), "×bar");
    assert_eq!(
        decode_entities("?&image_uri=1&ℑ=2&image=3"),
        "?&image_uri=1&ℑ=2&image=3"
    );
    assert_eq!(decode_entities("&ampa"), "&a");
    assert_eq!(decode_entities("&nbsp<"), "\u{00A0}<");
}
