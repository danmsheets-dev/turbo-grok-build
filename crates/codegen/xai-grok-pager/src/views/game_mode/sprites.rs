//! Unicode sprite frames for office characters and props.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::layout::SpriteSet;
use super::state::{ActorPhase, SupervisorPhase};

/// Centre `s` in `w` **display** columns.
///
/// Sprite rows are box-drawn art blitted left-aligned as a block, so every row
/// of one sprite has to measure the same width or the walls stagger. Byte or
/// char counts do not work here: the faces and props mix ambiguous-width
/// glyphs (`◕`, `░`), zero-width combining marks (`•̀`) and wide emoji (`📄`).
/// Oversized text is returned unchanged rather than truncated mid-glyph.
fn pad_center(s: &str, w: usize) -> String {
    let sw = UnicodeWidthStr::width(s);
    if sw >= w {
        return s.to_string();
    }
    let left = (w - sw) / 2;
    format!("{}{s}{}", " ".repeat(left), " ".repeat(w - sw - left))
}

/// Developer skin palettes (shirt / skin).
pub fn skin_colors(skin: u8) -> (Color, Color) {
    match skin % 6 {
        0 => (Color::Rgb(220, 80, 70), Color::Rgb(240, 180, 140)), // red horns vibe
        1 => (Color::Rgb(70, 160, 220), Color::Rgb(180, 200, 240)), // blue
        2 => (Color::Rgb(200, 180, 60), Color::Rgb(200, 160, 200)), // purple/yellow
        3 => (Color::Rgb(80, 180, 90), Color::Rgb(160, 220, 150)), // green
        4 => (Color::Rgb(90, 100, 200), Color::Rgb(150, 140, 210)), // indigo
        _ => (Color::Rgb(80, 180, 200), Color::Rgb(240, 190, 120)), // cyan/orange
    }
}

pub fn floor_style() -> Style {
    Style::default()
        .fg(Color::Rgb(40, 90, 95))
        .bg(Color::Rgb(30, 70, 75))
}

pub fn wall_bg() -> Style {
    Style::default()
        .fg(Color::Rgb(160, 165, 170))
        .bg(Color::Rgb(90, 95, 100))
}

pub fn desk_wood() -> Style {
    Style::default()
        .fg(Color::Rgb(160, 110, 60))
        .bg(Color::Rgb(120, 80, 40))
}

pub fn rug_style() -> Style {
    Style::default()
        .fg(Color::Rgb(140, 60, 90))
        .bg(Color::Rgb(100, 40, 65))
}

/// Supervisor multi-line sprite (medium).
pub fn supervisor_lines(phase: SupervisorPhase, tick: u64, set: SpriteSet) -> Vec<Line<'static>> {
    let gold = Style::default()
        .fg(Color::Rgb(255, 200, 60))
        .add_modifier(Modifier::BOLD);
    let body = Style::default().fg(Color::Rgb(240, 210, 120));
    let hands = match phase {
        SupervisorPhase::Working => {
            if tick % 4 < 2 {
                "⌨ ░"
            } else {
                "⌨ ▒"
            }
        }
        SupervisorPhase::Reviewing => "📄👀",
        SupervisorPhase::Waiting => "☕  ",
        SupervisorPhase::Idle => "    ",
    };
    let face = match phase {
        SupervisorPhase::Reviewing => "(◕‿◕)",
        SupervisorPhase::Working => "(•̀ᴗ•́)",
        _ => "(◕‿◕)",
    };
    match set {
        SpriteSet::Small => vec![
            Line::from(vec![Span::styled("  ∩∩  ", gold)]),
            Line::from(vec![Span::styled(format!(" {face}"), body), Span::raw(" ")]),
            Line::from(vec![Span::styled(format!(" {hands}"), body)]),
        ],
        // Every row is exactly `SUPER_W` display columns: `hands` is 3-4 cols
        // wide depending on phase, so the desk front used to fall 2-3 columns
        // short of the `╔═SUPER═╗` header and the right wall stepped inwards
        // (RC2 B8). Head and face are centred over the same box.
        SpriteSet::Medium => vec![
            Line::from(vec![Span::styled(pad_center("∩   ∩", SUPER_W), gold)]),
            Line::from(vec![Span::styled(pad_center(face, SUPER_W), body)]),
            Line::from(vec![Span::styled(" ╔═SUPER═╗ ", gold)]),
            Line::from(vec![Span::styled(
                format!(" ║{}║ ", pad_center(hands, SUPER_W - 4)),
                body,
            )]),
            Line::from(vec![Span::styled(" ╚═══════╝ ", desk_wood())]),
        ],
    }
}

/// Display width of every [`supervisor_lines`] row in [`SpriteSet::Medium`] —
/// the `╔═SUPER═╗` header plus its one-column margins.
const SUPER_W: usize = 11;

/// Developer sprite at desk (or walking placeholder glyph).
pub fn developer_lines(
    phase: ActorPhase,
    skin: u8,
    tick: u64,
    set: SpriteSet,
) -> Vec<Line<'static>> {
    let (shirt, face_c) = skin_colors(skin);
    let shirt_s = Style::default().fg(shirt).add_modifier(Modifier::BOLD);
    let face_s = Style::default().fg(face_c);
    let chair = Style::default().fg(Color::Rgb(60, 60, 70));

    let face = match phase {
        ActorPhase::AtDeskThinking => "·_·",
        ActorPhase::Celebrate => "★_★",
        ActorPhase::FailBeat => "x_x",
        ActorPhase::Handoff => "•_•",
        _ => {
            if tick % 6 < 3 {
                "•_•"
            } else {
                "•‿•"
            }
        }
    };
    let arms = match phase {
        ActorPhase::AtDeskWorking | ActorPhase::SpawnWalk => {
            if tick % 4 < 2 {
                "⌐■"
            } else {
                "¬■"
            }
        }
        ActorPhase::AtDeskThinking => " ? ",
        ActorPhase::Celebrate => "\\o/",
        ActorPhase::FailBeat => " > ",
        ActorPhase::WalkToBoss | ActorPhase::ExitDoor => {
            if tick % 2 == 0 {
                " 🚶"
            } else {
                " 🏃"
            }
        }
        ActorPhase::Handoff => " 📄",
    };

    match set {
        SpriteSet::Small => vec![
            Line::from(vec![Span::styled(format!(" {face}"), face_s)]),
            Line::from(vec![
                Span::styled(format!("{arms}"), shirt_s),
                Span::styled("█", chair),
            ]),
        ],
        SpriteSet::Medium => vec![
            Line::from(vec![Span::styled(format!("  {face}  "), face_s)]),
            Line::from(vec![
                Span::styled(format!(" {arms} "), shirt_s),
                Span::styled("▓", chair),
            ]),
            Line::from(vec![Span::styled("  █▓█  ", chair)]),
        ],
    }
}

/// Empty desk art.
pub fn empty_desk_lines(set: SpriteSet) -> Vec<Line<'static>> {
    let wood = desk_wood();
    let dim = Style::default().fg(Color::Rgb(80, 90, 95));
    match set {
        // 8 columns wide: the box is sized to its `IDLE` label, which is 4
        // columns and used to overhang a 3-column box by one, pushing the
        // right wall out of line with the rows above and below (RC2 B7).
        SpriteSet::Small => vec![
            Line::from(vec![Span::styled("  ░░░░  ", dim)]),
            Line::from(vec![Span::styled(" ┌────┐ ", wood)]),
            Line::from(vec![Span::styled(" │IDLE│ ", dim)]),
            Line::from(vec![Span::styled(" └─┬──┘ ", wood)]),
        ],
        SpriteSet::Medium => vec![
            Line::from(vec![Span::styled("    ░░░░    ", dim)]),
            Line::from(vec![Span::styled("  ┌──────┐  ", wood)]),
            Line::from(vec![Span::styled("  │ IDLE │  ", dim)]),
            Line::from(vec![Span::styled(
                "  │  ▄▄  │  ",
                Style::default().fg(Color::Rgb(40, 40, 50)),
            )]),
            Line::from(vec![Span::styled("  └──┬───┘  ", wood)]),
            Line::from(vec![Span::styled("    ─┴─     ", chair_style())]),
        ],
    }
}

fn chair_style() -> Style {
    Style::default().fg(Color::Rgb(50, 50, 60))
}

/// Plant prop.
pub fn plant_span() -> Span<'static> {
    Span::styled("☘", Style::default().fg(Color::Rgb(60, 180, 80)))
}

/// Door glyph column.
pub fn door_lines(height: u16) -> Vec<Line<'static>> {
    let s = Style::default()
        .fg(Color::Rgb(140, 100, 60))
        .bg(Color::Rgb(90, 60, 30));
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(" DOOR ", s)));
    for _ in 1..height.max(1) {
        lines.push(Line::from(Span::styled(" ║  ░║ ", s)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display width of each row of a sprite, spans summed.
    fn row_widths(lines: &[Line<'static>]) -> Vec<usize> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                    .sum()
            })
            .collect()
    }

    /// B7: sprite rows are blitted as a left-aligned block, so a row wider than
    /// its own box borders steps the right wall out. Measured in display
    /// columns — `░` is three bytes and one column.
    #[test]
    fn empty_desk_rows_are_rectangular() {
        for set in [SpriteSet::Small, SpriteSet::Medium] {
            let w = row_widths(&empty_desk_lines(set));
            assert!(
                w.windows(2).all(|p| p[0] == p[1]),
                "{set:?} empty-desk rows are ragged: {w:?}"
            );
        }
    }

    /// B8: the supervisor's desk front is built from a phase-dependent `hands`
    /// run (3-4 columns, including wide emoji) and must still fill the
    /// `╔═SUPER═╗` box on every phase and animation frame.
    #[test]
    fn medium_supervisor_rows_match_the_super_box() {
        for phase in [
            SupervisorPhase::Idle,
            SupervisorPhase::Working,
            SupervisorPhase::Reviewing,
            SupervisorPhase::Waiting,
        ] {
            for tick in 0..4u64 {
                let w = row_widths(&supervisor_lines(phase, tick, SpriteSet::Medium));
                assert!(
                    w.iter().all(|c| *c == SUPER_W),
                    "{phase:?} tick {tick}: rows {w:?} != {SUPER_W}"
                );
            }
        }
    }

    #[test]
    fn pad_center_measures_display_width_not_bytes() {
        // 4 display columns from 8 bytes of emoji.
        assert_eq!(pad_center("📄👀", 6), " 📄👀 ");
        // Oversized input is never truncated mid-glyph.
        assert_eq!(pad_center("📄👀", 2), "📄👀");
    }
}
