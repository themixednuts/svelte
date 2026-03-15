use std::collections::BTreeSet;

use svelte_kit::{CssUrlRewriteOptions, fix_css_urls, tippex_comments_and_strings};

#[test]
fn fixes_css_urls_like_upstream() {
    let cdn_assets = "https://cdn.example.com/_app/immutable/assets";
    let cdn_base = "https://cdn.example.com";
    let local_assets = "./_app/immutable/assets";
    let local_base = ".";

    let cases = vec![
        (
            "uses paths.assets for vite assets",
            "div { background: url(./image.png); }",
            format!("div {{ background: url({cdn_assets}/image.png); }}"),
            vec!["image.png"],
            vec![],
            cdn_assets,
            cdn_base,
        ),
        (
            "uses paths.base for static assets",
            "div { background: url(../../../image.png); }",
            format!("div {{ background: url({cdn_base}/image.png); }}"),
            vec![],
            vec!["image.png"],
            cdn_assets,
            cdn_base,
        ),
        (
            "keeps quotes",
            "div { background: url('./image.png#section'); }",
            format!("div {{ background: url('{local_assets}/image.png#section'); }}"),
            vec!["image.png"],
            vec![],
            local_assets,
            local_base,
        ),
        (
            "handles multiple urls",
            "div { background: image-set(url(./a.png) 1x, url(./b.png) 2x); }",
            format!(
                "div {{ background: image-set(url({local_assets}/a.png) 1x, url({local_assets}/b.png) 2x); }}"
            ),
            vec!["a.png", "b.png"],
            vec![],
            local_assets,
            local_base,
        ),
        (
            "ignores absolute urls",
            "div { background: url(/absolute/image.png); }",
            "div { background: url(/absolute/image.png); }".to_string(),
            vec!["image.png"],
            vec![],
            local_assets,
            local_base,
        ),
        (
            "ignores urls inside strings",
            "div::after { content: 'url(./image.png)'; }",
            "div::after { content: 'url(./image.png)'; }".to_string(),
            vec!["image.png"],
            vec![],
            local_assets,
            local_base,
        ),
        (
            "ignores urls inside comments",
            "div::before { content: \"/*\"; } div { background: blue /* url(./image.png) */; }",
            "div::before { content: \"/*\"; } div { background: blue /* url(./image.png) */; }"
                .to_string(),
            vec!["image.png"],
            vec![],
            local_assets,
            local_base,
        ),
    ];

    for (name, css, expected, vite_assets, static_assets, paths_assets, base) in cases {
        let vite_assets = vite_assets
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let static_assets = static_assets
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let actual = fix_css_urls(CssUrlRewriteOptions {
            css,
            vite_assets: &vite_assets,
            static_assets: &static_assets,
            paths_assets,
            base,
            static_asset_prefix: "../../../",
        });
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn tippexes_comments_and_strings_like_upstream() {
    let cases = [
        ("'hello'", "'     '"),
        ("\"hello\"", "\"     \""),
        ("/* comment */", "/*         */"),
        ("'it\\'s'", "'     '"),
        ("content: 'url(./fake.png)'", "content: '               '"),
        (
            "/* url(./x.png) */ url(./y.png)",
            "/*              */ url(./y.png)",
        ),
        ("'unterminated", "'            "),
        ("/* unterminated", "/*             "),
    ];

    for (input, expected) in cases {
        let actual = tippex_comments_and_strings(input);
        assert_eq!(actual, expected, "{input}");
        assert_eq!(actual.len(), input.len(), "{input}");
    }
}
