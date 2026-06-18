use dynamo_service_stock::Error;
use scraper::{Html, Selector};

pub(crate) fn build_consent_form_body(body: &str) -> Result<String, Error> {
    let selector = Selector::parse(r#"input[type="hidden"]"#)
        .map_err(|error| anyhow::anyhow!("Failed to build Yahoo consent selector: {error}"))?;
    let document = Html::parse_document(body);
    let mut pairs = Vec::new();

    for input in document.select(&selector) {
        let Some(name) = input.value().attr("name") else {
            continue;
        };
        let Some(value) = input.value().attr("value") else {
            continue;
        };
        pairs.push((name.to_string(), decode_html_entities(value)));
    }
    pairs.push(("agree".to_string(), "agree".to_string()));
    pairs.push(("agree".to_string(), "agree".to_string()));

    if pairs.is_empty() {
        return Err(anyhow::anyhow!(
            "Yahoo consent page did not contain any hidden form inputs"
        ));
    }

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(&name, &value);
    }
    Ok(serializer.finish())
}

pub(crate) fn decode_html_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '&' && chars.peek() == Some(&'#') {
            let _ = chars.next();
            if chars.peek() == Some(&'x') {
                let _ = chars.next();
                let mut hex = String::new();
                while let Some(next) = chars.peek() {
                    if *next == ';' {
                        let _ = chars.next();
                        break;
                    }
                    hex.push(*next);
                    let _ = chars.next();
                }
                if let Ok(codepoint) = u32::from_str_radix(&hex, 16)
                    && let Some(decoded) = char::from_u32(codepoint)
                {
                    output.push(decoded);
                    continue;
                }
                output.push('&');
                output.push('#');
                output.push('x');
                output.push_str(&hex);
                output.push(';');
                continue;
            }
            output.push('&');
            output.push('#');
            continue;
        }

        output.push(ch);
    }

    output
}
