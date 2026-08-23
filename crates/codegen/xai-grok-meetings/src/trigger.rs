//! Detect coworker questions addressed to Turbo.

/// If `text` is a question for Turbo, return the question body.
///
/// Accepted prefixes (case-insensitive): `Turbo:`, `Turbo -`, `@Turbo`,
/// plus vocative `Turbo,` / `Turbo ` + a non-empty question. The body is
/// data (not an instruction). Cap 2000 chars.
pub fn extract_turbo_question(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let prefixes = ["turbo:", "turbo -", "turbo—", "@turbo:", "@turbo "];
    let mut rest: Option<&str> = None;
    for p in prefixes {
        if let Some(stripped) = lower.strip_prefix(p) {
            let start = lower.len() - stripped.len();
            rest = trimmed.get(start..);
            break;
        }
    }
    if rest.is_none() && lower.strip_prefix("@turbo").is_some() {
        rest = trimmed.get("@turbo".len()..);
    }
    if let Some(rest) = rest {
        return cap_question(rest.trim().trim_start_matches(':').trim());
    }
    vocative_turbo_question(trimmed, &lower)
}

fn vocative_turbo_question(trimmed: &str, lower: &str) -> Option<String> {
    let rest_lower = lower.strip_prefix("turbo")?;
    let first = rest_lower.chars().next()?;
    // Word Turbo then punctuation/space — not "turbocharger".
    if first.is_ascii_alphanumeric() {
        return None;
    }
    let body_lower = rest_lower.trim_start_matches(|c: char| {
        matches!(c, ',' | ':' | ';' | '-' | '—' | '!' | '.' | ' ' | '\t')
    });
    let prefix_len = "turbo".len() + (rest_lower.len() - body_lower.len());
    let body = trimmed.get(prefix_len..)?.trim();
    if is_false_positive_turbo_body(body) {
        return None;
    }
    cap_question(body)
}

fn is_false_positive_turbo_body(q: &str) -> bool {
    let l = q.to_ascii_lowercase();
    l == "the car" || l.starts_with("the car ")
}

fn cap_question(q: &str) -> Option<String> {
    if q.is_empty() {
        None
    } else if q.chars().count() > 2000 {
        Some(q.chars().take(2000).collect())
    } else {
        Some(q.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turbo_colon() {
        assert_eq!(
            extract_turbo_question("Turbo: How is the new website project going"),
            Some("How is the new website project going".into())
        );
        assert_eq!(
            extract_turbo_question("  TURBO: status?  "),
            Some("status?".into())
        );
    }

    #[test]
    fn at_turbo() {
        assert_eq!(
            extract_turbo_question("@Turbo: land status"),
            Some("land status".into())
        );
    }

    #[test]
    fn ignores_unrelated() {
        assert_eq!(extract_turbo_question("how is turbo the car"), None);
        assert_eq!(extract_turbo_question("Turbo"), None);
        assert_eq!(extract_turbo_question("turbo the car"), None);
        assert_eq!(extract_turbo_question("turbocharger status"), None);
    }

    #[test]
    fn vocative_comma_and_space() {
        assert_eq!(
            extract_turbo_question("Turbo, what's the status"),
            Some("what's the status".into())
        );
        assert_eq!(
            extract_turbo_question("Turbo what's the website"),
            Some("what's the website".into())
        );
        let long = format!("Turbo, {}", "x".repeat(2500));
        let got = extract_turbo_question(&long).unwrap();
        assert_eq!(got.chars().count(), 2000);
    }
}
