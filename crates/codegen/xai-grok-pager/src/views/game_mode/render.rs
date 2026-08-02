//! Paint Game Mode into a ratatui Buffer.
//!
//! Primary path: mockup PNG + procedural sprites → halfblock raster.
//! Fallback: Unicode office (when pixel load/compose fails or Compact text-only).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::layout::{GameLayout, GameTier, SpriteSet, compute};
use super::monitor::monitor_lines;
use super::sprites::{
    developer_lines, door_lines, empty_desk_lines, floor_style, plant_span, rug_style,
    supervisor_lines, wall_bg,
};
use super::state::{ActorPhase, DeskSlot, GameModeState, SupervisorPhase};
use super::wall::WallMode;

/// Render game mode into `area` (region above composer).
pub fn render_game_mode(buf: &mut Buffer, area: Rect, state: &mut GameModeState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let layout = compute(area);

    // Pixel path: cell-res compose + direct halfblock paint (no PNG).
    let pixel_area = Rect {
        x: layout.stage.x,
        y: layout.stage.y,
        width: layout.stage.width,
        height: layout.stage.height,
    };
    let use_pixel = state.pixel_mode
        && layout.tier.uses_office_art()
        && pixel_area.width >= 40
        && pixel_area.height >= 8
        && state.ensure_pixel_frame(pixel_area.width, pixel_area.height);

    if use_pixel {
        if let Some(frame) = state.pixel_frame.as_ref() {
            let painted =
                crate::render::image_overlay::paint_halfblock_rgba(buf, pixel_area, frame);
            if !painted {
                render_unicode_office(buf, &layout, state);
            }
        } else {
            render_unicode_office(buf, &layout, state);
        }
    } else {
        render_unicode_office(buf, &layout, state);
    }

    paint_status_strip(buf, layout.status_strip, state, layout.tier);
}

fn render_unicode_office(buf: &mut Buffer, layout: &GameLayout, state: &GameModeState) {
    fill_stage_margins(buf, layout);
    paint_floor(buf, layout.content, layout.tier);

    paint_wall_display(buf, layout.wall, state.wall, state.tick, layout);
    paint_supervisor(buf, layout, state);
    paint_handoff_zone(buf, layout.handoff, state);
    for (i, desk_rect) in layout.desks.iter().enumerate() {
        paint_desk(
            buf,
            *desk_rect,
            &state.desks[i],
            layout.tier.sprite_set(),
            state.tick,
        );
    }
    if layout.tier.uses_office_art() && layout.door.width > 0 {
        paint_door(buf, layout.door, state.overflow_count);
    }
}

fn fill_stage_margins(buf: &mut Buffer, layout: &GameLayout) {
    if !layout.has_margins {
        return;
    }
    let style = wall_bg();
    for y in layout.stage.y..layout.stage.y.saturating_add(layout.stage.height) {
        for x in layout.stage.x..layout.stage.x.saturating_add(layout.stage.width) {
            if !rect_contains(layout.content, x, y) {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol("░");
                    cell.set_style(style);
                }
            }
        }
    }
}

fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && y >= r.y && x < r.x.saturating_add(r.width) && y < r.y.saturating_add(r.height)
}

fn paint_floor(buf: &mut Buffer, area: Rect, tier: GameTier) {
    let style = floor_style();
    let tile = if tier.uses_office_art() { "·" } else { " " };
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                // Checkerboard-ish
                let sym = if ((x + y) % 2) == 0 { tile } else { " " };
                cell.set_symbol(sym);
                cell.set_style(style);
            }
        }
    }
}

fn paint_wall_display(
    buf: &mut Buffer,
    area: Rect,
    mode: WallMode,
    tick: u64,
    layout: &GameLayout,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let (fg, bg) = match mode {
        WallMode::WorkFinished => (Color::Rgb(40, 255, 120), Color::Rgb(10, 40, 25)),
        WallMode::Working => (Color::Rgb(255, 220, 80), Color::Rgb(40, 35, 10)),
        WallMode::SupervisorBusy => (Color::Rgb(120, 200, 255), Color::Rgb(15, 30, 50)),
        WallMode::NeedsAttention => (Color::Rgb(255, 100, 90), Color::Rgb(50, 15, 15)),
        WallMode::WaitingOnYou => (Color::Rgb(255, 180, 80), Color::Rgb(40, 30, 10)),
        WallMode::Standby => (Color::Rgb(180, 190, 200), Color::Rgb(40, 45, 55)),
    };
    let mut style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);
    if mode.is_success_pulse() && (tick / 4) % 2 == 0 {
        style = style.add_modifier(Modifier::REVERSED);
    }

    // Fill wall strip
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(Style::default().bg(bg));
            }
        }
    }

    let title = mode.title();
    let clock = format_clock(tick);
    let title_line = if layout.tier.uses_office_art() {
        format!("══ {title} ══")
    } else {
        title.to_string()
    };
    put_line_centered(
        buf,
        area.x,
        area.y,
        area.width,
        &title_line,
        style,
    );
    if area.height >= 2 {
        put_line_right(
            buf,
            area.x,
            area.y.saturating_add(1),
            area.width,
            &format!("🕐 {clock}"),
            Style::default().fg(Color::Rgb(200, 200, 210)).bg(bg),
        );
    }
    if area.height >= 3 {
        let plants = " ☘  📚  🏆 ";
        put_line(
            buf,
            area.x.saturating_add(1),
            area.y.saturating_add(2),
            area.width.saturating_sub(2),
            plants,
            Style::default().fg(Color::Rgb(80, 180, 90)).bg(bg),
        );
    }
}

fn format_clock(tick: u64) -> String {
    // Decorative session clock from tick (not wall clock — fine for vibe).
    let secs = tick / 12; // ~15Hz → seconds-ish
    let m = secs / 60;
    let s = secs % 60;
    format!("{m:02}:{s:02}")
}

fn paint_supervisor(buf: &mut Buffer, layout: &GameLayout, state: &GameModeState) {
    let area = layout.supervisor;
    if area.width == 0 || area.height == 0 {
        return;
    }
    // Rug under supervisor for office tiers
    if layout.tier.uses_office_art() {
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    if ((x + y) % 3) == 0 {
                        cell.set_symbol("░");
                        cell.set_style(rug_style());
                    }
                }
            }
        }
    }
    let lines = supervisor_lines(state.supervisor, state.tick, layout.tier.sprite_set());
    let phase_label = match state.supervisor {
        SupervisorPhase::Idle => "idle",
        SupervisorPhase::Working => "typing…",
        SupervisorPhase::Reviewing => "reviewing",
        SupervisorPhase::Waiting => "watching",
    };
    blit_lines_centered(buf, area, &lines);
    if area.height > lines.len() as u16 {
        let y = area.y.saturating_add(lines.len() as u16);
        put_line_centered(
            buf,
            area.x,
            y.min(area.y.saturating_add(area.height.saturating_sub(1))),
            area.width,
            &format!("Supervisor ({phase_label})"),
            Style::default()
                .fg(Color::Rgb(255, 220, 120))
                .add_modifier(Modifier::BOLD),
        );
    }
}

fn paint_handoff_zone(buf: &mut Buffer, area: Rect, state: &GameModeState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let any_handoff = state.desks.iter().any(|d| {
        matches!(
            d.phase,
            ActorPhase::WalkToBoss | ActorPhase::Handoff | ActorPhase::Celebrate
        )
    });
    let style = if any_handoff {
        Style::default()
            .fg(Color::Rgb(255, 200, 80))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Rgb(70, 100, 110))
    };
    let msg = if any_handoff {
        "── handoff zone ⬆ supervisor ──"
    } else {
        "········ aisle ········"
    };
    put_line_centered(buf, area.x, area.y, area.width, msg, style);

    // Walking actors in handoff
    for desk in &state.desks {
        if matches!(desk.phase, ActorPhase::WalkToBoss | ActorPhase::Handoff) {
            let lines = developer_lines(desk.phase, desk.skin, state.tick, SpriteSet::Small);
            let t = desk.anim_t.clamp(0.0, 1.0);
            let x_off = ((area.width.saturating_sub(4) as f32) * t) as u16;
            let sub = Rect {
                x: area.x.saturating_add(x_off),
                y: area.y.saturating_add(1).min(area.y.saturating_add(area.height.saturating_sub(1))),
                width: 6.min(area.width),
                height: area.height.saturating_sub(1).max(1),
            };
            blit_lines(buf, sub, &lines);
        }
    }
}

fn paint_desk(buf: &mut Buffer, area: Rect, desk: &DeskSlot, set: SpriteSet, tick: u64) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if desk.is_empty() {
        blit_lines_centered(buf, area, &empty_desk_lines(set));
        return;
    }

    // Walking phases draw at desk only for spawn; walk/handoff drawn elsewhere
    let show_at_desk = matches!(
        desk.phase,
        ActorPhase::AtDeskWorking
            | ActorPhase::AtDeskThinking
            | ActorPhase::SpawnWalk
            | ActorPhase::Celebrate
            | ActorPhase::FailBeat
            | ActorPhase::ExitDoor
    );

    if show_at_desk {
        let sprite = developer_lines(desk.phase, desk.skin, tick, set);
        let sprite_h = sprite.len() as u16;
        let mon_h = area.height.saturating_sub(sprite_h).max(2);
        let mon_rect = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: mon_h.min(area.height),
        };
        let mon = monitor_lines(desk, mon_rect.width.saturating_sub(1).max(6), tick);
        blit_lines(buf, mon_rect, &mon);
        let y_cursor = area.y.saturating_add(mon_rect.height.min(area.height));
        let spr_rect = Rect {
            x: area.x,
            y: y_cursor.min(area.y.saturating_add(area.height.saturating_sub(1))),
            width: area.width,
            height: area
                .height
                .saturating_sub(mon_rect.height)
                .max(1),
        };
        // Spawn: slide in from left
        if desk.phase == ActorPhase::SpawnWalk {
            let t = desk.anim_t.clamp(0.0, 1.0);
            let x_off = ((1.0 - t) * 6.0) as u16;
            let shifted = Rect {
                x: spr_rect.x.saturating_add(x_off),
                ..spr_rect
            };
            blit_lines(buf, shifted, &sprite);
        } else {
            blit_lines_centered(buf, spr_rect, &sprite);
        }
    } else {
        // Desk empty-ish while agent walks: show monitor only + empty chair
        let mon = monitor_lines(desk, area.width.saturating_sub(1).max(6), tick);
        blit_lines(buf, area, &mon);
    }

    // Label
    if area.height >= 2 {
        let label = if desk.label.is_empty() {
            desk.subagent_type.clone()
        } else {
            desk.label.clone()
        };
        put_line(
            buf,
            area.x,
            area.y.saturating_add(area.height.saturating_sub(1)),
            area.width,
            &label,
            Style::default().fg(Color::Rgb(180, 200, 210)),
        );
    }
}

fn paint_door(buf: &mut Buffer, area: Rect, overflow: usize) {
    let lines = door_lines(area.height);
    blit_lines(buf, area, &lines);
    if overflow > 0 {
        put_line(
            buf,
            area.x,
            area.y.saturating_add(area.height.saturating_sub(1)),
            area.width,
            &format!("+{overflow}"),
            Style::default()
                .fg(Color::Rgb(255, 180, 60))
                .add_modifier(Modifier::BOLD),
        );
    } else {
        let _ = plant_span(); // keep plant helper linked for future props
    }
}

fn paint_status_strip(buf: &mut Buffer, area: Rect, state: &GameModeState, tier: GameTier) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let bg = Style::default()
        .fg(Color::Rgb(200, 210, 220))
        .bg(Color::Rgb(25, 35, 45));
    for x in area.x..area.x.saturating_add(area.width) {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_symbol(" ");
            cell.set_style(bg);
        }
    }
    let mut dots = String::new();
    for d in &state.desks {
        dots.push(if d.is_occupied() { '●' } else { '○' });
    }
    let sup = match state.supervisor {
        SupervisorPhase::Idle => "Idle",
        SupervisorPhase::Working => "Typing",
        SupervisorPhase::Reviewing => "Review",
        SupervisorPhase::Waiting => "Wait",
    };
    let mode = if state.pixel_mode { "pixel" } else { "ascii" };
    let text = format!(
        " {dots}  Active {}/6  Sup:{sup}  {}  Ctrl+G Normal  [{:?}/{mode}]",
        state.active_desk_count(),
        state.wall.title(),
        tier
    );
    put_line(buf, area.x, area.y, area.width, &text, bg);
}

// ── buffer helpers ──────────────────────────────────────────────

fn put_line(buf: &mut Buffer, x: u16, y: u16, max_w: u16, text: &str, style: Style) {
    let mut col = x;
    let end = x.saturating_add(max_w);
    for ch in text.chars() {
        let w = UnicodeWidthStr::width(ch.to_string().as_str()).max(1) as u16;
        if col.saturating_add(w) > end {
            break;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_symbol(&ch.to_string());
            cell.set_style(style);
        }
        col = col.saturating_add(w);
    }
}

fn put_line_centered(buf: &mut Buffer, x: u16, y: u16, max_w: u16, text: &str, style: Style) {
    let tw = UnicodeWidthStr::width(text) as u16;
    let start = x.saturating_add(max_w.saturating_sub(tw) / 2);
    put_line(buf, start, y, max_w.saturating_sub(start.saturating_sub(x)), text, style);
}

fn put_line_right(buf: &mut Buffer, x: u16, y: u16, max_w: u16, text: &str, style: Style) {
    let tw = UnicodeWidthStr::width(text) as u16;
    let start = x.saturating_add(max_w.saturating_sub(tw));
    put_line(buf, start, y, tw.min(max_w), text, style);
}

fn blit_lines(buf: &mut Buffer, area: Rect, lines: &[Line<'static>]) {
    for (i, line) in lines.iter().enumerate() {
        let y = area.y.saturating_add(i as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        let mut x = area.x;
        let end = area.x.saturating_add(area.width);
        for span in &line.spans {
            for ch in span.content.chars() {
                let w = UnicodeWidthStr::width(ch.to_string().as_str()).max(1) as u16;
                if x.saturating_add(w) > end {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(span.style);
                }
                x = x.saturating_add(w);
            }
        }
    }
}

fn blit_lines_centered(buf: &mut Buffer, area: Rect, lines: &[Line<'static>]) {
    let max_w = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0) as u16;
    let x_off = area.width.saturating_sub(max_w) / 2;
    let y_off = area.height.saturating_sub(lines.len() as u16) / 2;
    let sub = Rect {
        x: area.x.saturating_add(x_off),
        y: area.y.saturating_add(y_off),
        width: area.width.saturating_sub(x_off),
        height: area.height.saturating_sub(y_off),
    };
    blit_lines(buf, sub, lines);
}
