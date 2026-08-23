//! Parse `/schedule` `at` values: ISO-8601 datetimes and weekday clocks.

use chrono::{DateTime, Datelike, Local, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};

use super::types::SchedulerError;

const MINIMUM_INTERVAL_SECS: u64 = 60;

/// A parsed `at` / weekday clock specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtSpec {
    /// One-shot fire at this instant (UTC).
    Once(DateTime<Utc>),
    /// Every Monday–Friday at `time` (local).
    Weekdays(NaiveTime),
    /// Every matching weekday at `time` (local).
    Weekly(Weekday, NaiveTime),
}

/// Parse an `at` string: RFC3339 / naive local datetime, or a weekday clock.
///
/// Accepted datetime forms: `2026-08-24T09:00`, `2026-08-24T09:00:00`,
/// `2026-08-24T09:00:00Z`, offset RFC3339, `2026-08-24 09:00`.
/// Naive values are local time.
///
/// Weekday clocks: `weekday 08:00`, `every weekday 08:00`, `monday 09:00`.
pub fn parse_at(s: &str) -> Result<AtSpec, SchedulerError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(SchedulerError::InvalidAt("at cannot be empty".into()));
    }
    if let Some(spec) = parse_weekday_clock(s) {
        return Ok(spec);
    }
    parse_datetime(s).map(AtSpec::Once)
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>, SchedulerError> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    const NAIVE_FMTS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ];
    for fmt in NAIVE_FMTS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return local_naive_to_utc(naive);
        }
    }
    Err(SchedulerError::InvalidAt(format!(
        "invalid datetime {s:?} (expected ISO-8601 / 2026-08-24T09:00)"
    )))
}

fn local_naive_to_utc(naive: NaiveDateTime) -> Result<DateTime<Utc>, SchedulerError> {
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .ok_or_else(|| {
            SchedulerError::InvalidAt(format!(
                "ambiguous or invalid local time {naive}"
            ))
        })
}

fn parse_weekday_clock(s: &str) -> Option<AtSpec> {
    let mut rest = s.trim();
    if let Some(stripped) = strip_prefix_ci(rest, "every ") {
        rest = stripped.trim();
    }
    let (day_token, time_token) = rest.split_once(char::is_whitespace)?;
    let time = parse_clock(time_token.trim())?;
    match day_token.to_ascii_lowercase().as_str() {
        "weekday" | "weekdays" | "mon-fri" => Some(AtSpec::Weekdays(time)),
        "monday" | "mon" => Some(AtSpec::Weekly(Weekday::Mon, time)),
        "tuesday" | "tue" | "tues" => Some(AtSpec::Weekly(Weekday::Tue, time)),
        "wednesday" | "wed" => Some(AtSpec::Weekly(Weekday::Wed, time)),
        "thursday" | "thu" | "thur" | "thurs" => Some(AtSpec::Weekly(Weekday::Thu, time)),
        "friday" | "fri" => Some(AtSpec::Weekly(Weekday::Fri, time)),
        "saturday" | "sat" => Some(AtSpec::Weekly(Weekday::Sat, time)),
        "sunday" | "sun" => Some(AtSpec::Weekly(Weekday::Sun, time)),
        _ => None,
    }
}

fn parse_clock(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M"))
        .ok()
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Seconds until `then`, clamped to at least 60. Errors if `then` is not in the future.
pub fn seconds_until(now: DateTime<Utc>, then: DateTime<Utc>) -> Result<u64, SchedulerError> {
    let delta = (then - now).num_seconds();
    if delta <= 0 {
        return Err(SchedulerError::InvalidAt(
            "at time must be in the future".into(),
        ));
    }
    Ok((delta as u64).max(MINIMUM_INTERVAL_SECS))
}

/// Next local weekday (Mon–Fri) at `time` strictly after `now`.
pub fn next_weekday_clock(now: DateTime<Utc>, time: NaiveTime) -> DateTime<Utc> {
    let mut candidate = local_on_date(now, time);
    if candidate <= now || is_local_weekend(candidate) {
        // Walk forward at most 8 days (weekend + today already passed).
        for _ in 0..8 {
            candidate += chrono::Duration::days(1);
            if candidate > now && !is_local_weekend(candidate) {
                break;
            }
        }
    }
    candidate
}

/// Next local `weekday` at `time` strictly after `now`.
pub fn next_weekly_clock(
    now: DateTime<Utc>,
    weekday: Weekday,
    time: NaiveTime,
) -> DateTime<Utc> {
    let mut candidate = local_on_date(now, time);
    for _ in 0..8 {
        if candidate > now && candidate.with_timezone(&Local).weekday() == weekday {
            return candidate;
        }
        candidate += chrono::Duration::days(1);
    }
    candidate
}

fn local_on_date(now: DateTime<Utc>, time: NaiveTime) -> DateTime<Utc> {
    let local = now.with_timezone(&Local);
    local
        .date_naive()
        .and_time(time)
        .and_local_timezone(Local)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or(now)
}

pub fn is_local_weekend(dt: DateTime<Utc>) -> bool {
    matches!(
        dt.with_timezone(&Local).weekday(),
        Weekday::Sat | Weekday::Sun
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_naive_datetime() {
        let spec = parse_at("2026-08-24T09:00").unwrap();
        let AtSpec::Once(dt) = spec else {
            panic!("expected Once");
        };
        let local = dt.with_timezone(&Local);
        assert_eq!(local.format("%Y-%m-%dT%H:%M").to_string(), "2026-08-24T09:00");
    }

    #[test]
    fn parse_rfc3339() {
        let spec = parse_at("2026-08-24T09:00:00Z").unwrap();
        let AtSpec::Once(dt) = spec else {
            panic!("expected Once");
        };
        assert_eq!(dt, DateTime::parse_from_rfc3339("2026-08-24T09:00:00Z").unwrap().with_timezone(&Utc));
    }

    #[test]
    fn parse_weekday_clocks() {
        assert!(matches!(parse_at("weekday 08:00").unwrap(), AtSpec::Weekdays(_)));
        assert!(matches!(
            parse_at("every weekday 08:00").unwrap(),
            AtSpec::Weekdays(_)
        ));
        assert!(matches!(
            parse_at("monday 09:30").unwrap(),
            AtSpec::Weekly(Weekday::Mon, _)
        ));
        assert!(matches!(
            parse_at("every friday 17:00").unwrap(),
            AtSpec::Weekly(Weekday::Fri, _)
        ));
    }

    #[test]
    fn future_at_is_in_the_future() {
        let then = Utc::now() + chrono::Duration::hours(2);
        let secs = seconds_until(Utc::now(), then).unwrap();
        assert!(secs >= 3600);
        assert!(secs <= 2 * 3600 + 2);
    }

    #[test]
    fn past_at_errors() {
        let then = Utc::now() - chrono::Duration::minutes(5);
        let err = seconds_until(Utc::now(), then).unwrap_err();
        assert!(err.to_string().contains("future"));
    }

    #[test]
    fn delay_under_60s_clamps() {
        let then = Utc::now() + chrono::Duration::seconds(10);
        assert_eq!(seconds_until(Utc::now(), then).unwrap(), 60);
    }

    #[test]
    fn next_weekday_skips_weekend() {
        // 2026-08-22 is a Saturday.
        let sat = Local
            .with_ymd_and_hms(2026, 8, 22, 10, 0, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);
        let eight = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let next = next_weekday_clock(sat, eight);
        let local = next.with_timezone(&Local);
        assert_eq!(local.weekday(), Weekday::Mon);
        assert_eq!(local.time(), eight);
    }
}
