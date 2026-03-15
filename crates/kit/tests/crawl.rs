use std::fs;

use camino::Utf8PathBuf;
use svelte_kit::{CrawlResult, crawl};

fn repo_root() -> Utf8PathBuf {
    let manifest_dir = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .find(|candidate| candidate.join("kit").join("packages").join("kit").is_dir())
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn crawls_upstream_fixture_corpus() {
    let fixtures = repo_root()
        .join("kit")
        .join("packages")
        .join("kit")
        .join("src")
        .join("core")
        .join("postbuild")
        .join("fixtures");

    for entry in fs::read_dir(&fixtures).expect("read fixture root") {
        let entry = entry.expect("fixture entry");
        if !entry.file_type().expect("fixture type").is_dir() {
            continue;
        }

        let fixture = Utf8PathBuf::from_path_buf(entry.path()).expect("utf8 fixture path");
        let input = fs::read_to_string(fixture.join("input.html")).expect("read fixture input");
        let expected =
            fs::read_to_string(fixture.join("output.json")).expect("read fixture output");
        let expected: CrawlResult =
            serde_json::from_str(&expected).expect("parse expected crawl output");

        let actual = crawl(&input, "/").expect("crawl fixture");
        assert_eq!(
            actual,
            expected,
            "crawl fixture {} did not match",
            fixture.file_name().expect("fixture name"),
        );
    }
}
