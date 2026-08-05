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
///
/// PERF INVARIANTS (RC13):
/// - Pixel recompose is gated by `GameModeState::ensure_pixel_frame` fingerprint;
///   pure tick + hover must not rebuild the scaled BG or sprite composite unless
///   desk/supervisor visual inputs actually changed.
/// - Hover focus ring + popup + status strip are **buffer overlays** after the
///   halfblock blit so hover-only mouse moves stay cheap.
pub fn render_game_mode(buf: &mut Buffer, area: Rect, state: &mut GameModeState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let layout = compute(area);
    state.last_stage = Some(layout.stage);
    state.last_desks = layout.desks;

    // Pixel path: high-res compose + halfblock paint (downsamples for sharp SNES look).
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
        // Prefer precomputed cell cache (HIT skips image sampling). Fallback to
        // terminal-res RGBA paint buffer, then unicode office.
        let painted = if let Some(cache) = state.pixel_halfblock.as_ref() {
            crate::render::image_overlay::paint_halfblock_cells(buf, pixel_area, cache)
        } else if let Some(frame) = state.pixel_paint_frame() {
            crate::render::image_overlay::paint_halfblock_rgba(buf, pixel_area, frame)
        } else {
            false
        };
        if !painted {
            render_unicode_office(buf, &layout, state);
        }
    } else {
        render_unicode_office(buf, &layout, state);
    }

    // Overlay-only chrome (never invalidates pixel_frame / scaled BG).
    paint_focus_ring_overlay(buf, &layout, state);
    paint_status_strip(buf, layout.status_strip, state, layout.tier);
    paint_hover_popup(buf, area, &layout, state);
}

/// Gold corner brackets on the focused desk — cell overlay so hover does not
/// recompose the high-res office frame.
fn paint_focus_ring_overlay(buf: &mut Buffer, layout: &GameLayout, state: &GameModeState) {
    let Some(idx) = state.focus_desk() else {
        return;
    };
    if idx >= state.desks.len() || state.desks[idx].is_empty() {
        return;
    }
    let r = layout.desks[idx];
    if r.width == 0 || r.height == 0 {
        return;
    }
    // Pulse via tick without touching the pixel fingerprint.
    let pulse = (state.tick / 4) % 2 == 0;
    let style = Style::default().fg(if pulse {
        Color::Rgb(255, 220, 96)
    } else {
        Color::Rgb(255, 200, 48)
    });
    let x0 = r.x;
    let y0 = r.y;
    let x1 = r.x.saturating_add(r.width.saturating_sub(1));
    let y1 = r.y.saturating_add(r.height.saturating_sub(1));
    // Corner brackets only.
    for (x, y, sym) in [
        (x0, y0, "┌"),
        (x0.saturating_add(1), y0, "─"),
        (x0, y0.saturating_add(1), "│"),
        (x1, y0, "┐"),
        (x1.saturating_sub(1), y0, "─"),
        (x1, y0.saturating_add(1), "│"),
        (x0, y1, "└"),
        (x0.saturating_add(1), y1, "─"),
        (x0, y1.saturating_sub(1), "│"),
        (x1, y1, "┘"),
        (x1.saturating_sub(1), y1, "─"),
        (x1, y1.saturating_sub(1), "│"),
    ] {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(sym);
            cell.set_style(style);
        }
    }
}

/// Floating SNES-style info card when the mouse is over a seated subagent.
///
/// Buffer-only: never calls `ensure_pixel_frame` / compose. Live labels, tokens,
/// and elapsed update here without invalidating the scaled BG or sprite cache.
fn paint_hover_popup(buf: &mut Buffer, area: Rect, layout: &GameLayout, state: &GameModeState) {
    let Some(idx) = state.focus_desk() else {
        return;
    };
    if idx >= state.desks.len() || state.desks[idx].is_empty() {
        return;
    }
    let desk = &state.desks[idx];
    let phase = match desk.phase {
        ActorPhase::AtDeskWorking => "working",
        ActorPhase::AtDeskThinking => "thinking",
        ActorPhase::SpawnWalk => "arriving",
        ActorPhase::Celebrate => "done!",
        ActorPhase::WalkToBoss | ActorPhase::Handoff => "delivering",
        ActorPhase::ExitDoor => "leaving",
        ActorPhase::FailBeat => "failed",
    };
    let secs = desk.elapsed.as_secs();
    let lines = [
        format!(" {}", desk.label),
        format!(" type  {}", desk.subagent_type),
        format!(
            " id    {}",
            truncate_mid(desk.child_session_id.as_deref().unwrap_or("—"), 22)
        ),
        format!(" state {phase}"),
        format!(
            " time  {}m{:02}s  tok {}  tools {}",
            secs / 60,
            secs % 60,
            desk.tokens,
            desk.tool_calls
        ),
        format!(" act   {}", truncate_mid(&desk.activity, 28)),
    ];
    let width = lines
        .iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(20)
        .clamp(24, 48) as u16
        + 2;
    let height = lines.len() as u16 + 2;

    // The card is placed with room for its 1-cell SE drop shadow, so the
    // footprint to keep inside `area` is one row and one column larger than the
    // card itself. Clamping the card alone parked it flush against the right /
    // bottom edge and the shadow then bled into the neighbouring UI rows
    // (RC16 B5).
    let right = area.x.saturating_add(area.width);
    let bottom = area.y.saturating_add(area.height);
    let max_x = right.saturating_sub(width.saturating_add(1)).max(area.x);
    let max_y = bottom.saturating_sub(height.saturating_add(1)).max(area.y);

    // Prefer near cursor; fall back to above desk.
    let (mut x, mut y) = state.hover_screen.unwrap_or((
        layout.desks[idx].x,
        layout.desks[idx].y.saturating_sub(height),
    ));
    y = y.saturating_sub(height.saturating_add(1));
    x = x.clamp(area.x, max_x);
    if y < area.y {
        // Prefer below desk if no room above.
        y = layout.desks[idx].y.saturating_add(layout.desks[idx].height);
    }
    y = y.clamp(area.y, max_y);

    let popup = Rect {
        x,
        y,
        width,
        height,
    };
    // An area too small to hold card + shadow still clips per cell below.
    let in_area = |x: u16, y: u16| rect_contains(area, x, y);
    let border = Style::default()
        .fg(Color::Rgb(255, 220, 96))
        .bg(Color::Rgb(22, 28, 42));
    let body = Style::default()
        .fg(Color::Rgb(220, 236, 255))
        .bg(Color::Rgb(22, 28, 42));
    let title = Style::default()
        .fg(Color::Rgb(120, 255, 180))
        .bg(Color::Rgb(22, 28, 42))
        .add_modifier(Modifier::BOLD);
    // Soft drop shadow (1 cell SE) for popup depth.
    let shadow = Style::default()
        .fg(Color::Rgb(8, 10, 16))
        .bg(Color::Rgb(8, 10, 16));
    for row in 0..height {
        for col in 0..width {
            let cx = popup.x.saturating_add(col).saturating_add(1);
            let cy = popup.y.saturating_add(row).saturating_add(1);
            if !in_area(cx, cy) {
                continue;
            }
            if let Some(cell) = buf.cell_mut((cx, cy)) {
                if row + 1 == height || col + 1 == width {
                    cell.set_symbol(" ");
                    cell.set_style(shadow);
                }
            }
        }
    }

    // Fill + border
    for row in 0..height {
        for col in 0..width {
            let cx = popup.x.saturating_add(col);
            let cy = popup.y.saturating_add(row);
            if !in_area(cx, cy) {
                continue;
            }
            if let Some(cell) = buf.cell_mut((cx, cy)) {
                let edge = row == 0 || row + 1 == height || col == 0 || col + 1 == width;
                cell.set_symbol(if edge {
                    if row == 0 && col == 0 {
                        "┌"
                    } else if row == 0 && col + 1 == width {
                        "┐"
                    } else if row + 1 == height && col == 0 {
                        "└"
                    } else if row + 1 == height && col + 1 == width {
                        "┘"
                    } else if row == 0 || row + 1 == height {
                        "─"
                    } else {
                        "│"
                    }
                } else {
                    " "
                });
                cell.set_style(if edge { border } else { body });
            }
        }
    }
    for (i, line) in lines.iter().enumerate() {
        let style = if i == 0 { title } else { body };
        let tx = popup.x.saturating_add(1);
        let ty = popup.y.saturating_add(1 + i as u16);
        if ty >= bottom {
            break;
        }
        put_line(
            buf,
            tx,
            ty,
            width.saturating_sub(2).min(right.saturating_sub(tx)),
            line,
            style,
        );
    }
}

fn truncate_mid(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let take = max.saturating_sub(1);
    format!("{}…", t.chars().take(take).collect::<String>())
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
        // Phase-aware desk dots (working / walking / fail / empty).
        let ch = if d.is_empty() {
            '○'
        } else if d.failed || matches!(d.phase, ActorPhase::FailBeat) {
            '✕'
        } else if matches!(
            d.phase,
            ActorPhase::WalkToBoss | ActorPhase::Handoff | ActorPhase::ExitDoor
        ) {
            '◎'
        } else if matches!(d.phase, ActorPhase::Celebrate) {
            '★'
        } else {
            '●'
        };
        dots.push(ch);
        dots.push(' ');
    }
    let sup = match state.supervisor {
        SupervisorPhase::Idle => "Idle",
        SupervisorPhase::Working => "Typing",
        SupervisorPhase::Reviewing => "Review",
        SupervisorPhase::Waiting => "Wait",
    };
    let mode = if state.pixel_mode { "pixel" } else { "ascii" };
    let overflow = if state.overflow_count > 0 {
        format!(" +{}", state.overflow_count)
    } else {
        String::new()
    };
    let focus = state
        .focus_desk()
        .map(|i| format!(" focus:{}", i + 1))
        .unwrap_or_default();
    let text = format!(
        " {dots} {n}/6{overflow}  Sup:{sup}  {wall}{focus}  Tab focus · Ctrl+G exit  [{tier:?}/{mode}]",
        n = state.active_desk_count(),
        wall = state.wall.title(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Paint the office into `game`, inside a buffer with 4 spare cells of
    /// surrounding UI on every side, hovering desk 0 at `hover`.
    fn render_hovering(game: Rect, hover: (u16, u16)) -> Buffer {
        let full = Rect::new(0, 0, game.x + game.width + 4, game.y + game.height + 4);
        let mut buf = Buffer::empty(full);
        let mut state = GameModeState::new();
        // Unicode path: the popup + shadow are the same buffer overlay either
        // way, and this keeps the test off the PNG decode.
        state.pixel_mode = false;
        state.open = true;
        state.desks[0].child_session_id = Some("child-1".to_string());
        state.desks[0].label = "worker".to_string();
        state.desks[0].subagent_type = "general-purpose".to_string();
        state.desks[0].activity = "Running: cargo build".to_string();
        state.desks[0].phase = ActorPhase::AtDeskWorking;
        state.hover_desk = Some(0);
        state.hover_screen = Some(hover);
        render_game_mode(&mut buf, game, &mut state);
        buf
    }

    /// B5: the hover card is placed with a 1-cell SE drop shadow, so clamping
    /// the card alone to the game area let the shadow paint one row/column
    /// into the neighbouring UI. Nothing outside `game` may be touched — from
    /// any hover position, including the far corners.
    #[test]
    fn hover_popup_and_shadow_stay_inside_the_game_area() {
        let game = Rect::new(2, 3, 100, 24);
        let blank = Buffer::empty(Rect::new(
            0,
            0,
            game.x + game.width + 4,
            game.y + game.height + 4,
        ));
        let right = game.x + game.width - 1;
        let bottom = game.y + game.height - 1;
        for hover in [
            (game.x, game.y),
            (right, game.y),
            (game.x, bottom),
            (right, bottom),
            (right, game.y + game.height / 2),
        ] {
            let buf = render_hovering(game, hover);
            for y in blank.area.y..blank.area.y + blank.area.height {
                for x in blank.area.x..blank.area.x + blank.area.width {
                    if rect_contains(game, x, y) {
                        continue;
                    }
                    assert_eq!(
                        buf.cell((x, y)),
                        blank.cell((x, y)),
                        "hover {hover:?} painted outside the game area at ({x},{y})"
                    );
                }
            }
        }
    }

    /// ...and a game area far too small for the card still paints only inside
    /// it (the per-cell clip, not the placement clamp, carries this one).
    #[test]
    fn hover_popup_clips_when_the_area_is_smaller_than_the_card() {
        let game = Rect::new(1, 1, 12, 6);
        let blank = Buffer::empty(Rect::new(
            0,
            0,
            game.x + game.width + 4,
            game.y + game.height + 4,
        ));
        let buf = render_hovering(game, (game.x + 1, game.y + 1));
        for y in blank.area.y..blank.area.y + blank.area.height {
            for x in blank.area.x..blank.area.x + blank.area.width {
                if rect_contains(game, x, y) {
                    continue;
                }
                assert_eq!(
                    buf.cell((x, y)),
                    blank.cell((x, y)),
                    "popup escaped a {}x{} game area at ({x},{y})",
                    game.width,
                    game.height
                );
            }
        }
    }
}
