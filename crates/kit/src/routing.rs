use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, RoutingError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteParam {
    pub name: String,
    pub matcher: Option<String>,
    pub optional: bool,
    pub rest: bool,
    pub chained: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedRouteId {
    pub pattern: Regex,
    pub params: Vec<RouteParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundRoute<'a, R> {
    pub route: &'a R,
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Static,
    Required,
    Optional,
    Rest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Part {
    kind: PartKind,
    content: String,
    matched: bool,
}

pub fn parse_route_id(id: &str) -> Result<ParsedRouteId> {
    if id == "/" {
        return Ok(ParsedRouteId {
            pattern: Regex::new(r"^/$").map_err(|source| Error::Regex {
                route_id: id.to_string(),
                source,
            })?,
            params: Vec::new(),
        });
    }

    let mut params = Vec::new();
    let mut pattern = String::from("^");

    for segment in get_route_segments(id) {
        if let Some((name, matcher)) = parse_full_rest(segment) {
            params.push(RouteParam {
                name,
                matcher,
                optional: false,
                rest: true,
                chained: true,
            });
            pattern.push_str(r"(?:/(.*))?");
            continue;
        }

        if let Some((name, matcher)) = parse_full_optional(segment) {
            params.push(RouteParam {
                name,
                matcher,
                optional: true,
                rest: false,
                chained: true,
            });
            pattern.push_str(r"(?:/([^/]+))?");
            continue;
        }

        if segment.is_empty() {
            continue;
        }

        pattern.push('/');
        pattern.push_str(&build_segment_pattern(segment, &mut params)?);
    }

    pattern.push_str("/?$");

    let compiled = Regex::new(&pattern).map_err(|source| Error::Regex {
        route_id: id.to_string(),
        source,
    })?;

    Ok(ParsedRouteId {
        pattern: compiled,
        params,
    })
}

pub fn remove_optional_params(id: &str) -> String {
    let mut route = String::new();

    for segment in get_route_segments(id) {
        if segment.starts_with("[[") && segment.ends_with("]]") {
            continue;
        }

        route.push('/');
        route.push_str(&segment);
    }

    if route.is_empty() {
        "/".to_string()
    } else {
        route
    }
}

pub fn sort_routes<T>(routes: &mut [T], route_id: impl Fn(&T) -> &str) {
    let empty = Part {
        kind: PartKind::Static,
        content: String::new(),
        matched: false,
    };

    routes.sort_by(|left, right| {
        if route_id(left) == "/" {
            return std::cmp::Ordering::Less;
        }
        if route_id(right) == "/" {
            return std::cmp::Ordering::Greater;
        }

        let left_segments = split_sort_route_id(route_id(left));
        let right_segments = split_sort_route_id(route_id(right));

        for index in 0..left_segments.len().max(right_segments.len()) {
            let left_parts = left_segments.get(index);
            let right_parts = right_segments.get(index);
            let left_segment = left_parts.cloned().unwrap_or_else(|| vec![empty.clone()]);
            let right_segment = right_parts.cloned().unwrap_or_else(|| vec![empty.clone()]);

            for part_index in 0..left_segment.len().max(right_segment.len()) {
                let left_part = left_segment.get(part_index);
                let right_part = right_segment.get(part_index);
                let dynamic = part_index % 2 == 1;

                if dynamic {
                    match (left_part, right_part) {
                        (None, None) => {}
                        (None, Some(_)) => return std::cmp::Ordering::Less,
                        (Some(_), None) => return std::cmp::Ordering::Greater,
                        (Some(left_part), Some(right_part)) => {
                            let next_left = left_segment
                                .get(part_index + 1)
                                .map(|part| part.content.as_str())
                                .filter(|content| !content.is_empty())
                                .or_else(|| {
                                    left_segments
                                        .get(index + 1)
                                        .and_then(|segment| segment.first())
                                        .map(|part| part.content.as_str())
                                })
                                .unwrap_or("");
                            let next_right = right_segment
                                .get(part_index + 1)
                                .map(|part| part.content.as_str())
                                .filter(|content| !content.is_empty())
                                .or_else(|| {
                                    right_segments
                                        .get(index + 1)
                                        .and_then(|segment| segment.first())
                                        .map(|part| part.content.as_str())
                                })
                                .unwrap_or("");

                            if left_part.kind == PartKind::Rest && right_part.kind == PartKind::Rest
                            {
                                if !next_left.is_empty() && next_right.is_empty() {
                                    return std::cmp::Ordering::Less;
                                }
                                if next_left.is_empty() && !next_right.is_empty() {
                                    return std::cmp::Ordering::Greater;
                                }
                                continue;
                            }

                            if left_part.kind == PartKind::Rest {
                                return if !next_left.is_empty() && next_right.is_empty() {
                                    std::cmp::Ordering::Less
                                } else {
                                    std::cmp::Ordering::Greater
                                };
                            }

                            if right_part.kind == PartKind::Rest {
                                return if !next_right.is_empty() && next_left.is_empty() {
                                    std::cmp::Ordering::Greater
                                } else {
                                    std::cmp::Ordering::Less
                                };
                            }

                            if left_part.matched != right_part.matched {
                                return if left_part.matched {
                                    std::cmp::Ordering::Less
                                } else {
                                    std::cmp::Ordering::Greater
                                };
                            }

                            if left_part.kind != right_part.kind {
                                if left_part.kind == PartKind::Required {
                                    return std::cmp::Ordering::Less;
                                }
                                if right_part.kind == PartKind::Required {
                                    return std::cmp::Ordering::Greater;
                                }
                            }
                        }
                    }
                } else if left_part.map(|part| part.content.as_str())
                    != right_part.map(|part| part.content.as_str())
                {
                    if left_parts.is_none() {
                        return std::cmp::Ordering::Less;
                    }
                    if right_parts.is_none() {
                        return std::cmp::Ordering::Greater;
                    }

                    match (left_part, right_part) {
                        (Some(left_part), Some(right_part)) => {
                            let ordering = sort_static(&left_part.content, &right_part.content);
                            if ordering != std::cmp::Ordering::Equal {
                                return ordering;
                            }
                        }
                        _ => unreachable!(
                            "missing segments should be handled before comparing parts"
                        ),
                    }
                }
            }
        }

        route_id(right).cmp(route_id(left))
    });
}

pub fn exec_route_match<F>(
    captures: &regex::Captures<'_>,
    params: &[RouteParam],
    mut matches: F,
) -> Option<BTreeMap<String, String>>
where
    F: FnMut(&str, &str) -> bool,
{
    let mut result = BTreeMap::new();
    let values = captures
        .iter()
        .skip(1)
        .map(|value| value.map(|value| value.as_str().to_string()))
        .collect::<Vec<_>>();
    let values_needing_match = values.iter().filter(|value| value.is_some()).count();
    let mut buffered = 0usize;

    for (index, param) in params.iter().enumerate() {
        let mut value = values
            .get(index.saturating_sub(buffered))
            .cloned()
            .flatten();

        if param.chained && param.rest && buffered > 0 {
            value = Some(
                values[index - buffered..=index]
                    .iter()
                    .filter_map(|value| value.as_deref())
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
            buffered = 0;
        }

        if value.is_none() {
            if param.rest {
                value = Some(String::new());
            } else {
                continue;
            }
        }

        let value = value.expect("rest params should default to an empty string");
        let matched = param
            .matcher
            .as_deref()
            .is_none_or(|matcher| matches(matcher, &value));

        if matched {
            result.insert(param.name.clone(), value);

            let next_param = params.get(index + 1);
            let next_value = values.get(index + 1).and_then(|value| value.as_ref());
            if next_param.is_some_and(|next_param| {
                !next_param.rest && next_param.optional && next_value.is_some() && param.chained
            }) {
                buffered = 0;
            }

            if next_param.is_none() && next_value.is_none() && result.len() == values_needing_match
            {
                buffered = 0;
            }

            continue;
        }

        if param.optional && param.chained {
            buffered += 1;
            continue;
        }

        return None;
    }

    if buffered > 0 {
        return None;
    }

    Some(result)
}

pub fn resolve_route(id: &str, params: &BTreeMap<String, String>) -> Result<String> {
    let segments = get_route_segments(id);
    let has_trailing_slash = id != "/" && id.ends_with('/');
    let mut resolved = Vec::new();

    for segment in segments {
        let mut output = String::new();
        let mut cursor = 0usize;

        while cursor < segment.len() {
            let rest = &segment[cursor..];
            let Some(open_rel) = rest.find('[') else {
                output.push_str(rest);
                break;
            };

            let open = cursor + open_rel;
            output.push_str(&segment[cursor..open]);

            let optional = segment[open..].starts_with("[[");
            let close = find_closing_bracket(segment, open, optional).ok_or_else(|| {
                RoutingError::InvalidRouteSegment {
                    segment: segment.to_string(),
                }
            })?;
            let inner = if optional {
                &segment[open + 2..close - 1]
            } else {
                &segment[open + 1..close]
            };
            let (rest, name, _) =
                parse_param(inner).ok_or_else(|| RoutingError::InvalidRouteParameterInSegment {
                    segment: segment.to_string(),
                })?;

            let value = params.get(&name);
            match value {
                Some(value) if value.is_empty() && (optional || rest) => {}
                Some(value) if !value.is_empty() => {
                    if value.starts_with('/') || value.ends_with('/') {
                        return Err(RoutingError::ParameterStartsOrEndsWithSlash {
                            name,
                            route_id: id.to_string(),
                        }
                        .into());
                    }
                    output.push_str(value);
                }
                Some(_) if optional => {}
                Some(_) if rest => {}
                Some(_) | None if optional => {}
                Some(_) | None if rest && value.is_some() => {}
                _ => {
                    return Err(RoutingError::MissingRouteParameter {
                        name,
                        route_id: id.to_string(),
                    }
                    .into());
                }
            }

            cursor = close + 1;
        }

        if !output.is_empty() {
            resolved.push(output);
        }
    }

    let mut pathname = format!("/{}", resolved.join("/"));
    if pathname == "/" && !resolved.is_empty() {
        pathname.push_str(&resolved.join("/"));
    }
    if resolved.is_empty() {
        pathname = "/".to_string();
    }
    if has_trailing_slash && pathname != "/" {
        pathname.push('/');
    }

    Ok(pathname)
}

pub fn find_route<'a, R, P, F>(
    path: &str,
    routes: &'a [R],
    parsed: P,
    mut matches: F,
) -> Option<FoundRoute<'a, R>>
where
    P: Fn(&'a R) -> (&Regex, &[RouteParam]),
    F: FnMut(&str, &str) -> bool,
{
    for route in routes {
        let (pattern, params) = parsed(route);
        let Some(captures) = pattern.captures(path) else {
            continue;
        };
        let Some(params) =
            exec_route_match(&captures, params, |matcher, value| matches(matcher, value))
        else {
            continue;
        };

        let decoded = params
            .into_iter()
            .map(|(key, value)| decode_param(&value).map(|value| (key, value)))
            .collect::<Option<BTreeMap<_, _>>>()?;

        return Some(FoundRoute {
            route,
            params: decoded,
        });
    }

    None
}

pub fn get_route_segments(route: &str) -> Vec<&str> {
    route
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty() && !is_group(segment))
        .collect()
}

fn build_segment_pattern(segment: &str, params: &mut Vec<RouteParam>) -> Result<String> {
    let mut pattern = String::new();
    let mut cursor = 0usize;
    let mut dynamic_index = 0usize;

    while cursor < segment.len() {
        let rest = &segment[cursor..];
        let Some(open_rel) = rest.find('[') else {
            pattern.push_str(&escape_route_text(rest));
            break;
        };

        let open = cursor + open_rel;
        let static_part = &segment[cursor..open];
        pattern.push_str(&escape_route_text(static_part));

        let optional = segment[open..].starts_with("[[");
        let close = find_closing_bracket(segment, open, optional).ok_or_else(|| {
            RoutingError::InvalidRouteSegment {
                segment: segment.to_string(),
            }
        })?;
        let raw = &segment[open..=close];

        if !optional {
            if let Some(decoded) = decode_escape(raw) {
                pattern.push_str(&escape_route_text(&decoded));
                cursor = close + 1;
                continue;
            }
        }

        let inner = if optional {
            &segment[open + 2..close - 1]
        } else {
            &segment[open + 1..close]
        };

        let (rest, name, matcher) =
            parse_param(inner).ok_or_else(|| RoutingError::InvalidRawRouteParameter {
                raw: raw.to_string(),
                segment: segment.to_string(),
            })?;
        let chained = rest && dynamic_index == 0 && static_part.is_empty();

        params.push(RouteParam {
            name,
            matcher,
            optional,
            rest,
            chained,
        });

        pattern.push_str(if rest {
            "(.*?)"
        } else if optional {
            "([^/]*)?"
        } else {
            "([^/]+?)"
        });

        dynamic_index += 1;
        cursor = close + 1;
    }

    Ok(pattern)
}

fn split_sort_route_id(id: &str) -> Vec<Vec<Part>> {
    let trimmed = strip_mid_optionals(id);
    get_route_segments(&trimmed)
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .map(split_sort_segment)
        .collect()
}

fn split_sort_segment(segment: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut cursor = 0usize;

    while cursor <= segment.len() {
        if cursor == segment.len() {
            parts.push(Part {
                kind: PartKind::Static,
                content: String::new(),
                matched: false,
            });
            break;
        }

        let rest = &segment[cursor..];
        let Some(open_rel) = rest.find('[') else {
            parts.push(Part {
                kind: PartKind::Static,
                content: rest.to_string(),
                matched: false,
            });
            break;
        };

        let open = cursor + open_rel;
        parts.push(Part {
            kind: PartKind::Static,
            content: segment[cursor..open].to_string(),
            matched: false,
        });

        let optional = segment[open..].starts_with("[[");
        let close = find_closing_bracket(segment, open, optional).unwrap_or(segment.len() - 1);
        let content = segment[open..=close].to_string();
        let kind = if optional {
            PartKind::Optional
        } else if segment[open + 1..].starts_with("...") {
            PartKind::Rest
        } else {
            PartKind::Required
        };

        parts.push(Part {
            kind,
            matched: content.contains('='),
            content,
        });

        cursor = close + 1;
    }

    if parts.is_empty() {
        parts.push(Part {
            kind: PartKind::Static,
            content: String::new(),
            matched: false,
        });
    }

    parts
}

fn parse_full_rest(segment: &str) -> Option<(String, Option<String>)> {
    if !segment.starts_with("[...") || !segment.ends_with(']') || segment.starts_with("[[") {
        return None;
    }

    let inner = &segment[4..segment.len() - 1];
    let (name, matcher) = split_name_matcher(inner)?;
    Some((name.to_string(), matcher.map(str::to_string)))
}

fn parse_full_optional(segment: &str) -> Option<(String, Option<String>)> {
    if !segment.starts_with("[[") || !segment.ends_with("]]") {
        return None;
    }

    let inner = &segment[2..segment.len() - 2];
    let (name, matcher) = split_name_matcher(inner)?;
    Some((name.to_string(), matcher.map(str::to_string)))
}

fn parse_param(inner: &str) -> Option<(bool, String, Option<String>)> {
    if let Some(rest) = inner.strip_prefix("...") {
        let (name, matcher) = split_name_matcher(rest)?;
        return Some((true, name.to_string(), matcher.map(str::to_string)));
    }

    let (name, matcher) = split_name_matcher(inner)?;
    Some((false, name.to_string(), matcher.map(str::to_string)))
}

fn split_name_matcher(value: &str) -> Option<(&str, Option<&str>)> {
    let mut parts = value.splitn(2, '=');
    let name = parts.next()?;
    if !is_ident(name) {
        return None;
    }
    let matcher = parts.next();
    if matcher.is_some_and(|matcher| !is_ident(matcher)) {
        return None;
    }
    Some((name, matcher))
}

fn is_ident(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

fn decode_escape(raw: &str) -> Option<String> {
    let inner = raw.strip_prefix('[')?.strip_suffix(']')?;
    if let Some(hex) = inner.strip_prefix("x+") {
        if hex.len() != 2 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        return char::from_u32(value).map(|ch| ch.to_string());
    }

    let unicode = inner.strip_prefix("u+")?;
    let mut text = String::new();
    for part in unicode.split('-') {
        if !(4..=6).contains(&part.len()) || !part.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(part, 16).ok()?;
        text.push(char::from_u32(value)?);
    }
    Some(text)
}

fn find_closing_bracket(segment: &str, open: usize, optional: bool) -> Option<usize> {
    if optional {
        segment[open + 2..]
            .find("]]")
            .map(|index| open + 2 + index + 1)
    } else {
        segment[open + 1..].find(']').map(|index| open + 1 + index)
    }
}

fn escape_route_text(value: &str) -> String {
    let mut pattern = String::new();

    for ch in value.chars() {
        match ch {
            '%' => pattern.push_str("%25"),
            '/' => pattern.push_str("%2[Ff]"),
            '?' => pattern.push_str("%3[Ff]"),
            '#' => pattern.push_str("%23"),
            _ => pattern.push_str(&regex::escape(&ch.to_string())),
        }
    }

    pattern
}

fn strip_mid_optionals(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    let mut cursor = 0usize;

    while let Some(open_rel) = id[cursor..].find("[[") {
        let open = cursor + open_rel;
        let close = match id[open + 2..].find("]]") {
            Some(close_rel) => open + 2 + close_rel + 1,
            None => break,
        };
        let tail = &id[close + 1..];
        let keep = tail.is_empty()
            || tail
                .split('/')
                .all(|segment| segment.is_empty() || is_group(segment));

        out.push_str(&id[cursor..open]);
        if keep {
            out.push_str(&id[open..=close]);
        }
        cursor = close + 1;
    }

    out.push_str(&id[cursor..]);
    out
}

fn is_group(segment: &str) -> bool {
    segment.starts_with('(') && segment.ends_with(')')
}

fn sort_static(left: &str, right: &str) -> std::cmp::Ordering {
    if left == right {
        return std::cmp::Ordering::Equal;
    }

    let mut left_chars = left.chars();
    let mut right_chars = right.chars();

    loop {
        match (left_chars.next(), right_chars.next()) {
            (Some(left_char), Some(right_char)) if left_char == right_char => continue,
            (Some(left_char), Some(right_char)) => return left_char.cmp(&right_char),
            (None, Some(_)) => return std::cmp::Ordering::Greater,
            (Some(_), None) => return std::cmp::Ordering::Less,
            (None, None) => return std::cmp::Ordering::Equal,
        }
    }
}

fn decode_param(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = decode_hex(bytes[index + 1])?;
            let low = decode_hex(bytes[index + 2])?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).ok()
}

fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
