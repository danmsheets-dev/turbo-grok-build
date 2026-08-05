//! Per-desk monitor HUD text.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::state::DeskSlot;

fn trunc(s: &str, max: usize) -> String {
    trunc_chars(s.chars(), max)
}

/// Truncate to `max` display columns (ellipsis if it did not fit) and pad the
/// remainder with spaces, so every monitor row is exactly `max` wide.
///
/// PERF (RC16 P12): takes an iterator so the marquee can be rotated straight
/// into the output instead of collecting an intermediate `String`; measures
/// with `UnicodeWidthChar` instead of allocating a `String` per character; and
/// tracks the running width instead of re-measuring the whole output on every
/// pad step (that loop was quadratic in `max`).
///
/// The running width counts `max(1)` cell per char — the same accounting
/// `render::blit_lines` uses when it paints the row — so a zero-width mark now
/// pads to the columns actually consumed rather than to the string's nominal
/// display width.
fn trunc_chars(chars: impl Iterator<Item = char>, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(max);
    let mut w = 0;
    for ch in chars {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        if w + cw > max {
            if w + 1 <= max {
                out.push('…');
                w += 1;
            }
            break;
        }
        out.push(ch);
        w += cw;
    }
    // Pad to width for clean borders
    for _ in w..max {
        out.push(' ');
    }
    out
}

fn fmt_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    let m = secs / 60;
    let s = secs % 60;
    if m >= 60 {
        format!("{}h{:02}m", m / 60, m % 60)
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Smallest count that renders as `1.0M` / `1.0B`.
///
/// `{:.1}` rounds to nearest, so promoting on the raw unit boundary printed
/// `1000.0k` for everything from 999_950 up (RC16 B13). Promote on the
/// *rounded* boundary instead so the digits always match the suffix. The `k`
/// tier needs no such constant — below 1000 the count prints in full.
const TOK_M_MIN: u64 = 999_950;
const TOK_B_MIN: u64 = 999_950_000;

fn fmt_tokens(n: u64) -> String {
    if n >= TOK_B_MIN {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= TOK_M_MIN {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Build monitor lines for an occupied desk. `width` is usable cols inside the desk rect.
pub fn monitor_lines(desk: &DeskSlot, width: u16, tick: u64) -> Vec<Line<'static>> {
    let w = width as usize;
    if w < 6 {
        return vec![];
    }

    let border = Style::default().fg(Color::Rgb(60, 70, 80));
    let screen = Style::default()
        .fg(Color::Rgb(80, 255, 120))
        .bg(Color::Rgb(20, 30, 35));
    let dim = Style::default()
        .fg(Color::Rgb(120, 200, 140))
        .bg(Color::Rgb(20, 30, 35));
    let title_s = Style::default()
        .fg(Color::Rgb(200, 220, 255))
        .bg(Color::Rgb(20, 30, 35))
        .add_modifier(Modifier::BOLD);

    let inner = w.saturating_sub(2).max(1);
    let ty = trunc(&desk.subagent_type, inner);

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("┌", border),
        Span::styled(ty, title_s),
        Span::styled("┐", border),
    ]));

    if inner >= 8 {
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(trunc(&fmt_duration(desk.elapsed), inner), screen),
            Span::styled("│", border),
        ]));
    }
    if inner >= 12 {
        let tok = fmt_tokens(desk.tokens);
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(trunc(&format!("{tok} tok"), inner), dim),
            Span::styled("│", border),
        ]));
    }
    if inner >= 14 {
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(trunc(&format!("{} tools", desk.tool_calls), inner), dim),
            Span::styled("│", border),
        ]));
    }
    if inner >= 6 {
        // The marquee rotates straight into the truncated row (RC16 P12): the
        // rotated copy used to be collected into a `String` that `trunc` then
        // immediately threw away, every paint, for every occupied desk.
        let act = if desk.activity.is_empty() {
            trunc("…", inner)
        } else {
            let raw = &desk.activity;
            let len = raw.chars().count().max(1);
            let shift = (tick as usize / 2) % len;
            trunc_chars(raw.chars().cycle().skip(shift).take(len), inner)
        };
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(act, screen),
            Span::styled("│", border),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("└", border),
        Span::styled("─".repeat(inner), border),
        Span::styled("┘", border),
    ]));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B13: the unit must never disagree with the digits — `{:.1}` rounding
    /// used to print `1000.0k` / `1000.0M` just below each boundary.
    #[test]
    fn fmt_tokens_promotes_on_the_rounded_boundary() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(1000), "1.0k");
        assert_eq!(fmt_tokens(999_949), "999.9k");
        assert_eq!(fmt_tokens(999_950), "1.0M");
        assert_eq!(fmt_tokens(1_000_000), "1.0M");
        assert_eq!(fmt_tokens(999_949_999), "999.9M");
        assert_eq!(fmt_tokens(999_950_000), "1.0B");
        assert_eq!(fmt_tokens(1_000_000_000), "1.0B");
    }

    /// No rendered token string may carry a mantissa of 1000 or more: that is
    /// the exact shape of the bug, at every tier.
    #[test]
    fn fmt_tokens_never_renders_a_thousand_of_a_unit() {
        for n in [
            999_949_u64,
            999_950,
            999_999,
            999_949_999,
            999_950_000,
            u64::MAX / 2,
        ] {
            let s = fmt_tokens(n);
            assert!(
                !s.starts_with("1000."),
                "fmt_tokens({n}) = {s:?} should have promoted a unit"
            );
        }
    }

    /// Cell width of a row as `render::blit_lines` counts it (`max(1)` per char).
    fn painted_cells(s: &str) -> usize {
        s.chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0).max(1))
            .sum()
    }

    /// P12: `trunc` measures and pads without allocating per character. The
    /// contract the monitor borders depend on is unchanged — every row is
    /// exactly `max` painted cells, and a wide glyph never straddles the edge.
    #[test]
    fn trunc_pads_and_clips_to_exactly_max_cells() {
        for max in [0_usize, 1, 2, 5, 12] {
            for s in ["", "ab", "general-purpose", "日本語テスト", "a…b", "🏆x"] {
                let out = trunc(s, max);
                assert_eq!(
                    painted_cells(&out),
                    max,
                    "trunc({s:?}, {max}) = {out:?} is not {max} cells wide"
                );
            }
        }
        assert_eq!(trunc("abc", 5), "abc  ");
        assert_eq!(trunc("abcdefg", 5), "abcde");
        // A wide glyph that would straddle the edge yields to the ellipsis —
        // the only case that produces one, since a 1-col char that fits exactly
        // is kept instead.
        assert_eq!(trunc("日本語", 5), "日本…");
    }

    /// Activity row text for a desk at `tick`, at a 20-col desk (`inner` = 18).
    fn activity_row(desk: &DeskSlot, tick: u64) -> String {
        let lines = monitor_lines(desk, 20, tick);
        lines[lines.len() - 2].spans[1].content.to_string()
    }

    /// The marquee rotates through `trunc_chars` now; it must still step one
    /// char every other tick and stay exactly `inner` cells wide.
    #[test]
    fn marquee_rotates_without_changing_row_width() {
        let desk = DeskSlot {
            child_session_id: Some("child-1".to_string()),
            subagent_type: "general-purpose".to_string(),
            activity: "Running: cargo build".to_string(),
            ..DeskSlot::default()
        };
        let t0 = activity_row(&desk, 0);
        assert_eq!(painted_cells(&t0), 18, "activity row must fill `inner`");
        assert_eq!(
            activity_row(&desk, 1),
            t0,
            "tick/2 bucket: an odd tick does not step the marquee"
        );
        let t2 = activity_row(&desk, 2);
        assert_ne!(t2, t0, "the marquee must advance on the next tick/2 bucket");
        assert_eq!(painted_cells(&t2), 18, "rotation must not change row width");
    }

    /// Empty activity still renders a full-width placeholder row.
    #[test]
    fn empty_activity_row_is_still_padded() {
        let desk = DeskSlot {
            child_session_id: Some("child-1".to_string()),
            ..DeskSlot::default()
        };
        let row = activity_row(&desk, 0);
        assert!(
            row.starts_with('…'),
            "row {row:?} must start with the ellipsis"
        );
        assert_eq!(painted_cells(&row), 18, "row {row:?} must fill `inner`");
    }
}
