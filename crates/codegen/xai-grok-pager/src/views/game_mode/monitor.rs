//! Per-desk monitor HUD text.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::state::DeskSlot;

fn trunc(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str()).max(1);
        if w + cw > max {
            if w + 1 <= max {
                out.push('…');
            }
            break;
        }
        out.push(ch);
        w += cw;
    }
    // Pad to width for clean borders
    while UnicodeWidthStr::width(out.as_str()) < max {
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

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
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
    let elapsed = fmt_duration(desk.elapsed);
    let tok = fmt_tokens(desk.tokens);
    let tools = desk.tool_calls.to_string();

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("┌", border),
        Span::styled(ty, title_s),
        Span::styled("┐", border),
    ]));

    if inner >= 8 {
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(trunc(&elapsed, inner), screen),
            Span::styled("│", border),
        ]));
    }
    if inner >= 12 {
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(trunc(&format!("{tok} tok"), inner), dim),
            Span::styled("│", border),
        ]));
    }
    if inner >= 14 {
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(trunc(&format!("{tools} tools"), inner), dim),
            Span::styled("│", border),
        ]));
    }
    if inner >= 6 {
        let act = if desk.activity.is_empty() {
            "…".to_string()
        } else {
            let raw = &desk.activity;
            let len = raw.chars().count().max(1);
            let shift = (tick as usize / 2) % len;
            raw.chars().cycle().skip(shift).take(len).collect()
        };
        lines.push(Line::from(vec![
            Span::styled("│", border),
            Span::styled(trunc(&act, inner), screen),
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
