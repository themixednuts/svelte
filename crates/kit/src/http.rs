pub const BINARY_FORM_CONTENT_TYPE: &str = "application/x-sveltekit-formdata";

#[derive(Debug)]
struct AcceptPart<'a> {
    type_: &'a str,
    subtype: &'a str,
    q: f64,
    index: usize,
}

pub fn negotiate<'a>(accept: &str, types: &'a [&str]) -> Option<&'a str> {
    let mut parts = accept
        .split(',')
        .enumerate()
        .filter_map(|(index, part)| parse_accept_part(part.trim(), index))
        .collect::<Vec<_>>();

    parts.sort_by(|a, b| {
        b.q.total_cmp(&a.q)
            .then_with(|| wildcard_rank(a.subtype).cmp(&wildcard_rank(b.subtype)))
            .then_with(|| wildcard_rank(a.type_).cmp(&wildcard_rank(b.type_)))
            .then_with(|| a.index.cmp(&b.index))
    });

    let mut accepted = None;
    let mut min_priority = usize::MAX;

    for mimetype in types {
        let Some((type_, subtype)) = mimetype.split_once('/') else {
            continue;
        };
        let priority = parts.iter().position(|part| {
            (part.type_ == type_ || part.type_ == "*")
                && (part.subtype == subtype || part.subtype == "*")
        });

        if let Some(priority) = priority {
            if priority < min_priority {
                accepted = Some(*mimetype);
                min_priority = priority;
            }
        }
    }

    accepted
}

pub fn is_form_content_type(content_type: Option<&str>) -> bool {
    is_content_type(
        content_type,
        &[
            "application/x-www-form-urlencoded",
            "multipart/form-data",
            "text/plain",
            BINARY_FORM_CONTENT_TYPE,
        ],
    )
}

fn is_content_type(content_type: Option<&str>, types: &[&str]) -> bool {
    let type_ = content_type
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    types.contains(&type_.as_str())
}

fn parse_accept_part(part: &str, index: usize) -> Option<AcceptPart<'_>> {
    let (mimetype, params) = part.split_once(';').unwrap_or((part, ""));
    let (type_, subtype) = mimetype.trim().split_once('/')?;

    if type_.is_empty() || subtype.is_empty() {
        return None;
    }

    let mut q = 1.0;
    for parameter in params.split(';') {
        let parameter = parameter.trim();
        if let Some(value) = parameter.strip_prefix("q=") {
            q = value.parse::<f64>().ok()?;
            break;
        }
    }

    Some(AcceptPart {
        type_,
        subtype,
        q,
        index,
    })
}

fn wildcard_rank(value: &str) -> usize {
    usize::from(value == "*")
}
