//! Detect coworker questions addressed to Turbo.

/// If `text` is a question for Turbo, return the question body.
///
/// Accepted prefixes (case-insensitive): `Turbo:`, `Turbo -`, `@Turbo`.
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
    let rest = rest?;
    let q = rest.trim().trim_start_matches(':').trim();
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
    }
}
