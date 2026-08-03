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

pub fn load_office_background() -> Result<RgbaImage, String> {
    image::load_from_memory(OFFICE_BG_PNG)
        .map(|i| i.to_rgba8())
        .map_err(|e| format!("decode office bg: {e}"))
}

/// Scale background to high internal resolution (PIXEL_SCALE × halfblock grid).
///
/// Terminal halfblock paint maps `cell_w × cell_h*2` → paint. We compose at
/// `cell_w*SCALE × cell_h*2*SCALE` so sprites keep crisp SNES detail, then
/// `paint_halfblock_rgba` box-filters down.
pub fn scale_bg_to_cells(full: &RgbaImage, cell_w: u16, cell_h: u16) -> RgbaImage {
    let scale = super::sprites_pixel::pixel_scale().max(1);
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

// Thread-local scaled sprite cache — avoid rebuild+scale every paint (~8–10 Hz).
thread_local! {
    static SPRITE_CACHE: std::cell::RefCell<std::collections::HashMap<u64, RgbaImage>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn cache_get_or_insert(key: u64, build: impl FnOnce() -> RgbaImage) -> RgbaImage {
    SPRITE_CACHE.with(|c| {
        let mut map = c.borrow_mut();
        // Cap cache so palette/scale churn cannot grow unbounded.
        if map.len() > 128 {
            map.clear();
        }
        map.entry(key).or_insert_with(build).clone()
    })
}

fn cached_empty_desk(sc: u32) -> RgbaImage {
    cache_get_or_insert(0xE0u64 << 56 | sc as u64, || {
        scale_nn(&sprite_empty_desk(), sc.max(1))
    })
}

fn cached_dev_at_desk(skin: u8, typing: bool, frame: u8, sc: u32) -> RgbaImage {
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

fn cached_walk(skin: u8, frame: u8, with_packet: bool, sc: u32) -> RgbaImage {
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

fn cached_supervisor(phase: u8, frame: u8, sc: u32) -> RgbaImage {
    let key = (0xA0u64 << 56) | ((phase as u64) << 32) | ((frame as u64) << 24) | sc as u64;
    cache_get_or_insert(key, || {
        scale_nn(&sprite_supervisor(phase, frame), sc.max(1))
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

/// Gold focus ring for keyboard/mouse hover desk (SNES selector).
fn paint_focus_ring(canvas: &mut RgbaImage, cx: i32, cy: i32, sw: i32, sh: i32, pulse: u8) {
    let gold = if pulse % 2 == 0 {
        [255, 220, 96, 255]
    } else {
        [255, 200, 48, 255]
    };
    let x0 = cx - sw / 2 - 2;
    let y0 = cy - sh / 2 - 2;
    let ww = sw + 4;
    let hh = sh + 4;
    // Corner brackets only — less noisy than full box.
    for (dx, dy) in [
        (0, 0),
        (1, 0),
        (2, 0),
        (0, 1),
        (0, 2),
        (ww - 1, 0),
        (ww - 2, 0),
        (ww - 3, 0),
        (ww - 1, 1),
        (ww - 1, 2),
        (0, hh - 1),
        (1, hh - 1),
        (2, hh - 1),
        (0, hh - 2),
        (0, hh - 3),
        (ww - 1, hh - 1),
        (ww - 2, hh - 1),
        (ww - 3, hh - 1),
        (ww - 1, hh - 2),
        (ww - 1, hh - 3),
    ] {
        let x = x0 + dx;
        let y = y0 + dy;
        if x >= 0 && y >= 0 {
            let (cw, ch) = canvas.dimensions();
            if (x as u32) < cw && (y as u32) < ch {
                canvas.put_pixel(x as u32, y as u32, image::Rgba(gold));
            }
        }
    }
}

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

pub fn compose_cell_frame(bg_scaled: &RgbaImage, state: &GameModeState, tick: u64) -> RgbaImage {
    let mut canvas = bg_scaled.clone();
    let (w, h) = canvas.dimensions();
    let frame = ((tick / 4) % 4) as u8;
    let sc = desk_scale(w);
    let walk_sc = ((w as f32 * 0.05) / 14.0).max(1.0).round().min(5.0) as u32;

    // Ambient props (plants / coffee) near room edges — cheap static sprites.
    {
        let plant = scale_nn(&sprite_plant(), sc.max(1).min(3));
        let coffee = scale_nn(&sprite_coffee(), sc.max(1).min(3));
        blit(&mut canvas, &plant, (w as f32 * 0.06) as i32, (h as f32 * 0.62) as i32);
        blit(
            &mut canvas,
            &plant,
            (w as f32 * 0.90) as i32,
            (h as f32 * 0.58) as i32,
        );
        blit(
            &mut canvas,
            &coffee,
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
            &mut canvas,
            Some(bg_scaled),
            cx - cover_w / 2,
            cy - cover_h / 2,
            cover_w,
            cover_h,
        );
        paint_boss_rug(&mut canvas, cx, cy, cover_w, cover_h);
        let phase = match state.supervisor {
            SupervisorPhase::Working => 1u8,
            SupervisorPhase::Reviewing => 2,
            SupervisorPhase::Idle | SupervisorPhase::Waiting => 0,
        };
        let ssc = ((w as f32 * 0.072) / 26.0).max(1.0).round().min(5.0) as u32;
        let spr = cached_supervisor(phase, frame, ssc.max(1));
        blit(
            &mut canvas,
            &spr,
            cx - spr.width() as i32 / 2,
            cy - spr.height() as i32 / 2,
        );
    }

    // Six desks
    for i in 0..6 {
        let (ax, ay) = DESK_ANCHORS[i];
        let cx = (ax * w as f32) as i32;
        let cy = (ay * h as f32) as i32;
        let desk = &state.desks[i];
        let focused = state.hover_desk == Some(i);

        if desk.is_empty() {
            clear_desk_area(&mut canvas, bg_scaled, cx, cy, w, h);
            let spr = cached_empty_desk(sc.max(1));
            blit(
                &mut canvas,
                &spr,
                cx - spr.width() as i32 / 2,
                cy - spr.height() as i32 / 2,
            );
            continue;
        }

        match desk.phase {
            ActorPhase::WalkToBoss | ActorPhase::ExitDoor | ActorPhase::Handoff => {
                clear_desk_area(&mut canvas, bg_scaled, cx, cy, w, h);
                let empty = cached_empty_desk(sc.max(1));
                blit(
                    &mut canvas,
                    &empty,
                    cx - empty.width() as i32 / 2,
                    cy - empty.height() as i32 / 2,
                );
                // Packet baked into walk sprite — no second packet blit (double handoff fix).
                let with_packet = matches!(
                    desk.phase,
                    ActorPhase::WalkToBoss | ActorPhase::Handoff
                );
                let walker = cached_walk(desk.skin, frame, with_packet, walk_sc.max(1));
                let (tx, ty) = SUPERVISOR_ANCHOR;
                let t = match desk.phase {
                    ActorPhase::Handoff => 1.0,
                    ActorPhase::ExitDoor => 0.55 + desk.anim_t * 0.45,
                    _ => desk.anim_t.clamp(0.0, 1.0),
                };
                let x = cx as f32 + (tx * w as f32 - cx as f32) * t;
                let y = cy as f32 + (ty * h as f32 - cy as f32) * t;
                blit(
                    &mut canvas,
                    &walker,
                    x as i32 - walker.width() as i32 / 2,
                    y as i32 - walker.height() as i32 / 2,
                );
            }
            ActorPhase::Celebrate => {
                clear_desk_area(&mut canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, false, frame, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
                paint_fx_celebrate(&mut canvas, cx, cy, frame);
            }
            ActorPhase::FailBeat => {
                clear_desk_area(&mut canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, false, 0, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
                paint_fx_fail(&mut canvas, cx, cy, frame);
            }
            ActorPhase::AtDeskWorking | ActorPhase::SpawnWalk => {
                clear_desk_area(&mut canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, true, frame, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
            ActorPhase::AtDeskThinking => {
                clear_desk_area(&mut canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_at_desk(desk.skin, false, 0, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
        }

        if focused {
            let ring_w = (w as f32 * 0.12) as i32;
            let ring_h = (h as f32 * 0.14) as i32;
            paint_focus_ring(&mut canvas, cx, cy, ring_w, ring_h, frame);
        }
    }

    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::game_mode::state::GameModeState;

    #[test]
    fn load_and_scale_is_high_res() {
        let full = load_office_background().expect("bg");
        let scaled = scale_bg_to_cells(&full, 80, 24);
        let s = crate::views::game_mode::sprites_pixel::pixel_scale().max(1);
        assert_eq!(scaled.width(), 80 * s);
        assert_eq!(scaled.height(), 48 * s);
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
