use svelte_kit::{has_children, should_ignore};

fn should_warn_for_content(content: &str, filename: &str) -> bool {
    let basename = std::path::Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .expect("filename should have basename");

    if !(basename.starts_with("+page.") || basename.starts_with("+layout.")) {
        return false;
    }

    let mut match_index = None;
    for option in ["prerender", "csr", "ssr", "trailingSlash"] {
        let needle = format!("export const {option}");
        if let Some(index) = content.find(&needle) {
            match_index = Some(index);
            break;
        }
    }

    match match_index {
        Some(index) => !should_ignore(content, index),
        None => false,
    }
}

#[test]
fn ignores_page_option_exports_inside_comments_and_strings() {
    let cases = [
        ("// export const trailingSlash = \"always\"", false),
        ("/* export const trailingSlash = \"always\" */", false),
        ("<!-- export const trailingSlash = \"always\" -->", false),
        ("\"export const trailingSlash = true\"", false),
        ("`${42}export const trailingSlash = true`", false),
        ("export const trailingSlash = \"always\"", true),
        ("// comment\nexport const trailingSlash = \"always\"", true),
        (
            "\"/*\"; export const trailingSlash = \"always\"; \"*/\"",
            true,
        ),
    ];

    for (content, expected) in cases {
        assert_eq!(
            should_warn_for_content(content, "+page.svelte"),
            expected,
            "content: {content}"
        );
    }
}

#[test]
fn detects_children_rendering_in_layout_content() {
    let cases = [
        ("{@render children()}", true),
        ("<slot />", true),
        ("<slot name=\"default\" />", true),
        (
            "<script>\n\tlet { children } = $props();\n</script>\n<Layout {children} />",
            true,
        ),
        (
            "<script>\n\tlet { children } = $props();\n</script>\n<Layout children={children} />",
            true,
        ),
        (
            "<script>\n\tlet { data } = $props();\n</script>\n<div>{data}</div>",
            false,
        ),
        ("", false),
    ];

    for (content, expected) in cases {
        assert_eq!(has_children(content, true), expected, "content: {content}");
    }
}
