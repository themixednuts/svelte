use svelte_kit::{escape_for_interpolation, escape_html_utf16, escape_html_with_mode};

#[test]
fn escape_html_attr_escapes_special_attribute_characters() {
    assert_eq!(
        escape_html_with_mode("some \"values\" are &special here, <others> aren't.", true),
        "some &quot;values&quot; are &amp;special here, <others> aren't."
    );
}

#[test]
fn escape_html_attr_escapes_invalid_surrogates() {
    assert_eq!(escape_html_utf16(&[0xD800, 0xDC00], true), "\u{10000}");
    assert_eq!(escape_html_utf16(&[0xD800], true), "&#55296;");
    assert_eq!(escape_html_utf16(&[0xDC00], true), "&#56320;");
    assert_eq!(
        escape_html_utf16(&[0xDC00, 0xD800], true),
        "&#56320;&#55296;"
    );
    assert_eq!(
        escape_html_utf16(&[0xD800, 0xD800, 0xDC00], true),
        "&#55296;\u{10000}"
    );
    assert_eq!(
        escape_html_utf16(&[0xD800, 0xDC00, 0xDC00], true),
        "\u{10000}&#56320;"
    );
    assert_eq!(
        escape_html_utf16(&[0xD800, 0xD800, 0xDC00, 0xDC00], true),
        "&#55296;\u{10000}&#56320;"
    );
}

#[test]
fn escape_for_interpolation_escapes_backticks_and_dollar_signs() {
    assert_eq!(
        escape_for_interpolation("div:after { content: \"` and ${example}`\"; }"),
        "div:after { content: \"\\` and \\${example}\\`\"; }"
    );
}
