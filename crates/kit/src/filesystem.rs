use std::collections::BTreeMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use regex::Regex;

use crate::{Error, Result};

#[derive(Default)]
pub struct CopyOptions {
    pub filter: Option<fn(&str) -> bool>,
    pub replace: BTreeMap<String, String>,
}

pub fn mkdirp(dir: &Utf8Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(Into::into)
}

pub fn copy(source: &Utf8Path, target: &Utf8Path, options: &CopyOptions) -> Result<Vec<String>> {
    if !source.exists() {
        return Ok(Vec::new());
    }

    let mut copied = Vec::new();
    let prefix = format!("{}/", posixify(target));
    let replacement_regex = if options.replace.is_empty() {
        None
    } else {
        Some(
            Regex::new(&format!(
                r"\b({})\b",
                options
                    .replace
                    .keys()
                    .map(|key| regex::escape(key))
                    .collect::<Vec<_>>()
                    .join("|")
            ))
            .expect("valid replacement regex"),
        )
    };

    fn walk(
        from: &Utf8Path,
        to: &Utf8Path,
        options: &CopyOptions,
        replacement_regex: Option<&Regex>,
        prefix: &str,
        copied: &mut Vec<String>,
        root_target: &Utf8Path,
    ) -> Result<()> {
        if let Some(filter) = options.filter {
            let Some(name) = from.file_name() else {
                return Ok(());
            };
            if !filter(name) {
                return Ok(());
            }
        }

        let metadata = fs::metadata(from)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(from)? {
                let entry = entry?;
                let path =
                    Utf8PathBuf::from_path_buf(entry.path()).map_err(|_| Error::InvalidUtf8Path)?;
                let target_path = to.join(entry.file_name().to_string_lossy().as_ref());
                walk(
                    &path,
                    &target_path,
                    options,
                    replacement_regex,
                    prefix,
                    copied,
                    root_target,
                )?;
            }
            return Ok(());
        }

        if let Some(parent) = to.parent() {
            mkdirp(parent)?;
        }

        if let Some(regex) = replacement_regex {
            let contents = fs::read_to_string(from)?;
            let replaced = regex.replace_all(&contents, |captures: &regex::Captures<'_>| {
                let key = captures.get(1).expect("replacement capture").as_str();
                options
                    .replace
                    .get(key)
                    .expect("replacement key should exist")
                    .clone()
            });
            fs::write(to, replaced.as_bytes())?;
        } else {
            fs::copy(from, to)?;
        }

        let relative = if to == root_target {
            posixify(
                to.file_name()
                    .expect("copied file should have a basename")
                    .to_string(),
            )
        } else {
            posixify(to).replacen(prefix, "", 1)
        };
        copied.push(relative);
        Ok(())
    }

    walk(
        source,
        target,
        options,
        replacement_regex.as_ref(),
        &prefix,
        &mut copied,
        target,
    )?;

    Ok(copied)
}

pub fn resolve_entry(entry: &Utf8Path) -> Result<Option<Utf8PathBuf>> {
    if entry.exists() {
        let metadata = fs::metadata(entry)?;
        if metadata.is_file() {
            return Ok(Some(entry.to_path_buf()));
        }

        let index = entry.join("index");
        if index.with_extension("js").exists() || index.with_extension("ts").exists() {
            return resolve_entry(&index);
        }
    }

    let Some(dir) = entry.parent() else {
        return Ok(None);
    };

    if dir.exists() {
        let base = entry
            .file_name()
            .expect("entry should have a basename")
            .to_string();

        for child in fs::read_dir(dir)? {
            let child = child?;
            let path =
                Utf8PathBuf::from_path_buf(child.path()).map_err(|_| Error::InvalidUtf8Path)?;
            let Some(stem) = path.file_stem() else {
                continue;
            };
            if stem != base {
                continue;
            }
            if fs::metadata(&path)?.is_file() {
                return Ok(Some(path));
            }
        }
    }

    Ok(None)
}

pub fn posixify(path: impl AsRef<str>) -> String {
    path.as_ref().replace('\\', "/")
}
