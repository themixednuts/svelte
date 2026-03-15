use std::collections::BTreeMap;
use std::fs;

use camino::Utf8PathBuf;
use svelte_kit::{CopyOptions, copy, mkdirp, resolve_entry};

fn temp_dir(name: &str) -> Utf8PathBuf {
    let dir = std::env::temp_dir().join(format!("kit-filesystem-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    Utf8PathBuf::from_path_buf(dir).expect("utf8 temp dir")
}

fn write(source_dir: &Utf8PathBuf, file: &str, contents: &str) {
    let path = source_dir.join(file);
    if let Some(parent) = path.parent() {
        mkdirp(parent).expect("create source parent");
    }
    fs::write(path, contents).expect("write source file");
}

#[test]
fn copies_without_filter() {
    let root = temp_dir("copy-basic");
    let source = root.join("source");
    let dest = root.join("dest");
    mkdirp(&source).expect("create source");
    mkdirp(&dest).expect("create dest");

    write(&source, "file-one.js", "");
    write(&source, "file-two.css", "");
    write(&source, "file-three", "");

    copy(&source, &dest, &CopyOptions::default()).expect("copy tree");

    let mut copied: Vec<_> = fs::read_dir(&dest)
        .expect("read copied dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    copied.sort();
    assert_eq!(copied, vec!["file-one.js", "file-three", "file-two.css"]);
}

#[test]
fn filters_out_subdirectory_contents() {
    let root = temp_dir("copy-filter");
    let source = root.join("source");
    let dest = root.join("dest");
    mkdirp(&source).expect("create source");
    mkdirp(&dest).expect("create dest");

    write(&source, "file-one.js", "");
    write(&source, "file-two.css", "");
    write(&source, "no-copy/do-not-copy.js", "");

    let options = CopyOptions {
        filter: Some(|name| name != "no-copy"),
        replace: BTreeMap::new(),
    };
    copy(&source, &dest, &options).expect("copy filtered tree");

    let mut copied: Vec<_> = fs::read_dir(&dest)
        .expect("read copied dir")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    copied.sort();
    assert_eq!(copied, vec!["file-one.js", "file-two.css"]);
}

#[test]
fn copies_recursively_and_returns_relative_files() {
    let root = temp_dir("copy-recursive");
    let source = root.join("source");
    let dest = root.join("dest");
    mkdirp(&source).expect("create source");
    mkdirp(&dest).expect("create dest");

    write(&source, "file-one.js", "");
    write(&source, "file-two.css", "");
    write(&source, "deep/a.js", "");
    write(&source, "deep/b.js", "");

    let mut copied = copy(&source, &dest, &CopyOptions::default()).expect("copy tree");
    copied.sort();
    assert_eq!(
        copied,
        vec!["deep/a.js", "deep/b.js", "file-one.js", "file-two.css"]
    );

    let file_copy = copy(
        &source.join("file-one.js"),
        &dest.join("file-one-renamed.js"),
        &CopyOptions::default(),
    )
    .expect("copy file");
    assert_eq!(file_copy, vec!["file-one-renamed.js"]);
}

#[test]
fn copies_with_replacements() {
    let root = temp_dir("copy-replace");
    let source = root.join("source");
    let dest = root.join("dest");
    mkdirp(&source).expect("create source");
    mkdirp(&dest).expect("create dest");

    write(
        &source,
        "foo.md",
        "the quick brown JUMPER jumps over the lazy JUMPEE",
    );

    let mut replace = BTreeMap::new();
    replace.insert("JUMPER".to_string(), "fox".to_string());
    replace.insert("JUMPEE".to_string(), "dog".to_string());
    let options = CopyOptions {
        filter: None,
        replace,
    };
    copy(&source, &dest, &options).expect("copy with replacements");

    assert_eq!(
        fs::read_to_string(dest.join("foo.md")).expect("read copied file"),
        "the quick brown fox jumps over the lazy dog"
    );
}

#[test]
fn resolves_entries_like_upstream() {
    let root = temp_dir("resolve-entry");
    let source = root.join("source");
    mkdirp(&source).expect("create source");

    write(&source, "service-worker/index.js", "");
    assert_eq!(
        resolve_entry(&source.join("service-worker")).expect("resolve service-worker"),
        Some(source.join("service-worker/index.js"))
    );

    write(&source, "hooks.js", "");
    assert_eq!(
        resolve_entry(&source.join("hooks.js")).expect("resolve hooks.js"),
        Some(source.join("hooks.js"))
    );

    write(&source, "hooks/not-index.js", "");
    assert_eq!(
        resolve_entry(&source.join("hooks")).expect("resolve hooks path"),
        Some(source.join("hooks.js"))
    );

    let server_only = temp_dir("resolve-entry-server-only");
    write(&server_only, "hooks.server/index.js", "");
    assert_eq!(
        resolve_entry(&server_only.join("hooks")).expect("resolve missing universal hooks"),
        None
    );

    let nested = temp_dir("resolve-entry-hooks-dir");
    write(&nested, "hooks/hooks.server.js", "");
    assert_eq!(
        resolve_entry(&nested.join("hooks")).expect("resolve hooks folder"),
        None
    );
}
