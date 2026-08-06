//! Paint Game Mode into a ratatui Buffer.
//!
//! Primary path: mockup PNG + procedural sprites → halfblock raster.
//! Fallback: Unicode office (when pixel load/compose fails or Compact text-only).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::layout::{GameLayout, GameTier, SpriteSet, compute};
use super::monitor::monitor_lines;
use super::sprites::{
    developer_lines, door_lines, empty_desk_lines, floor_style, plant_span, rug_style,
    supervisor_lines, wall_bg,
};
use super::state::{
    ActorPhase, DeskSlot, GameModeState, HoverTarget, SupervisorPhase, local_clock_bucket,
};
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

    let mut pixel_painted = false;
    if use_pixel {
        // Prefer precomputed cell cache (HIT skips image sampling). Fallback to
        // terminal-res RGBA paint buffer, then unicode office.
        pixel_painted = if let Some(cache) = state.pixel_halfblock.as_ref() {
            crate::render::image_overlay::paint_halfblock_cells(buf, pixel_area, cache)
        } else if let Some(frame) = state.pixel_paint_frame() {
            crate::render::image_overlay::paint_halfblock_rgba(buf, pixel_area, frame)
        } else {
            false
        };
        if !pixel_painted {
            render_unicode_office(buf, &layout, state);
        }
    } else {
        render_unicode_office(buf, &layout, state);
    }

    // Supervisor hover hit box — must follow whichever office actually painted:
    // the pixel path places the boss from `compose`'s fractional anchors, the
    // unicode path fills `layout.supervisor` with the rug and centres the art.
    state.last_supervisor = if pixel_painted {
        super::compose::supervisor_hit_rect(pixel_area)
    } else {
        layout.supervisor
    };
    // The MCP rack is pixel-office-only art: the Unicode fallback composes no
    // rack and Compact has no office at all, so a zero-size rect (which never
    // hit-tests) is how "no rack to hover here" is expressed.
    state.last_mcp_rack = if pixel_painted {
        super::compose::rack_hit_rect(pixel_area)
    } else {
        Rect::default()
    };
    // Same signal, for the ambient wake rather than for hit-testing: the coffee
    // sip, the Supervisor's steam and the wall-clock hands are pixel-office art,
    // so only a pixel paint earns the ~0.33 Hz tick that animates them (see
    // `GameModeState::needs_ambient_tick`).
    state.last_pixel_painted = pixel_painted;

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

/// Floating SNES-style info card for whatever the pointer / Tab focus is on.
///
/// Buffer-only: never calls `ensure_pixel_frame` / compose. Live labels, tokens,
/// elapsed and the Supervisor's model/turn/context update here without
/// invalidating the scaled BG or sprite cache.
fn paint_hover_popup(buf: &mut Buffer, area: Rect, layout: &GameLayout, state: &GameModeState) {
    let Some(target) = state.focus_target() else {
        return;
    };
    let (lines, subject) = match target {
        HoverTarget::Desk(idx) => {
            if idx >= state.desks.len() || state.desks[idx].is_empty() {
                return;
            }
            (desk_popup_lines(&state.desks[idx]), layout.desks[idx])
        }
        HoverTarget::Supervisor => (supervisor_popup_lines(state), state.last_supervisor),
        HoverTarget::McpRack => (mcp_rack_popup_lines(state), state.last_mcp_rack),
    };
    paint_popup(
        buf,
        area,
        PopupAnchor {
            cursor: state.hover_screen,
            subject,
        },
        &lines,
    );
}

/// Card body for a seated subagent.
fn desk_popup_lines(desk: &DeskSlot) -> Vec<(String, Style)> {
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
    let body = popup_body_style();
    vec![
        (format!(" {}", desk.label), popup_title_style()),
        (format!(" type  {}", desk.subagent_type), body),
        (
            format!(
                " id    {}",
                truncate_mid(desk.child_session_id.as_deref().unwrap_or("—"), 22)
            ),
            body,
        ),
        (format!(" state {phase}"), body),
        (
            format!(
                " time  {}m{:02}s  tok {}  tools {}",
                secs / 60,
                secs % 60,
                desk.tokens,
                desk.tool_calls
            ),
            body,
        ),
        (format!(" act   {}", truncate_mid(&desk.activity, 28)), body),
    ]
}

/// Card body for the Supervisor (the main agent driving the room).
///
/// Reads the overlay-only [`super::state::SupervisorSnapshot`] plus room state
/// the painter already owns (seats, overflow, wall). Deliberately tight — this
/// is a tooltip, not the status bar: what is on the model, how long this turn
/// has run, how full the context is, and what the room is doing.
fn supervisor_popup_lines(state: &GameModeState) -> Vec<(String, Style)> {
    let info = &state.supervisor_info;
    let phase = match state.supervisor {
        SupervisorPhase::Idle => "idle",
        SupervisorPhase::Working => "working",
        SupervisorPhase::Reviewing => "reviewing",
        SupervisorPhase::Waiting => "watching",
    };
    let body = popup_body_style();
    let mut lines = vec![
        (" Supervisor".to_string(), popup_title_style()),
        (
            format!(
                " model {}",
                truncate_mid(info.model.as_deref().unwrap_or("—"), 28)
            ),
            body,
        ),
        (
            format!(
                " turn  {}  ({phase})",
                match info.turn_elapsed {
                    Some(d) => {
                        let secs = d.as_secs();
                        format!("{}m{:02}s", secs / 60, secs % 60)
                    }
                    None => "—".to_string(),
                }
            ),
            body,
        ),
    ];
    if info.context_total > 0 {
        lines.push((
            format!(
                " ctx   {}/{}  {}%",
                super::monitor::fmt_tokens(info.context_used),
                super::monitor::fmt_tokens(info.context_total),
                info.context_pct
            ),
            body,
        ));
    }
    let overflow = if state.overflow_count > 0 {
        format!(" +{}", state.overflow_count)
    } else {
        String::new()
    };
    lines.push((
        format!(
            " desks {}/{}{overflow}  {}",
            state.active_desk_count(),
            super::state::DESK_COUNT,
            state.wall.title()
        ),
        body,
    ));
    if let Some(branch) = info.branch.as_deref() {
        lines.push((format!(" branch {}", truncate_mid(branch, 26)), body));
    }
    if info.waiting_on_user {
        lines.push((" ▲ waiting on you".to_string(), popup_title_style()));
    }
    lines
}

/// Body lines the rack card may print before it collapses the rest into
/// `+N more`.
///
/// A budget on **emitted lines**, not on servers, and that distinction is the
/// whole point: a non-Ready server prints a second, indented detail line, so a
/// per-server cap of six let six unavailable servers emit twelve rows plus the
/// tail. [`paint_popup`] clips per cell, so nothing painted out of bounds, but
/// on a short stage the card covered the office and lost its bottom border.
///
/// Counting detail lines and the `+N more` tail against the same budget makes
/// the bound real: the card is at most `1 title + MCP_POPUP_MAX_ROWS body + 2
/// border rows` = 9 cells tall, whatever the fleet looks like — which is what
/// keeps it from out-growing the desk card it shares [`paint_popup`] with.
const MCP_POPUP_MAX_ROWS: usize = 6;

/// Status glyph for one MCP server row.
///
/// Drawn instead of a colour badge because [`paint_popup`] paints one `Style`
/// per line and the office cards use fixed SNES-ish colours rather than the
/// theme — see [`popup_body_style`]. `label()` on the right carries the same
/// information in words, so the glyph is decoration, not the only signal.
fn mcp_status_glyph(status: crate::views::mcps_modal::McpServerDisplayStatus) -> char {
    use crate::views::mcps_modal::McpServerDisplayStatus as S;
    match status {
        S::Ready => '●',
        S::Initializing => '◐',
        S::NeedsAuth | S::SetupRequired => '▲',
        S::Unavailable => '✕',
    }
}

/// Card body for the MCP server rack.
///
/// Reads the overlay-only [`super::state::McpRackSnapshot`]. Falls back to the
/// `x.ai/mcp/init_progress` counts while the per-server list has not landed
/// yet, which is the whole window between session start and the first
/// `mcp/list` response — the rack is on screen for all of it.
fn mcp_rack_popup_lines(state: &GameModeState) -> Vec<(String, Style)> {
    let info = &state.mcp_info;
    let body = popup_body_style();
    let mut lines = vec![(" MCP servers".to_string(), popup_title_style())];

    if info.servers.is_empty() {
        // No list yet. Say which of the two reasons it is, rather than
        // implying the agent has no servers.
        lines.push((
            if info.init_active || info.init_total > 0 {
                format!(
                    " connecting {}/{}",
                    info.init_connected, info.init_total
                )
            } else {
                " no servers reported".to_string()
            },
            body,
        ));
        return lines;
    }

    // Budget is on emitted body lines, not on servers: a non-Ready row costs
    // two (see [`MCP_POPUP_MAX_ROWS`]). One line is held back for the `+N more`
    // tail whenever servers would be left over, so the tail always fits.
    let total = info.servers.len();
    let mut emitted = 0usize;
    let mut shown = 0usize;
    for (i, row) in info.servers.iter().enumerate() {
        let ready = matches!(
            row.status,
            crate::views::mcps_modal::McpServerDisplayStatus::Ready
        );
        let detail = if ready {
            None
        } else {
            row.status_detail.as_deref()
        };
        let cost = 1 + usize::from(detail.is_some());
        let reserve = usize::from(i + 1 < total);
        if emitted + cost + reserve > MCP_POPUP_MAX_ROWS {
            break;
        }
        lines.push((
            format!(
                " {} {}  {}  {} tools",
                mcp_status_glyph(row.status),
                truncate_mid(row.label(), 18),
                row.status.label(),
                row.tool_count
            ),
            if ready { body } else { popup_title_style() },
        ));
        // Why it is not ready, when the shell told us.
        if let Some(detail) = detail {
            lines.push((format!("   {}", truncate_mid(detail, 32)), body));
        }
        emitted += cost;
        shown += 1;
    }
    if shown < total {
        lines.push((format!(" +{} more", total - shown), body));
    }
    lines
}

/// Placement inputs for [`paint_popup`].
struct PopupAnchor {
    /// Cursor cell — the card sits just above it when there is room.
    cursor: Option<(u16, u16)>,
    /// Rect the card describes: the fallback anchor when there is no cursor,
    /// and the "drop below" position when the card does not fit above.
    subject: Rect,
}

fn popup_body_style() -> Style {
    Style::default()
        .fg(Color::Rgb(220, 236, 255))
        .bg(Color::Rgb(22, 28, 42))
}

fn popup_title_style() -> Style {
    Style::default()
        .fg(Color::Rgb(120, 255, 180))
        .bg(Color::Rgb(22, 28, 42))
        .add_modifier(Modifier::BOLD)
}

/// Paint one bordered SNES-style card with a drop shadow, clipped to `area`.
///
/// The box, the 1-cell SE shadow, the cursor-relative placement and the edge
/// clamping live here so every Game Mode tooltip (desk, Supervisor, and the MCP
/// rack to come) is placed and framed identically; callers only supply text.
fn paint_popup(buf: &mut Buffer, area: Rect, anchor: PopupAnchor, lines: &[(String, Style)]) {
    if lines.is_empty() {
        return;
    }
    let width = lines
        .iter()
        .map(|(s, _)| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(20)
        .clamp(24, 48) as u16
        + 2;
    let height = lines.len() as u16 + 2;

    // The card is placed with room for its 1-cell SE drop shadow, so the
    // footprint to keep inside `area` is one row and one column larger than the
    // card itself. Clamping the card alone parked it flush against the right /
    // bottom edge and the shadow then bled into the neighbouring UI rows
    // (RC2 B5).
    let right = area.x.saturating_add(area.width);
    let bottom = area.y.saturating_add(area.height);
    let max_x = right.saturating_sub(width.saturating_add(1)).max(area.x);
    let max_y = bottom.saturating_sub(height.saturating_add(1)).max(area.y);

    // Prefer near cursor; fall back to above the subject.
    let (mut x, mut y) = anchor.cursor.unwrap_or((
        anchor.subject.x,
        anchor.subject.y.saturating_sub(height),
    ));
    y = y.saturating_sub(height.saturating_add(1));
    x = x.clamp(area.x, max_x);
    if y < area.y {
        // Prefer below the subject if no room above.
        y = anchor.subject.y.saturating_add(anchor.subject.height);
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
    let body = popup_body_style();
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
    for (i, (line, style)) in lines.iter().enumerate() {
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
            *style,
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

/// Paint the wall strip: mode title, wall clock, and the shelf trinkets.
///
/// The clock is read **live** here rather than from
/// [`GameModeState::clock_hm`], and that is load-bearing. `clock_hm` is only
/// refreshed by `tick_anim`'s ambient step, and the ambient wake
/// (`needs_ambient_tick`) is gated on the last paint having drawn the pixel
/// office — which is false in exactly the two tiers this function serves
/// (Compact and the Unicode fallback). An idle office in either tier parks at
/// `TickDemand::None`, so a `clock_hm` read would freeze at whatever time the
/// room last animated and quietly show the wrong hour.
///
/// A live read is free here and cannot be free in the pixel office: this text
/// is a ratatui buffer overlay painted after the halfblock blit, so it never
/// reaches `visual_fingerprint` and costs zero extra wakeups. `clock_hm` stays
/// the fingerprint input for the *pixel* office's hands and hour tint, where
/// the value must be hashable.
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
    let clock = format_clock(local_clock_bucket());
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

/// Wall-strip clock for the Unicode office — real local time (RC2 §4 #12).
///
/// It used to be a decorative `tick / 12` session timer, which RC2 BUG-2 ran at
/// half speed and RC2 PERF-1 then froze outright whenever the room parked. Now
/// it formats the same `(hour, ten-minute)` bucket the pixel office draws hands
/// from, so the two offices agree and neither depends on the tick rate. Its
/// caller ([`paint_wall_display`]) samples that bucket live at paint time, so
/// the text is correct on the very first repaint of a room that has never
/// ticked.
fn format_clock(clock_hm: (u8, u8)) -> String {
    let (h, tenmin) = clock_hm;
    format!("{:02}:{:02}", h % 24, (tenmin.min(5)) * 10)
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

/// Cells consumed by one painted char — never zero, so a combining mark still
/// advances (matches the pre-RC2 behaviour of `width(ch.to_string()).max(1)`).
///
/// PERF (RC2 P9): the old form heap-allocated a `String` just to measure, and
/// `set_symbol(&ch.to_string())` allocated a second one to paint. A full stage
/// is thousands of characters per frame; `UnicodeWidthChar` + `set_char` are
/// the pattern used by the rest of this pager's buffer painters.
fn char_cells(ch: char) -> u16 {
    UnicodeWidthChar::width(ch).unwrap_or(0).max(1) as u16
}

fn put_line(buf: &mut Buffer, x: u16, y: u16, max_w: u16, text: &str, style: Style) {
    let mut col = x;
    let end = x.saturating_add(max_w);
    for ch in text.chars() {
        let w = char_cells(ch);
        if col.saturating_add(w) > end {
            break;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch);
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
                let w = char_cells(ch);
                if x.saturating_add(w) > end {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch);
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
        state.hover = Some(HoverTarget::Desk(0));
        state.hover_screen = Some(hover);
        render_game_mode(&mut buf, game, &mut state);
        buf
    }

    /// Rows of the popup card as painted, located by its top-left corner.
    ///
    /// Returns the card's interior text lines (trailing blanks trimmed) so a
    /// tooltip's rendered content can be pinned without pinning its placement.
    fn popup_text_rows(buf: &Buffer, area: Rect) -> Vec<String> {
        let (mut ox, mut oy) = (0u16, 0u16);
        let mut found = false;
        // Match on the card's gold border colour as well as the corner glyph:
        // the office art draws `┌` corners of its own (empty desks, compact
        // cards), and a short card can land below one of them.
        let gold = Color::Rgb(255, 220, 96);
        'scan: for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if buf
                    .cell((x, y))
                    .is_some_and(|c| c.symbol() == "┌" && c.style().fg == Some(gold))
                {
                    (ox, oy) = (x, y);
                    found = true;
                    break 'scan;
                }
            }
        }
        assert!(found, "no popup card painted");
        // Card width: run of "─" after the corner, plus both corners.
        let mut w = 1u16;
        while buf.cell((ox + w, oy)).map(|c| c.symbol()) == Some("─") {
            w += 1;
        }
        w += 1;
        let mut rows = Vec::new();
        let mut y = oy + 1;
        while buf.cell((ox, y)).map(|c| c.symbol()) == Some("│") {
            let mut row = String::new();
            for x in ox + 1..ox + w - 1 {
                row.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            rows.push(row.trim_end().to_string());
            y += 1;
        }
        rows
    }

    /// The desk card was reimplemented on top of the shared [`paint_popup`]
    /// core (so the Supervisor and the MCP rack can reuse the box); its
    /// rendered content must be exactly what it was before.
    #[test]
    fn desk_tooltip_lines_are_unchanged_by_the_popup_refactor() {
        let game = Rect::new(2, 3, 100, 24);
        let buf = render_hovering(game, (game.x + 20, game.y + 18));
        assert_eq!(
            popup_text_rows(&buf, game),
            vec![
                " worker",
                " type  general-purpose",
                " id    child-1",
                " state working",
                " time  0m00s  tok 0  tools 0",
                " act   Running: cargo build",
            ]
        );
    }

    /// Hovering the boss paints the Supervisor card from the overlay snapshot
    /// (never from the composed frame).
    #[test]
    fn supervisor_tooltip_renders_the_snapshot() {
        let game = Rect::new(2, 3, 100, 24);
        let mut buf = Buffer::empty(Rect::new(0, 0, game.x + game.width + 4, game.y + game.height + 4));
        let mut state = GameModeState::new();
        state.pixel_mode = false;
        state.open = true;
        state.desks[0].child_session_id = Some("child-1".to_string());
        state.overflow_count = 2;
        state.supervisor = SupervisorPhase::Working;
        state.wall = WallMode::Working;
        state.supervisor_info = super::super::state::SupervisorSnapshot {
            model: Some("Grok 4.5".to_string()),
            turn_elapsed: Some(std::time::Duration::from_secs(93)),
            context_used: 42_000,
            context_total: 256_000,
            context_pct: 16,
            waiting_on_user: true,
            branch: Some("rc2-game-mode".to_string()),
        };
        state.hover = Some(HoverTarget::Supervisor);
        state.hover_screen = Some((game.x + 20, game.y + 18));
        render_game_mode(&mut buf, game, &mut state);

        assert_eq!(
            popup_text_rows(&buf, game),
            vec![
                " Supervisor",
                " model Grok 4.5",
                " turn  1m33s  (working)",
                " ctx   42.0k/256.0k  16%",
                " desks 1/6 +2  WORKING",
                " branch rc2-game-mode",
                " ▲ waiting on you",
            ]
        );
        assert_ne!(
            state.last_supervisor,
            Rect::default(),
            "the unicode path must publish a supervisor hit rect"
        );
    }

    /// Paint the office with the MCP rack hovered and `mcp_info` preloaded.
    ///
    /// Forces the target rather than hit-testing it: the rack is pixel-office
    /// art, so the unicode path this helper uses (to stay off the PNG decode)
    /// deliberately publishes a zero-size `last_mcp_rack`. The card body is the
    /// same either way — that is what `paint_popup` exists for.
    fn render_rack_card(info: super::super::state::McpRackSnapshot) -> (Buffer, Rect) {
        let game = Rect::new(2, 3, 100, 24);
        let mut buf = Buffer::empty(Rect::new(
            0,
            0,
            game.x + game.width + 4,
            game.y + game.height + 4,
        ));
        let mut state = GameModeState::new();
        state.pixel_mode = false;
        state.open = true;
        state.mcp_info = info;
        state.hover = Some(HoverTarget::McpRack);
        state.hover_screen = Some((game.x + 20, game.y + 18));
        render_game_mode(&mut buf, game, &mut state);
        assert_eq!(
            state.last_mcp_rack,
            Rect::default(),
            "the unicode office composes no rack, so none may be hoverable"
        );
        (buf, game)
    }

    /// Startup: the per-server list has not landed, so the card falls back to
    /// the `mcp/init_progress` counts rather than claiming there are no
    /// servers.
    #[test]
    fn mcp_rack_tooltip_falls_back_to_init_progress() {
        let (buf, game) = render_rack_card(super::super::state::McpRackSnapshot {
            servers: Vec::new(),
            init_connected: 2,
            init_total: 5,
            init_active: true,
            rows_gen: 0,
        });
        assert_eq!(
            popup_text_rows(&buf, game),
            vec![" MCP servers", " connecting 2/5"]
        );

        // ...and with no init progress either, it says so instead of lying
        // about a fleet it has never seen.
        let (buf, game) = render_rack_card(super::super::state::McpRackSnapshot::default());
        assert_eq!(
            popup_text_rows(&buf, game),
            vec![" MCP servers", " no servers reported"]
        );
    }

    /// Once the cache is populated the card is one row per server, with the
    /// truncated failure detail under the rows that are not Ready, and a
    /// `+N more` tail past [`MCP_POPUP_MAX_ROWS`].
    ///
    /// The height bound is the one claim the constant makes, so it is asserted
    /// for a *worst case* fleet — every server unavailable with a detail line,
    /// i.e. two emitted lines each. A per-server cap used to let that render a
    /// 16-row card.
    #[test]
    fn mcp_rack_tooltip_renders_one_row_per_server() {
        use crate::views::mcps_modal::{McpServerDisplayStatus, McpStatusRow};

        let row = |name: &str, status, tools| McpStatusRow {
            name: name.to_string(),
            display_name: None,
            status,
            tool_count: tools,
            status_detail: None,
        };
        let mut servers = vec![
            row("github", McpServerDisplayStatus::Ready, 12),
            McpStatusRow {
                status_detail: Some("EOF while reading handshake".into()),
                ..row("linear", McpServerDisplayStatus::Unavailable, 0)
            },
        ];
        let (buf, game) = render_rack_card(super::super::state::McpRackSnapshot {
            servers: servers.clone(),
            init_connected: 0,
            init_total: 0,
            init_active: false,
            rows_gen: 1,
        });
        assert_eq!(
            popup_text_rows(&buf, game),
            vec![
                " MCP servers",
                " ● github  ready  12 tools",
                " ✕ linear  unavailable  0 tools",
                "   EOF while reading handshake",
            ]
        );

        // Past the cap the tail collapses; the card must not grow without bound.
        // github(1) + linear(2) + extra-0(1) + extra-1(1) = 5 emitted lines,
        // and the sixth is held back so the tail fits inside the budget.
        for i in 0..6 {
            servers.push(row(&format!("extra-{i}"), McpServerDisplayStatus::Ready, 1));
        }
        let (buf, game) = render_rack_card(super::super::state::McpRackSnapshot {
            servers,
            init_connected: 0,
            init_total: 0,
            init_active: false,
            rows_gen: 2,
        });
        let rows = popup_text_rows(&buf, game);
        assert_eq!(rows.first().map(String::as_str), Some(" MCP servers"));
        assert_eq!(rows.last().map(String::as_str), Some(" +4 more"));
        assert_eq!(
            rows.len(),
            1 + MCP_POPUP_MAX_ROWS,
            "title + at most MCP_POPUP_MAX_ROWS body lines: {rows:?}"
        );

        // Worst case for the bound: every row is non-Ready *and* carries a
        // detail, so each server costs two emitted lines. The per-server cap
        // this replaced rendered 13 rows here.
        let unavailable: Vec<_> = (0..8)
            .map(|i| McpStatusRow {
                status_detail: Some(format!("handshake failed ({i})")),
                ..row(&format!("down-{i}"), McpServerDisplayStatus::Unavailable, 0)
            })
            .collect();
        let (buf, game) = render_rack_card(super::super::state::McpRackSnapshot {
            servers: unavailable,
            init_connected: 0,
            init_total: 0,
            init_active: false,
            rows_gen: 3,
        });
        let rows = popup_text_rows(&buf, game);
        assert!(
            rows.len() <= 1 + MCP_POPUP_MAX_ROWS,
            "all-unavailable fleet blew the height bound: {rows:?}"
        );
        assert_eq!(
            rows.last().map(String::as_str),
            Some(" +6 more"),
            "the collapsed tail must count the servers, not the lines: {rows:?}"
        );
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

    fn symbol_at(buf: &Buffer, x: u16, y: u16) -> &str {
        buf.cell((x, y)).expect("cell in bounds").symbol()
    }

    /// P9: `put_line` no longer allocates a `String` per painted character.
    /// The cursor advance and the right-edge clip must still be by display
    /// width — a wide glyph that would cross `max_w` is dropped whole, not
    /// half-painted, and a zero-width mark still consumes one cell.
    #[test]
    fn put_line_advances_and_clips_by_display_width() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        put_line(&mut buf, 0, 0, 8, "a日b", Style::default());
        assert_eq!(symbol_at(&buf, 0, 0), "a");
        assert_eq!(symbol_at(&buf, 1, 0), "日");
        assert_eq!(symbol_at(&buf, 2, 0), " ", "wide glyph's trailing half");
        assert_eq!(symbol_at(&buf, 3, 0), "b");

        // Two columns of room: the wide glyph does not fit after 'a'.
        let mut narrow = Buffer::empty(Rect::new(0, 0, 8, 1));
        put_line(&mut narrow, 0, 0, 2, "a日b", Style::default());
        assert_eq!(symbol_at(&narrow, 0, 0), "a");
        assert_eq!(symbol_at(&narrow, 1, 0), " ", "half a 日 must never paint");

        // `char_cells` floors at 1, as the old `width(..).max(1)` did.
        assert_eq!(char_cells('\u{0301}'), 1);
        assert_eq!(char_cells('日'), 2);
    }

    /// P9: `blit_lines` shares the same per-char advance, and a span continues
    /// where the previous one stopped.
    #[test]
    fn blit_lines_advances_across_spans_by_display_width() {
        use ratatui::text::Span;

        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 1));
        let line = Line::from(vec![Span::raw("日"), Span::raw("ab")]);
        blit_lines(&mut buf, Rect::new(0, 0, 4, 1), &[line]);
        assert_eq!(symbol_at(&buf, 0, 0), "日");
        assert_eq!(symbol_at(&buf, 2, 0), "a");
        assert_eq!(symbol_at(&buf, 3, 0), "b");
    }

    /// RC2 §4 #12: the Unicode office's wall clock now shows the same real
    /// local time bucket the pixel office draws hands from. It used to be a
    /// `tick / 12` session timer, which BUG-2 ran at half speed and PERF-1 then
    /// froze outright — so it must no longer depend on `tick` at all.
    #[test]
    fn unicode_wall_clock_shows_the_real_ten_minute_bucket() {
        assert_eq!(format_clock((0, 0)), "00:00");
        assert_eq!(format_clock((9, 3)), "09:30");
        assert_eq!(format_clock((23, 5)), "23:50");
        // Out-of-range input is clamped, never panics or renders ":60".
        assert_eq!(format_clock((24, 9)), "00:50");
    }

    /// The `hh:mm` the wall strip actually painted, read back off the buffer.
    ///
    /// Located by the 🕐 glyph, then filtered to the digits and colon that
    /// follow it — the clock is right-aligned at the end of the wall row, so
    /// nothing else on the row can contaminate the scan.
    fn painted_wall_clock(buf: &Buffer, area: Rect) -> String {
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if buf.cell((x, y)).map(|c| c.symbol()) != Some("🕐") {
                    continue;
                }
                return (x..area.x + area.width)
                    .filter_map(|cx| buf.cell((cx, y)).map(|c| c.symbol()))
                    .flat_map(|s| s.chars())
                    .filter(|c| c.is_ascii_digit() || *c == ':')
                    .collect();
            }
        }
        panic!("no wall clock painted");
    }

    /// RC2 §4 #12 regression: the wall clock must be sampled **at paint time**,
    /// not read from the tick-refreshed `clock_hm`.
    ///
    /// `clock_hm` is only refreshed by `tick_anim`'s ambient step, and the
    /// ambient wake is gated on `last_pixel_painted` — false in exactly the two
    /// tiers this office serves. So an idle Compact/Unicode room never ticks,
    /// and a `clock_hm` read would park the strip at whatever time the room
    /// last animated: a plausible-looking clock showing the wrong hour. The
    /// pure `format_clock` test above cannot see that; only the paint site can.
    #[test]
    fn wall_clock_is_live_at_paint_time_in_a_room_that_never_ticked() {
        use super::super::state::local_clock_bucket;

        let game = Rect::new(2, 3, 100, 24);
        let mut buf = Buffer::empty(Rect::new(
            0,
            0,
            game.x + game.width + 4,
            game.y + game.height + 4,
        ));
        let mut state = GameModeState::new();
        state.pixel_mode = false;
        state.open = true;
        // A room that has never animated: no ticks, and `clock_hm` left at a
        // stale hour that no amount of waiting will refresh in this tier.
        assert_eq!(state.tick, 0, "this office must not have ticked");
        let stale = ((local_clock_bucket().0 + 7) % 24, 0u8);
        state.clock_hm = stale;

        let before = local_clock_bucket();
        render_game_mode(&mut buf, game, &mut state);
        let after = local_clock_bucket();

        let painted = painted_wall_clock(&buf, game);
        assert!(
            painted == format_clock(before) || painted == format_clock(after),
            "wall clock painted {painted:?}, expected live local time \
             {:?} (the ten-minute bucket may have rolled mid-test)",
            format_clock(after)
        );
        assert_ne!(
            painted,
            format_clock(stale),
            "the wall clock froze at the stale `clock_hm` instead of reading live"
        );
        assert_eq!(
            state.clock_hm, stale,
            "painting must not refresh `clock_hm` — it is the pixel office's \
             fingerprint input and only `tick_anim` may move it"
        );
    }
}
