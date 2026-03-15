fn escape_html_units(units: &[u16], is_attr: bool) -> String {
    let mut escaped = String::new();
    let mut index = 0;

    while index < units.len() {
        let unit = units[index];

        if (0xD800..=0xDBFF).contains(&unit) {
            if let Some(&next) = units.get(index + 1) {
                if (0xDC00..=0xDFFF).contains(&next) {
                    let pair = [unit, next];
                    escaped.push_str(&String::from_utf16_lossy(&pair));
                    index += 2;
                    continue;
                }
            }

            escaped.push_str(&format!("&#{};", unit));
            index += 1;
            continue;
        }

        if (0xDC00..=0xDFFF).contains(&unit) {
            escaped.push_str(&format!("&#{};", unit));
            index += 1;
            continue;
        }

        if unit == u16::from(b'&') {
            escaped.push_str("&amp;");
        } else if is_attr && unit == u16::from(b'"') {
            escaped.push_str("&quot;");
        } else if let Some(ch) = char::from_u32(unit as u32) {
            escaped.push(ch);
        } else {
            escaped.push_str(&format!("&#{};", unit));
        }

        index += 1;
    }

    escaped
}

pub fn escape_html_with_mode(value: &str, is_attr: bool) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    escape_html_units(&units, is_attr)
}

pub fn escape_html_utf16(units: &[u16], is_attr: bool) -> String {
    escape_html_units(units, is_attr)
}

pub fn escape_for_interpolation(value: &str) -> String {
    value.replace('`', "\\`").replace('$', "\\$")
}
