//! Composite office background + sprites at **high internal resolution**.
//!
//! - `PIXEL_SCALE` denser than terminal halfblock cells for sharper SNES detail
//! - Floor clears use SNES carpet tiles (no diagonal green mask)
//! - Smaller desk sprites relative to the room

use image::imageops::FilterType;
use image::RgbaImage;

use super::sprites_pixel::{
    DevPalette, blit, scale_nn, sprite_coffee, sprite_developer_at_desk, sprite_developer_walk,
    sprite_empty_desk, sprite_plant, sprite_supervisor, stamp_floor_patch_sampled,
};
use super::state::{ActorPhase, GameModeState, SupervisorPhase};

/// Embedded mockup.
pub const OFFICE_BG_PNG: &[u8] = include_bytes!("../../../assets/game_mode/office_bg.png");

const DESK_ANCHORS: [(f32, f32); 6] = [
    (0.22, 0.52),
    (0.50, 0.52),
    (0.78, 0.52),
    (0.22, 0.78),
    (0.50, 0.78),
    (0.78, 0.78),
];

const SUPERVISOR_ANCHOR: (f32, f32) = (0.50, 0.28);

/// Door position as a fraction of frame width — actors enter (SpawnWalk) and
/// leave (ExitDoor) through it.
const DOOR_X_FRAC: f32 = 0.06;

pub fn load_office_background() -> Result<RgbaImage, String> {
    image::load_from_memory(OFFICE_BG_PNG)
        .map(|i| i.to_rgba8())
        .map_err(|e| format!("decode office bg: {e}"))
}

/// Scale background to high internal resolution (PIXEL_SCALE × halfblock grid).
///
/// Terminal halfblock paint maps `cell_w × cell_h*2` → paint. We compose at
/// `cell_w*SCALE × cell_h*2*SCALE` so sprites keep crisp SNES detail, then
/// halfblock paints from a terminal-res downsample.
pub fn scale_bg_to_cells(full: &RgbaImage, cell_w: u16, cell_h: u16) -> RgbaImage {
    let scale = super::sprites_pixel::effective_pixel_scale(cell_w, cell_h).max(1);
    scale_bg_to_cells_with_scale(full, cell_w, cell_h, scale)
}

/// Same as [`scale_bg_to_cells`] with an explicit scale (must match
/// [`super::sprites_pixel::effective_pixel_scale`] used by the fingerprint).
pub fn scale_bg_to_cells_with_scale(
    full: &RgbaImage,
    cell_w: u16,
    cell_h: u16,
    scale: u32,
) -> RgbaImage {
    let scale = scale.max(1);
    let tw = u32::from(cell_w).saturating_mul(scale).max(1);
    let th = u32::from(cell_h)
        .saturating_mul(2)
        .saturating_mul(scale)
        .max(1);
    // CatmullRom keeps more mockup sharpness than Triangle at high scale.
    image::imageops::resize(full, tw, th, FilterType::CatmullRom)
}

pub fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut buf),
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("encode png: {e}"))?;
    Ok(buf)
}

/// Smaller sprites: ~7.5% of frame width vs old ~11%.
fn desk_scale(w: u32) -> u32 {
    let base = 28.0; // empty desk sprite width
    ((w as f32 * 0.075) / base).max(1.0).round().min(5.0) as u32
}

// Thread-local scaled sprite cache — Arc so blit does not clone every frame.
//
// PERF INVARIANT: compose_cell_frame must stay cheap on tick-only frames.
// Keys are stable per (kind, skin, frame, scale); plant/coffee are static.
use std::sync::Arc;
thread_local! {
    static SPRITE_CACHE: std::cell::RefCell<std::collections::HashMap<u64, Arc<RgbaImage>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn cache_get_or_insert(key: u64, build: impl FnOnce() -> RgbaImage) -> Arc<RgbaImage> {
    SPRITE_CACHE.with(|c| {
        let mut map = c.borrow_mut();
        // Cap cache so palette/scale churn cannot grow unbounded.
        if map.len() > 128 {
            map.clear();
        }
        Arc::clone(map.entry(key).or_insert_with(|| Arc::new(build())))
    })
}

fn cached_empty_desk(sc: u32) -> Arc<RgbaImage> {
    cache_get_or_insert(0xE0u64 << 56 | sc as u64, || {
        scale_nn(&sprite_empty_desk(), sc.max(1))
    })
}

fn cached_dev_at_desk(skin: u8, typing: bool, frame: u8, sc: u32) -> Arc<RgbaImage> {
    let key = (0xD1u64 << 56)
        | ((skin as u64) << 40)
        | ((typing as u64) << 32)
        | ((frame as u64) << 24)
        | sc as u64;
    cache_get_or_insert(key, || {
        let pal = DevPalette::by_index(skin);
        scale_nn(&sprite_developer_at_desk(pal, typing, frame), sc.max(1))
    })
}

fn cached_walk(skin: u8, frame: u8, with_packet: bool, sc: u32) -> Arc<RgbaImage> {
    let key = (0xD2u64 << 56)
        | ((skin as u64) << 40)
        | ((with_packet as u64) << 32)
        | ((frame as u64) << 24)
        | sc as u64;
    cache_get_or_insert(key, || {
        let pal = DevPalette::by_index(skin);
        scale_nn(
            &sprite_developer_walk(pal, frame, with_packet),
            sc.max(1),
        )
    })
}

fn cached_supervisor(phase: u8, frame: u8, sc: u32) -> Arc<RgbaImage> {
    let key = (0xA0u64 << 56) | ((phase as u64) << 32) | ((frame as u64) << 24) | sc as u64;
    cache_get_or_insert(key, || {
        scale_nn(&sprite_supervisor(phase, frame), sc.max(1))
    })
}

fn cached_plant(sc: u32) -> Arc<RgbaImage> {
    cache_get_or_insert(0xF1u64 << 56 | sc as u64, || {
        scale_nn(&sprite_plant(), sc.max(1))
    })
}

fn cached_coffee(sc: u32) -> Arc<RgbaImage> {
    cache_get_or_insert(0xC0u64 << 56 | sc as u64, || {
        scale_nn(&sprite_coffee(), sc.max(1))
    })
}

/// Clear baked mockup character + furniture in a desk region with SNES floor.
fn clear_desk_area(canvas: &mut RgbaImage, bg: &RgbaImage, cx: i32, cy: i32, w: u32, h: u32) {
    let cover_w = (w as f32 * 0.15) as i32;
    let cover_h = (h as f32 * 0.17) as i32;
    stamp_floor_patch_sampled(
        canvas,
        Some(bg),
        cx - cover_w / 2,
        cy - cover_h / 2,
        cover_w,
        cover_h,
    );
}

/// Soft boardroom rug under supervisor (burgundy oval).
fn paint_boss_rug(canvas: &mut RgbaImage, cx: i32, cy: i32, cover_w: i32, cover_h: i32) {
    let rug: [u8; 4] = [120, 48, 72, 255];
    let (cw, ch) = canvas.dimensions();
    for dy in 0..cover_h {
        for dx in 0..cover_w {
            let x = cx - cover_w / 2 + dx;
            let y = cy - cover_h / 4 + dy;
            if x < 0 || y < 0 || (x as u32) >= cw || (y as u32) >= ch {
                continue;
            }
            let nx = (dx as f32 / cover_w as f32 - 0.5) * 2.0;
            let ny = (dy as f32 / cover_h as f32 - 0.5) * 2.0;
            if nx * nx + ny * ny * 1.4 >= 1.0 {
                continue;
            }
            let p = canvas.get_pixel(x as u32, y as u32).0;
            canvas.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([
                    ((u16::from(p[0]) + u16::from(rug[0]) * 2) / 3) as u8,
                    ((u16::from(p[1]) + u16::from(rug[1]) * 2) / 3) as u8,
                    ((u16::from(p[2]) + u16::from(rug[2]) * 2) / 3) as u8,
                    255,
                ]),
            );
        }
    }
}

// Focus ring is painted as a ratatui cell overlay in `render.rs` so hover-only
// frames never force a full pixel recompose (see GameModeState::visual_fingerprint).

/// Celebrate sparkles / fail flash over a seated developer.
fn paint_fx_celebrate(canvas: &mut RgbaImage, cx: i32, cy: i32, frame: u8) {
    let colors = [
        [255, 220, 96, 255],
        [120, 255, 180, 255],
        [120, 200, 255, 255],
        [255, 120, 180, 255],
    ];
    for i in 0..6 {
        let a = (frame as i32 * 40 + i * 55) % 360;
        let rad = (a as f32).to_radians();
        let r = 10 + (frame as i32 % 3);
        let x = cx + (rad.cos() * r as f32) as i32;
        let y = cy - 8 + (rad.sin() * r as f32) as i32;
        if x >= 0 && y >= 0 {
            let (cw, ch) = canvas.dimensions();
            if (x as u32) < cw && (y as u32) < ch {
                canvas.put_pixel(x as u32, y as u32, image::Rgba(colors[(i as usize) % 4]));
            }
        }
    }
}

fn paint_fx_fail(canvas: &mut RgbaImage, cx: i32, cy: i32, frame: u8) {
    if frame % 2 != 0 {
        return;
    }
    // Red alert flash above monitor
    for dx in -4..5 {
        for dy in -10..-6 {
            let x = cx + dx;
            let y = cy + dy;
            if x >= 0 && y >= 0 {
                let (cw, ch) = canvas.dimensions();
                if (x as u32) < cw && (y as u32) < ch {
                    let p = canvas.get_pixel(x as u32, y as u32).0;
                    canvas.put_pixel(
                        x as u32,
                        y as u32,
                        image::Rgba([
                            p[0].saturating_add(80),
                            p[1].saturating_sub(20),
                            p[2].saturating_sub(20),
                            255,
                        ]),
                    );
                }
            }
        }
    }
}

/// Door x in canvas pixels.
fn door_x(w: u32) -> f32 {
    w as f32 * DOOR_X_FRAC
}

/// Walking-actor centre (canvas pixels) for `phase` at `anim_t`.
///
/// `cx`/`cy` are the actor's own desk anchor:
/// - `SpawnWalk` slides in from the door at desk height,
/// - `WalkToBoss` crosses desk → supervisor,
/// - `Handoff` stands on the rug (position does not depend on `anim_t` — see
///   [`GameModeState::visual_fingerprint`], which no longer hashes it),
/// - `ExitDoor` mirrors the entry back out: rug → door (RC16 BUG-3; it used to
///   restart 45% back along the desk line and walk *into* the supervisor again).
fn walk_position(phase: ActorPhase, anim_t: f32, cx: i32, cy: i32, w: u32, h: u32) -> (f32, f32) {
    let t = anim_t.clamp(0.0, 1.0);
    let (sx, sy) = SUPERVISOR_ANCHOR;
    let sup_x = sx * w as f32;
    let sup_y = sy * h as f32;
    match phase {
        ActorPhase::SpawnWalk => {
            let dx = door_x(w);
            (dx + (cx as f32 - dx) * t, cy as f32)
        }
        ActorPhase::Handoff => (sup_x, sup_y),
        ActorPhase::ExitDoor => (
            sup_x + (door_x(w) - sup_x) * t,
            sup_y + (cy as f32 - sup_y) * t,
        ),
        _ => (
            cx as f32 + (sup_x - cx as f32) * t,
            cy as f32 + (sup_y - cy as f32) * t,
        ),
    }
}

/// Composite sprites onto a clone of the scaled office background.
///
/// Prefer [`compose_cell_frame_into`] with a reused canvas to avoid allocating
/// a full-frame clone on every compose miss.
///
/// PERF INVARIANTS:
/// - Caller must skip this when `visual_fingerprint` is unchanged.
/// - Does **not** paint hover focus ring (buffer overlay in `render.rs`).
/// - Does **not** paint status strip / hover popup (buffer overlays).
/// - Ambient plant/coffee and character sprites come from the scaled cache.
/// - Idle/Waiting supervisor uses frame 0 so pure-idle ticks can freeze.
pub fn compose_cell_frame(bg_scaled: &RgbaImage, state: &GameModeState, tick: u64) -> RgbaImage {
    let mut canvas = RgbaImage::new(bg_scaled.width(), bg_scaled.height());
    compose_cell_frame_into(&mut canvas, bg_scaled, state, tick);
    canvas
}

/// Composite into `canvas`, reusing its allocation when dimensions match.
///
/// Resets from `bg_scaled` via `copy_from` (no full clone when sizes match).
pub fn compose_cell_frame_into(
    canvas: &mut RgbaImage,
    bg_scaled: &RgbaImage,
    state: &GameModeState,
    tick: u64,
) {
    let (bw, bh) = bg_scaled.dimensions();
    if canvas.dimensions() != (bw, bh) {
        *canvas = RgbaImage::new(bw, bh);
    }
    // copy_from is O(pixels) but reuses the destination allocation across misses.
    let _ = image::imageops::replace(canvas, bg_scaled, 0, 0);
    let (w, h) = canvas.dimensions();
    let frame = ((tick / 4) % 4) as u8;
    let sc = desk_scale(w);
    let walk_sc = ((w as f32 * 0.05) / 14.0).max(1.0).round().min(5.0) as u32;
    let prop_sc = sc.max(1).min(3);

    // Ambient props (plants / coffee) near room edges — cached static sprites.
    {
        let plant = cached_plant(prop_sc);
        let coffee = cached_coffee(prop_sc);
        blit(
            canvas,
            plant.as_ref(), (w as f32 * 0.06) as i32, (h as f32 * 0.62) as i32);
        blit(
            canvas,
            plant.as_ref(),
            (w as f32 * 0.90) as i32,
            (h as f32 * 0.58) as i32,
        );
        blit(
            canvas,
            coffee.as_ref(),
            (w as f32 * 0.88) as i32,
            (h as f32 * 0.40) as i32,
        );
    }

    // Supervisor
    {
        let (sx, sy) = SUPERVISOR_ANCHOR;
        let cx = (sx * w as f32) as i32;
        let cy = (sy * h as f32) as i32;
        let cover_w = (w as f32 * 0.13) as i32;
        let cover_h = (h as f32 * 0.14) as i32;
        stamp_floor_patch_sampled(
            canvas,
            Some(bg_scaled),
            cx - cover_w / 2,
            cy - cover_h / 2,
            cover_w,
            cover_h,
        );
        paint_boss_rug(canvas, cx, cy, cover_w, cover_h);
        let phase = match state.supervisor {
            SupervisorPhase::Working => 1u8,
            SupervisorPhase::Reviewing => 2,
            SupervisorPhase::Idle | SupervisorPhase::Waiting => 0,
        };
        // Freeze idle/waiting pose so pure tick animation can skip recompose.
        let sup_frame = if matches!(
            state.supervisor,
            SupervisorPhase::Idle | SupervisorPhase::Waiting
        ) {
            0
        } else {
            frame
        };
        let ssc = ((w as f32 * 0.072) / 26.0).max(1.0).round().min(5.0) as u32;
        let spr = cached_supervisor(phase, sup_frame, ssc.max(1));
        blit(
            canvas,
            spr.as_ref(),
            cx - spr.width() as i32 / 2,
            cy - spr.height() as i32 / 2,
        );
    }

    // Six desks — no hover ring here (see render::paint_focus_ring_overlay).
    for i in 0..6 {
        let (ax, ay) = DESK_ANCHORS[i];
        let cx = (ax * w as f32) as i32;
        let cy = (ay * h as f32) as i32;
        let desk = &state.desks[i];

        if desk.is_empty() {
            clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
            let spr = cached_empty_desk(sc.max(1));
            blit(
            canvas,
            spr.as_ref(),
                cx - spr.width() as i32 / 2,
                cy - spr.height() as i32 / 2,
            );
            continue;
        }

        match desk.phase {
            ActorPhase::WalkToBoss | ActorPhase::ExitDoor | ActorPhase::Handoff => {
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                let empty = cached_empty_desk(sc.max(1));
                blit(
            canvas,
            empty.as_ref(),
                    cx - empty.width() as i32 / 2,
                    cy - empty.height() as i32 / 2,
                );
                // Packet baked into walk sprite — no second packet blit (double handoff fix).
                let with_packet = matches!(
                    desk.phase,
                    ActorPhase::WalkToBoss | ActorPhase::Handoff
                );
                let walker = cached_walk(desk.skin, frame, with_packet, walk_sc.max(1));
                let (x, y) = walk_position(desk.phase, desk.anim_t, cx, cy, w, h);
                blit(
            canvas,
            walker.as_ref(),
                    x as i32 - walker.width() as i32 / 2,
                    y as i32 - walker.height() as i32 / 2,
                );
            }
            ActorPhase::Celebrate => {
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, false, frame, sc.max(1));
                blit(
            canvas,
            spr.as_ref(),
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
                paint_fx_celebrate(canvas, cx, cy, frame);
            }
            ActorPhase::FailBeat => {
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, false, 0, sc.max(1));
                blit(
            canvas,
            spr.as_ref(),
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
                paint_fx_fail(canvas, cx, cy, frame);
            }
            ActorPhase::SpawnWalk => {
                // Slide from door (left) toward desk using anim_t (matches Unicode path).
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                let empty = cached_empty_desk(sc.max(1));
                blit(
                    canvas,
                    empty.as_ref(),
                    cx - empty.width() as i32 / 2,
                    cy - empty.height() as i32 / 2,
                );
                let walker = cached_walk(desk.skin, frame, false, walk_sc.max(1));
                let (x, y) = walk_position(desk.phase, desk.anim_t, cx, cy, w, h);
                blit(
                    canvas,
                    walker.as_ref(),
                    x as i32 - walker.width() as i32 / 2,
                    y as i32 - walker.height() as i32 / 2,
                );
            }
            ActorPhase::AtDeskWorking => {
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, true, frame, sc.max(1));
                blit(
                    canvas,
                    spr.as_ref(),
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
            ActorPhase::AtDeskThinking => {
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, false, 0, sc.max(1));
                blit(
                    canvas,
                    spr.as_ref(),
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::game_mode::state::GameModeState;

    #[test]
    fn load_and_scale_is_high_res() {
        let full = load_office_background().expect("bg");
        let scaled = scale_bg_to_cells(&full, 80, 24);
        let s = crate::views::game_mode::sprites_pixel::effective_pixel_scale(80, 24).max(1);
        assert_eq!(scaled.width(), 80 * s);
        assert_eq!(scaled.height(), 48 * s);
    }

    /// BUG-3: the exit walk used to reuse the desk→supervisor line, so its first
    /// frame teleported 45% backwards and the actor then walked back into the
    /// supervisor. It must leave the rug and reach the door instead.
    #[test]
    fn exit_door_walks_from_supervisor_to_the_door() {
        let (w, h) = (400u32, 200u32);
        let (cx, cy) = (300i32, 150i32); // bottom-right desk anchor
        let (sup_x, sup_y) = (
            SUPERVISOR_ANCHOR.0 * w as f32,
            SUPERVISOR_ANCHOR.1 * h as f32,
        );

        let start = walk_position(ActorPhase::ExitDoor, 0.0, cx, cy, w, h);
        assert!(
            (start.0 - sup_x).abs() < 1.0 && (start.1 - sup_y).abs() < 1.0,
            "exit must start where the handoff left the actor, got {start:?}"
        );

        let mut prev = start.0;
        for step in 1..=10 {
            let t = step as f32 / 10.0;
            let (x, _) = walk_position(ActorPhase::ExitDoor, t, cx, cy, w, h);
            assert!(x < prev, "exit walk must keep moving left (t={t}, x={x})");
            prev = x;
        }

        let end = walk_position(ActorPhase::ExitDoor, 1.0, cx, cy, w, h);
        assert!(
            (end.0 - door_x(w)).abs() < 1.0,
            "exit must finish at the door, got {end:?}"
        );
        assert!(
            (end.0 - sup_x).abs() > (w as f32 * 0.3),
            "exit must not vanish on the supervisor"
        );
        assert!(
            (end.1 - cy as f32).abs() < 1.0,
            "exit must drop back to the desk row it entered on, got {end:?}"
        );
    }

    /// The other walk phases keep their RC13 geometry.
    #[test]
    fn spawn_and_boss_walks_keep_their_endpoints() {
        let (w, h) = (400u32, 200u32);
        let (cx, cy) = (300i32, 150i32);
        let (sup_x, sup_y) = (
            SUPERVISOR_ANCHOR.0 * w as f32,
            SUPERVISOR_ANCHOR.1 * h as f32,
        );

        let spawn0 = walk_position(ActorPhase::SpawnWalk, 0.0, cx, cy, w, h);
        assert!(
            (spawn0.0 - door_x(w)).abs() < 1.0,
            "spawn enters at the door"
        );
        let spawn1 = walk_position(ActorPhase::SpawnWalk, 1.0, cx, cy, w, h);
        assert_eq!((spawn1.0 as i32, spawn1.1 as i32), (cx, cy));

        let boss1 = walk_position(ActorPhase::WalkToBoss, 1.0, cx, cy, w, h);
        assert!((boss1.0 - sup_x).abs() < 1.0 && (boss1.1 - sup_y).abs() < 1.0);

        // Handoff is pinned on the rug for its whole duration.
        for t in [0.0, 0.5, 1.0] {
            let p = walk_position(ActorPhase::Handoff, t, cx, cy, w, h);
            assert_eq!((p.0 as i32, p.1 as i32), (sup_x as i32, sup_y as i32));
        }
    }

    #[test]
    fn compose_cell_frame_matches_bg_size() {
        let full = load_office_background().unwrap();
        let bg = scale_bg_to_cells(&full, 60, 20);
        let state = GameModeState::new();
        let frame = compose_cell_frame(&bg, &state, 0);
        assert_eq!(frame.dimensions(), bg.dimensions());
    }
}
