use std::fs;
use std::io;
use std::process::{Command, Stdio};

use serde::de::DeserializeOwned;

use crate::FixtureCase;

pub fn load_test_config<T: DeserializeOwned>(case: &FixtureCase) -> io::Result<Option<T>> {
    let path = case.path.join("_config.js");
    if !path.exists() {
        return Ok(None);
    }

    let source = fs::read_to_string(&path)?;
    let transformed = transform_config_source(&source);
    evaluate_config_source(&transformed, case)
}

fn transform_config_source(source: &str) -> String {
    let mut imported_names: Vec<String> = Vec::new();
    let mut body = String::new();

    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import ") {
            imported_names.extend(parse_import_names(trimmed));
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }

    let mut output = String::new();
    output.push_str(
        "const __dummy = new Proxy(function () { return __dummy; }, { get: () => __dummy, apply: () => __dummy, construct: () => __dummy });\n",
    );
    output.push_str("const test = (value) => value;\n");

    imported_names.sort();
    imported_names.dedup();
    for name in imported_names {
        if name == "test" || name.is_empty() {
            continue;
        }
        output.push_str("const ");
        output.push_str(&name);
        output.push_str(" = __dummy;\n");
    }

    let mut replaced = body.replacen("export default", "globalThis.__export_default =", 1);
    for (from, to) in [
        ("export const", "const"),
        ("export let", "let"),
        ("export var", "var"),
        ("export function", "function"),
        ("export class", "class"),
    ] {
        replaced = replaced.replace(from, to);
    }
    output.push_str(&replaced);
    output
}

#[derive(serde::Deserialize)]
struct ConfigEvalResult<T> {
    ok: bool,
    value: Option<T>,
    error: Option<String>,
}

fn evaluate_config_source<T: DeserializeOwned>(
    source: &str,
    case: &FixtureCase,
) -> io::Result<Option<T>> {
    let script = r#"
import fs from 'node:fs';

function normalize(value, seen = new WeakSet()) {
  if (value === null) return null;

  const t = typeof value;
  if (t === 'string' || t === 'number' || t === 'boolean') return value;
  if (t === 'undefined') return null;
  if (t === 'bigint') return { __kind: 'bigint', value: value.toString() };
  if (t === 'function') {
    if (value.length === 0) {
      try {
        return normalize(value(), seen);
      } catch {}
    }
    return { __kind: 'function', source: value.toString() };
  }
  if (t === 'symbol') return { __kind: 'symbol', value: value.toString() };

  if (value instanceof RegExp) {
    return { __kind: 'regexp', source: value.source, flags: value.flags };
  }

  if (Array.isArray(value)) {
    return value.map((entry) => normalize(entry, seen));
  }

  if (t === 'object') {
    if (seen.has(value)) {
      return { __kind: 'circular' };
    }
    seen.add(value);
    const out = {};
    for (const key of Object.keys(value)) {
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (!descriptor) continue;
      if (Object.prototype.hasOwnProperty.call(descriptor, 'value')) {
        out[key] = normalize(descriptor.value, seen);
      } else {
        out[key] = { __kind: 'accessor' };
      }
    }
    seen.delete(value);
    return out;
  }

  return { __kind: 'unknown', value: String(value) };
}

const source = fs.readFileSync(0, 'utf8');

try {
  eval(source);
  process.stdout.write(JSON.stringify({ ok: true, value: normalize(globalThis.__export_default) }));
} catch (error) {
  process.stdout.write(JSON.stringify({ ok: false, error: String(error) }));
  process.exit(1);
}
"#;

    let mut child = Command::new("node")
        .arg("--input-type=module")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(stdin) = &mut child.stdin {
        use std::io::Write;
        stdin.write_all(source.as_bytes())?;
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "failed to evaluate _config.js for {}: status={} stdout={} stderr={}",
            case.name,
            output.status,
            stdout.trim(),
            stderr.trim()
        )));
    }

    let result: ConfigEvalResult<T> = serde_json::from_slice(&output.stdout)
        .map_err(|error| io::Error::other(format!("invalid config JSON output: {error}")))?;

    if !result.ok {
        let message = result
            .error
            .as_deref()
            .unwrap_or("unknown config evaluation error");
        return Err(io::Error::other(format!(
            "failed to evaluate _config.js for {}: {message}",
            case.name
        )));
    }

    Ok(result.value)
}

fn parse_import_names(import_line: &str) -> Vec<String> {
    let line = import_line.trim().trim_end_matches(';');

    if !line.starts_with("import ") {
        return Vec::new();
    }

    if line.starts_with("import '") || line.starts_with("import \"") {
        return Vec::new();
    }

    let Some((left, _)) = line[7..].split_once(" from ") else {
        return Vec::new();
    };

    parse_import_clause(left)
}

fn parse_import_clause(clause: &str) -> Vec<String> {
    let clause = clause.trim();
    if clause.is_empty() {
        return Vec::new();
    }

    if let Some(rest) = clause.strip_prefix("*") {
        if let Some(alias) = rest.trim().strip_prefix("as ") {
            return vec![sanitize_ident(alias)];
        }
    }

    if clause.starts_with('{') {
        return parse_named_imports(clause);
    }

    if let Some((default_name, named)) = clause.split_once(',') {
        let mut names = vec![sanitize_ident(default_name)];
        names.extend(parse_named_imports(named));
        return names;
    }

    vec![sanitize_ident(clause)]
}

fn parse_named_imports(clause: &str) -> Vec<String> {
    let trimmed = clause.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(trimmed);

    inner
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            if let Some((_, alias)) = entry.split_once(" as ") {
                sanitize_ident(alias)
            } else {
                sanitize_ident(entry)
            }
        })
        .collect()
}

fn sanitize_ident(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '$')
        .collect()
}
