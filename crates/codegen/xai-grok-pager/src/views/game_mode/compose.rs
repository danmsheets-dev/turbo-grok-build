//! Composite office background + sprites at **cell resolution**.
//!
//! Visual rules (user feedback):
//! - No brown top bar / no giant WORKING text / no yellow status squares
//! - Working agents type; idle agents stop typing
//! - Handoff/walk: empty desk (agent not seated)
//! - Supervisor: laptop open+type when busy; closed+coffee when idle
//! - Square monitors; animate when working

use image::imageops::FilterType;
use image::RgbaImage;

use super::sprites_pixel::{
    DevPalette, blit, scale_nn, sprite_developer_at_desk, sprite_developer_walk, sprite_empty_desk,
    sprite_packet, sprite_supervisor, stamp_floor_patch,
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

pub fn scale_bg_to_cells(full: &RgbaImage, cell_w: u16, cell_h: u16) -> RgbaImage {
    let tw = u32::from(cell_w).max(1);
    let th = u32::from(cell_h).saturating_mul(2).max(1);
    image::imageops::resize(full, tw, th, FilterType::Triangle)
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

fn desk_scale(w: u32) -> u32 {
    ((w as f32 * 0.11) / 36.0).max(1.0).round().min(4.0) as u32
}

/// Clear baked mockup character + furniture in a desk region, then draw sprite.
fn clear_desk_area(canvas: &mut RgbaImage, cx: i32, cy: i32, w: u32, h: u32) {
    let cover_w = (w as f32 * 0.18) as i32;
    let cover_h = (h as f32 * 0.20) as i32;
    stamp_floor_patch(
        canvas,
        cx - cover_w / 2,
        cy - cover_h / 2,
        cover_w,
        cover_h,
    );
}

pub fn compose_cell_frame(bg_scaled: &RgbaImage, state: &GameModeState, tick: u64) -> RgbaImage {
    let mut canvas = bg_scaled.clone();
    let (w, h) = canvas.dimensions();
    let frame = ((tick / 4) % 4) as u8;
    let sc = desk_scale(w);

    // Supervisor (always redraw so laptop/coffee state is correct)
    {
        let (sx, sy) = SUPERVISOR_ANCHOR;
        let cx = (sx * w as f32) as i32;
        let cy = (sy * h as f32) as i32;
        // Clear boss seat on rug
        let cover_w = (w as f32 * 0.16) as i32;
        let cover_h = (h as f32 * 0.16) as i32;
        stamp_floor_patch(
            &mut canvas,
            cx - cover_w / 2,
            cy - cover_h / 2,
            cover_w,
            cover_h,
        );
        // rug tint under boss
        let rug = [90, 40, 60, 255];
        for dy in 0..cover_h {
            for dx in 0..cover_w {
                let x = cx - cover_w / 2 + dx;
                let y = cy - cover_h / 3 + dy;
                if x >= 0 && y >= 0 {
                    let (cw, ch) = canvas.dimensions();
                    if (x as u32) < cw && (y as u32) < ch && ((dx + dy) % 5) < 3 {
                        let p = canvas.get_pixel(x as u32, y as u32).0;
                        // blend slightly toward rug
                        canvas.put_pixel(
                            x as u32,
                            y as u32,
                            image::Rgba([
                                ((p[0] as u16 * 2 + rug[0] as u16) / 3) as u8,
                                ((p[1] as u16 * 2 + rug[1] as u16) / 3) as u8,
                                ((p[2] as u16 * 2 + rug[2] as u16) / 3) as u8,
                                255,
                            ]),
                        );
                    }
                }
            }
        }
        let phase = match state.supervisor {
            SupervisorPhase::Working => 1u8,
            SupervisorPhase::Reviewing => 2,
            SupervisorPhase::Idle | SupervisorPhase::Waiting => 0,
        };
        let mut spr = sprite_supervisor(phase, frame);
        let ssc = ((w as f32 * 0.10) / spr.width() as f32)
            .max(1.0)
            .round()
            .min(4.0) as u32;
        spr = scale_nn(&spr, ssc.max(1));
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

        if desk.is_empty() {
            // Idle slot: clear baked person → empty desk
            clear_desk_area(&mut canvas, cx, cy, w, h);
            let mut spr = sprite_empty_desk();
            spr = scale_nn(&spr, sc.max(1));
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
                // Away from desk
                clear_desk_area(&mut canvas, cx, cy, w, h);
                let mut empty = sprite_empty_desk();
                empty = scale_nn(&empty, sc.max(1));
                blit(
                    &mut canvas,
                    &empty,
                    cx - empty.width() as i32 / 2,
                    cy - empty.height() as i32 / 2,
                );
                let pal = DevPalette::by_index(desk.skin);
                let with_packet = matches!(
                    desk.phase,
                    ActorPhase::WalkToBoss | ActorPhase::Handoff | ActorPhase::Celebrate
                );
                let mut walker = sprite_developer_walk(pal, frame, with_packet);
                let wsc = ((w as f32 * 0.07) / walker.width() as f32)
                    .max(1.0)
                    .round() as u32;
                walker = scale_nn(&walker, wsc.max(1));
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
                if matches!(desk.phase, ActorPhase::Handoff) {
                    let pkt = scale_nn(&sprite_packet(), wsc.max(1));
                    blit(
                        &mut canvas,
                        &pkt,
                        x as i32 + 4,
                        y as i32 - 4,
                    );
                }
            }
            ActorPhase::Celebrate => {
                // Brief celebrate at desk then they leave (still seated for now)
                clear_desk_area(&mut canvas, cx, cy, w, h);
                let pal = DevPalette::by_index(desk.skin);
                let mut spr = sprite_developer_at_desk(pal, false, frame);
                spr = scale_nn(&spr, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
            ActorPhase::FailBeat => {
                clear_desk_area(&mut canvas, cx, cy, w, h);
                let pal = DevPalette::by_index(desk.skin);
                let mut spr = sprite_developer_at_desk(pal, false, 0);
                spr = scale_nn(&spr, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
            ActorPhase::AtDeskWorking | ActorPhase::SpawnWalk => {
                // Working: typing + animated square monitor
                clear_desk_area(&mut canvas, cx, cy, w, h);
                let pal = DevPalette::by_index(desk.skin);
                let mut spr = sprite_developer_at_desk(pal, true, frame);
                spr = scale_nn(&spr, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
            ActorPhase::AtDeskThinking => {
                // Idle at desk: not typing; monitor frozen/dim content
                clear_desk_area(&mut canvas, cx, cy, w, h);
                let pal = DevPalette::by_index(desk.skin);
                let mut spr = sprite_developer_at_desk(pal, false, 0);
                spr = scale_nn(&spr, sc.max(1));
                blit(
                    &mut canvas,
                    &spr,
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
            }
        }
    }

    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::game_mode::state::GameModeState;

    #[test]
    fn load_and_scale_is_small() {
        let full = load_office_background().expect("bg");
        let scaled = scale_bg_to_cells(&full, 80, 24);
        assert_eq!(scaled.width(), 80);
        assert_eq!(scaled.height(), 48);
    }

    #[test]
    fn compose_cell_frame_is_cheap_size() {
        let full = load_office_background().unwrap();
        let bg = scale_bg_to_cells(&full, 60, 20);
        let state = GameModeState::new();
        let frame = compose_cell_frame(&bg, &state, 0);
        assert_eq!(frame.dimensions(), bg.dimensions());
    }
}
