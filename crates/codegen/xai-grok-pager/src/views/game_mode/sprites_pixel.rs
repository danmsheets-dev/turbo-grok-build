//! SNES / SimCity-style procedural sprites for Game Mode (16-bit pixel look).
//!
//! Visual goals:
//! - Saturated but readable palettes (city-builder office vibe)
//! - 1px dark outlines, soft dither / edge highlights
//! - Smaller base sprites so the office feels larger
//! - Floor tiles that match the teal office carpet (no diagonal green mask)

use image::{Rgba, RgbaImage};

/// Higher-res compose scale: internal frame is `PIXEL_SCALE` times denser
/// than one terminal halfblock cell on each axis. paint_halfblock downsamples.
///
/// Override with env `GROK_GAME_PIXEL_SCALE` = 2|3|4.
/// Default base is **3**; large stages drop to **2** via [`effective_pixel_scale`]
/// unless the env override is set.
pub fn pixel_scale() -> u32 {
    static CACHED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("GROK_GAME_PIXEL_SCALE")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|n| (2..=4).contains(n))
            .unwrap_or(3)
    })
}

/// True when `GROK_GAME_PIXEL_SCALE` explicitly pinned the scale.
fn pixel_scale_env_pinned() -> bool {
    static PINNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PINNED.get_or_init(|| {
        std::env::var("GROK_GAME_PIXEL_SCALE")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .is_some_and(|n| (2..=4).contains(&n))
    })
}

/// Adaptive scale for a stage size (triple-scan P2).
///
/// When the terminal stage is large (`cell_w * cell_h >= 4800`, e.g. ~100×48)
/// and the env does not pin scale, use **2** instead of the default **3** to
/// cut compose pixels ~44%.
pub fn effective_pixel_scale(cell_w: u16, cell_h: u16) -> u32 {
    let base = pixel_scale().max(1);
    if pixel_scale_env_pinned() {
        return base;
    }
    let cells = u32::from(cell_w).saturating_mul(u32::from(cell_h));
    if cells >= 4800 {
        base.min(2)
    } else {
        base
    }
}

/// Default scale constant (for docs/tests). Prefer [`pixel_scale()`] /
/// [`effective_pixel_scale`] at runtime.
pub const PIXEL_SCALE: u32 = 3;

/// Developer palette: shirt + skin + hair + pants + accent.
#[derive(Debug, Clone, Copy)]
pub struct DevPalette {
    pub shirt: [u8; 4],
    pub skin: [u8; 4],
    pub hair: [u8; 4],
    pub pants: [u8; 4],
    pub accent: [u8; 4],
}

impl DevPalette {
    pub fn by_index(i: u8) -> Self {
        // SimCity-ish saturated city folk
        match i % 6 {
            0 => Self {
                shirt: [56, 176, 188, 255],
                skin: [232, 168, 128, 255],
                hair: [72, 48, 40, 255],
                pants: [48, 64, 104, 255],
                accent: [255, 220, 96, 255],
            },
            1 => Self {
                shirt: [72, 196, 96, 255],
                skin: [248, 200, 160, 255],
                hair: [40, 36, 48, 255],
                pants: [56, 56, 80, 255],
                accent: [255, 120, 80, 255],
            },
            2 => Self {
                shirt: [232, 88, 120, 255],
                skin: [216, 152, 120, 255],
                hair: [160, 48, 64, 255],
                pants: [64, 48, 80, 255],
                accent: [120, 220, 255, 255],
            },
            3 => Self {
                shirt: [255, 176, 48, 255],
                skin: [240, 184, 136, 255],
                hair: [96, 64, 32, 255],
                pants: [48, 72, 120, 255],
                accent: [88, 200, 160, 255],
            },
            4 => Self {
                shirt: [120, 96, 220, 255],
                skin: [224, 176, 144, 255],
                hair: [32, 32, 48, 255],
                pants: [72, 56, 40, 255],
                accent: [255, 96, 160, 255],
            },
            _ => Self {
                shirt: [48, 160, 200, 255],
                skin: [255, 208, 168, 255],
                hair: [200, 88, 40, 255],
                pants: [40, 56, 88, 255],
                accent: [180, 255, 120, 255],
            },
        }
    }
}

const CLEAR: [u8; 4] = [0, 0, 0, 0];
const OUTLINE: [u8; 4] = [24, 28, 36, 255];

// Floor stamp helpers: runtime desk clears sample the office carpet; tests too.
mod floor_stamp {
    use super::*;

    /// SNES office carpet — warm teal tiles (matches mockup floor).
    pub const FLOOR_A: [u8; 4] = [42, 108, 112, 255];
    pub const FLOOR_B: [u8; 4] = [36, 96, 100, 255];
    pub const FLOOR_HI: [u8; 4] = [56, 128, 132, 255];
    pub const FLOOR_LO: [u8; 4] = [28, 80, 84, 255];

    /// SNES carpet tile (8×8) — checker with edge bevel.
    pub fn floor_tile_at(wx: i32, wy: i32) -> [u8; 4] {
        let tx = wx.rem_euclid(8);
        let ty = wy.rem_euclid(8);
        let tile = ((wx.div_euclid(8)) + (wy.div_euclid(8))).rem_euclid(2);
        let base = if tile == 0 { FLOOR_A } else { FLOOR_B };
        if tx == 0 || ty == 0 {
            return FLOOR_HI;
        }
        if tx == 7 || ty == 7 {
            return FLOOR_LO;
        }
        if (tx + ty) % 5 == 0 {
            FLOOR_LO
        } else if (tx * 3 + ty) % 7 == 0 {
            FLOOR_HI
        } else {
            base
        }
    }

    /// Procedural floor (tests / Unicode fallback paths only).
    #[cfg(test)]
    pub fn stamp_floor_patch(dest: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32) {
        let (dw, dh) = dest.dimensions();
        for dy in 0..h {
            for dx in 0..w {
                let px_ = x + dx;
                let py = y + dy;
                if px_ < 0 || py < 0 || px_ as u32 >= dw || py as u32 >= dh {
                    continue;
                }
                dest.put_pixel(px_ as u32, py as u32, Rgba(floor_tile_at(px_, py)));
            }
        }
    }

    pub fn stamp_floor_patch_sampled(
        dest: &mut RgbaImage,
        bg: Option<&RgbaImage>,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) {
        let (dw, dh) = dest.dimensions();
        let sample = bg.and_then(|img| {
            let (bw, bh) = img.dimensions();
            if bw < 8 || bh < 8 {
                return None;
            }
            let sx = (bw as f32 * 0.08) as u32;
            let sy = (bh as f32 * 0.88) as u32;
            Some(img.get_pixel(sx.min(bw - 1), sy.min(bh - 1)).0)
        });

        for dy in 0..h {
            for dx in 0..w {
                let px_ = x + dx;
                let py = y + dy;
                if px_ < 0 || py < 0 || px_ as u32 >= dw || py as u32 >= dh {
                    continue;
                }
                let c = if let Some(base) = sample {
                    let tile = floor_tile_at(px_, py);
                    let mix = |a: u8, b: u8| ((u16::from(a) * 2 + u16::from(b)) / 3) as u8;
                    [mix(base[0], tile[0]), mix(base[1], tile[1]), mix(base[2], tile[2]), 255]
                } else {
                    floor_tile_at(px_, py)
                };
                dest.put_pixel(px_ as u32, py as u32, Rgba(c));
            }
        }
    }
}

pub use floor_stamp::stamp_floor_patch_sampled;

fn px(img: &mut RgbaImage, x: i32, y: i32, c: [u8; 4]) {
    if x < 0 || y < 0 {
        return;
    }
    let (w, h) = img.dimensions();
    if (x as u32) >= w || (y as u32) >= h {
        return;
    }
    img.put_pixel(x as u32, y as u32, Rgba(c));
}

fn fill_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
    for dy in 0..h {
        for dx in 0..w {
            px(img, x + dx, y + dy, c);
        }
    }
}

fn outline_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
    for dx in 0..w {
        px(img, x + dx, y, c);
        px(img, x + dx, y + h - 1, c);
    }
    for dy in 0..h {
        px(img, x, y + dy, c);
        px(img, x + w - 1, y + dy, c);
    }
}

/// Draw a rounded-ish filled rect with dark outline (SNES sprite language).
fn filled_body(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, fill: [u8; 4]) {
    fill_rect(img, x + 1, y, w - 2, h, fill);
    fill_rect(img, x, y + 1, w, h - 2, fill);
    outline_rect(img, x, y, w, h, OUTLINE);
    // top highlight
    for dx in 1..w - 1 {
        let p = img.get_pixel((x + dx) as u32, (y + 1) as u32).0;
        px(
            img,
            x + dx,
            y + 1,
            [
                p[0].saturating_add(24),
                p[1].saturating_add(24),
                p[2].saturating_add(20),
                255,
            ],
        );
    }
}

/// Square monitor bezel. `active` scrolls code when true.
pub fn sprite_square_monitor(active: bool, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(12, 12, Rgba(CLEAR));
    let bezel = [40, 44, 56, 255];
    let edge = OUTLINE;
    filled_body(&mut img, 0, 0, 12, 11, bezel);
    fill_rect(&mut img, 4, 11, 4, 1, edge); // stand
    if !active {
        fill_rect(&mut img, 2, 2, 8, 7, [20, 26, 34, 255]);
        fill_rect(&mut img, 3, 3, 2, 1, [40, 48, 60, 255]);
        return img;
    }
    fill_rect(&mut img, 2, 2, 8, 7, [12, 20, 28, 255]);
    let greens = [
        [64, 232, 120, 255],
        [80, 200, 255, 255],
        [255, 200, 80, 255],
        [200, 120, 255, 255],
    ];
    let scroll = (frame % 4) as i32;
    for row in 0..4 {
        let y = 3 + row;
        let len = 2 + ((row + scroll) % 4);
        fill_rect(
            &mut img,
            3,
            y,
            len.min(6),
            1,
            greens[(row as usize + frame as usize) % 4],
        );
    }
    if frame % 2 == 0 {
        px(&mut img, 9, 8, [220, 255, 220, 255]);
    }
    img
}

/// Detailed empty desk + chair + dark monitor (worker station).
pub fn sprite_empty_desk() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(36, 30, Rgba(CLEAR));
    let wood = [184, 128, 72, 255];
    let wood_d = [128, 84, 44, 255];
    let wood_h = [208, 156, 96, 255];
    let chair = [56, 64, 88, 255];
    let chair_d = [40, 48, 68, 255];
    let metal = [88, 96, 112, 255];

    // Desk body + drawers
    filled_body(&mut img, 10, 14, 24, 8, wood);
    fill_rect(&mut img, 12, 15, 20, 1, wood_h);
    // left drawer unit
    filled_body(&mut img, 11, 18, 8, 6, wood_d);
    px(&mut img, 17, 20, metal);
    px(&mut img, 17, 22, metal);
    // right drawer unit
    filled_body(&mut img, 25, 18, 8, 6, wood_d);
    px(&mut img, 31, 20, metal);
    px(&mut img, 31, 22, metal);
    // legs
    fill_rect(&mut img, 12, 26, 2, 3, wood_d);
    fill_rect(&mut img, 30, 26, 2, 3, wood_d);
    // grain
    for gx in [14, 18, 22, 28] {
        px(&mut img, gx, 16, wood_d);
    }
    // keyboard (idle)
    filled_body(&mut img, 18, 18, 9, 3, [48, 52, 64, 255]);
    for kx in 0..4 {
        px(&mut img, 19 + kx * 2, 19, [72, 80, 96, 255]);
    }
    // mouse
    filled_body(&mut img, 28, 18, 3, 2, [40, 44, 56, 255]);
    // plant on desk corner
    fill_rect(&mut img, 32, 12, 2, 2, [64, 160, 80, 255]);
    fill_rect(&mut img, 32, 14, 2, 2, [160, 100, 60, 255]);
    // monitor
    let mon = sprite_square_monitor(false, 0);
    blit_local(&mut img, &mon, 18, 1);
    // office chair
    filled_body(&mut img, 1, 14, 9, 8, chair);
    fill_rect(&mut img, 2, 15, 7, 2, chair_d); // back cushion
    fill_rect(&mut img, 3, 22, 5, 2, chair_d); // seat depth
    // chair base + wheels
    fill_rect(&mut img, 4, 24, 3, 2, metal);
    px(&mut img, 2, 26, metal);
    px(&mut img, 8, 26, metal);
    px(&mut img, 5, 27, metal);
    img
}

/// Canonical `frame` for [`sprite_developer_at_desk`] (RC16 P8).
///
/// Two frames with the same key render byte-identical images, so the compose
/// sprite cache keys on this instead of the raw frame and stores no duplicates.
/// Declared here so it cannot drift from the sprite body:
/// - typing reads `frame % 2` (keys, mouse, arms, slim monitor) and forwards
///   `frame` to [`sprite_square_monitor`], which reads `% 4` — period **4**;
/// - idle pins the monitor to frame 0 and only reads `frame % 4 < 2` — **2**
///   distinct poses, so odd frames collapse onto their even neighbour.
pub fn dev_at_desk_frame_key(typing: bool, frame: u8) -> u8 {
    if typing {
        frame % 4
    } else if frame % 4 < 2 {
        0
    } else {
        2
    }
}

/// Developer at desk: detailed figure, animated typing, active monitor.
pub fn sprite_developer_at_desk(pal: DevPalette, typing: bool, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(36, 32, Rgba(CLEAR));
    let wood = [184, 128, 72, 255];
    let wood_d = [128, 84, 44, 255];
    let wood_h = [208, 156, 96, 255];
    let chair = [56, 64, 88, 255];
    let chair_d = [40, 48, 68, 255];
    let metal = [88, 96, 112, 255];

    // Desk
    filled_body(&mut img, 12, 15, 22, 8, wood);
    fill_rect(&mut img, 14, 16, 18, 1, wood_h);
    filled_body(&mut img, 13, 19, 7, 5, wood_d);
    filled_body(&mut img, 26, 19, 7, 5, wood_d);
    fill_rect(&mut img, 14, 27, 2, 3, wood_d);
    fill_rect(&mut img, 30, 27, 2, 3, wood_d);
    // keyboard
    filled_body(&mut img, 18, 19, 10, 3, [40, 44, 56, 255]);
    for kx in 0..5 {
        let key_c = if typing && (frame as i32 + kx) % 2 == 0 {
            [96, 200, 140, 255]
        } else {
            [72, 80, 96, 255]
        };
        px(&mut img, 19 + kx * 2, 20, key_c);
    }
    // mouse
    filled_body(&mut img, 29, 19, 3, 2, [36, 40, 52, 255]);
    if typing && frame % 2 == 0 {
        px(&mut img, 30, 19, pal.accent);
    }
    // sticky note / papers
    filled_body(&mut img, 31, 16, 3, 3, [255, 240, 140, 255]);
    px(&mut img, 32, 17, [80, 80, 100, 255]);

    // Active widescreen-ish monitor (reuse square + side glow)
    let mon = sprite_square_monitor(true, if typing { frame } else { 0 });
    blit_local(&mut img, &mon, 19, 1);
    // second slim monitor edge (dual-monitor vibe)
    filled_body(&mut img, 30, 3, 4, 9, [40, 44, 56, 255]);
    fill_rect(
        &mut img,
        31,
        4,
        2,
        6,
        if typing {
            [20, 48, 40, 255]
        } else {
            [16, 22, 30, 255]
        },
    );
    if typing && frame % 2 == 0 {
        px(&mut img, 31, 5, [80, 255, 140, 255]);
        px(&mut img, 32, 7, [80, 180, 255, 255]);
    }

    // Chair
    filled_body(&mut img, 1, 14, 10, 9, chair);
    fill_rect(&mut img, 2, 15, 8, 3, chair_d);
    fill_rect(&mut img, 4, 25, 4, 2, metal);
    px(&mut img, 2, 27, metal);
    px(&mut img, 9, 27, metal);

    // Worker body (side-ish 3/4 view facing monitor)
    filled_body(&mut img, 3, 12, 8, 9, pal.shirt);
    // collar / badge
    fill_rect(&mut img, 5, 12, 4, 1, [pal.shirt[0].saturating_add(20), pal.shirt[1].saturating_add(20), pal.shirt[2].saturating_add(20), 255]);
    px(&mut img, 6, 14, pal.accent);
    // head
    filled_body(&mut img, 4, 4, 7, 8, pal.skin);
    // hair with fringe
    fill_rect(&mut img, 4, 3, 7, 3, pal.hair);
    px(&mut img, 3, 5, pal.hair);
    px(&mut img, 11, 5, pal.hair);
    // ears
    px(&mut img, 3, 7, pal.skin);
    px(&mut img, 11, 7, pal.skin);
    // eyes + highlight
    px(&mut img, 5, 7, OUTLINE);
    px(&mut img, 9, 7, OUTLINE);
    px(&mut img, 6, 7, [255, 255, 255, 255]);
    // brow
    px(&mut img, 5, 6, pal.hair);
    px(&mut img, 9, 6, pal.hair);
    // mouth
    if typing {
        px(&mut img, 7, 10, [160, 70, 70, 255]);
    } else {
        px(&mut img, 6, 10, [180, 90, 90, 255]);
        px(&mut img, 7, 10, [180, 90, 90, 255]);
        px(&mut img, 8, 10, [180, 90, 90, 255]);
    }
    // arms to keyboard
    if typing {
        let y = 16 + (frame % 2) as i32;
        // No outline at this thickness: `outline_rect` paints the top *and*
        // bottom row, which on a 2px-tall rect is every pixel — the arm came
        // out as a solid black bar with no skin left (RC16 B6). The idle arm
        // below is likewise outline-free.
        fill_rect(&mut img, 11, y, 6, 2, pal.skin);
        // sleeve
        fill_rect(&mut img, 10, y, 2, 2, pal.shirt);
    } else {
        fill_rect(&mut img, 11, 17, 5, 2, pal.skin);
        fill_rect(&mut img, 10, 17, 2, 2, pal.shirt);
    }
    // legs under desk
    fill_rect(&mut img, 4, 21, 2, 6, pal.pants);
    fill_rect(&mut img, 8, 21, 2, 6, pal.pants);
    fill_rect(&mut img, 3, 26, 3, 2, OUTLINE);
    fill_rect(&mut img, 8, 26, 3, 2, OUTLINE);

    // thinking bubble when not typing
    if !typing && frame % 4 < 2 {
        px(&mut img, 12, 2, [230, 230, 240, 255]);
        px(&mut img, 13, 1, [230, 230, 240, 255]);
        filled_body(&mut img, 14, 0, 5, 3, [240, 240, 248, 255]);
        px(&mut img, 15, 1, OUTLINE);
        px(&mut img, 17, 1, OUTLINE);
    }
    img
}

/// Canonical `frame` for [`sprite_developer_walk`] (RC16 P8) — the limb swap is
/// the only frame-dependent art, so the period is **2**.
pub fn walk_frame_key(frame: u8) -> u8 {
    frame % 2
}

/// Developer walking (optionally carrying a packet).
pub fn sprite_developer_walk(pal: DevPalette, frame: u8, with_packet: bool) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(16, 24, Rgba(CLEAR));
    // head
    filled_body(&mut img, 5, 1, 6, 6, pal.skin);
    fill_rect(&mut img, 5, 0, 6, 2, pal.hair);
    px(&mut img, 4, 2, pal.hair);
    px(&mut img, 6, 3, OUTLINE);
    px(&mut img, 9, 3, OUTLINE);
    // torso
    filled_body(&mut img, 4, 7, 8, 8, pal.shirt);
    px(&mut img, 6, 9, pal.accent);
    // limbs
    if frame % 2 == 0 {
        fill_rect(&mut img, 2, 8, 2, 5, pal.skin);
        fill_rect(&mut img, 12, 9, 2, 5, pal.skin);
        fill_rect(&mut img, 4, 15, 3, 6, pal.pants);
        fill_rect(&mut img, 9, 16, 3, 6, pal.pants);
        fill_rect(&mut img, 3, 21, 4, 2, OUTLINE);
        fill_rect(&mut img, 9, 22, 4, 2, OUTLINE);
    } else {
        fill_rect(&mut img, 2, 9, 2, 5, pal.skin);
        fill_rect(&mut img, 12, 8, 2, 5, pal.skin);
        fill_rect(&mut img, 4, 16, 3, 6, pal.pants);
        fill_rect(&mut img, 9, 15, 3, 6, pal.pants);
        fill_rect(&mut img, 3, 22, 4, 2, OUTLINE);
        fill_rect(&mut img, 9, 21, 4, 2, OUTLINE);
    }
    if with_packet {
        let packet = sprite_packet();
        blit_local(&mut img, &packet, 10, 10);
    }
    img
}

/// Canonical `frame` for [`sprite_supervisor`] (RC16 P8), per phase:
/// - **1** (working) scrolls code at `% 3` and bobs the hands at `% 2` — period 6;
/// - **2** (reviewing) only scrolls code — period 3;
/// - anything else (idle/waiting) only alternates the coffee steam — period 2.
pub fn supervisor_frame_key(phase: u8, frame: u8) -> u8 {
    match phase {
        1 => frame % 6,
        2 => frame % 3,
        _ => frame % 2,
    }
}

/// Supervisor: phase 0 idle, 1 working, 2 reviewing — boss gold accents + horns.
pub fn sprite_supervisor(phase: u8, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(34, 30, Rgba(CLEAR));
    let gold = [255, 208, 72, 255];
    let gold_d = [200, 150, 40, 255];
    let gold_h = [255, 236, 140, 255];
    let skin = [255, 220, 168, 255];
    let shirt = [64, 48, 40, 255];
    let wood = [176, 120, 64, 255];
    let wood_d = [120, 76, 40, 255];
    let wood_h = [200, 148, 88, 255];

    // Wide executive desk
    filled_body(&mut img, 1, 18, 32, 7, wood);
    fill_rect(&mut img, 3, 19, 28, 1, wood_h);
    fill_rect(&mut img, 4, 25, 3, 3, wood_d);
    fill_rect(&mut img, 27, 25, 3, 3, wood_d);
    // drawers
    filled_body(&mut img, 3, 21, 6, 4, wood_d);
    filled_body(&mut img, 25, 21, 6, 4, wood_d);
    px(&mut img, 8, 22, gold);
    px(&mut img, 30, 22, gold);

    // Chair
    filled_body(&mut img, 12, 12, 10, 8, [88, 56, 36, 255]);
    fill_rect(&mut img, 13, 13, 8, 2, [112, 72, 48, 255]);

    // Body + gold vest
    filled_body(&mut img, 12, 12, 10, 9, shirt);
    fill_rect(&mut img, 14, 13, 6, 5, gold);
    fill_rect(&mut img, 15, 14, 4, 3, gold_h);
    // tie
    fill_rect(&mut img, 16, 15, 2, 4, [180, 48, 48, 255]);

    // Head + horns
    filled_body(&mut img, 12, 3, 10, 9, skin);
    // horns
    fill_rect(&mut img, 10, 0, 3, 5, gold);
    fill_rect(&mut img, 21, 0, 3, 5, gold);
    px(&mut img, 10, 0, gold_d);
    px(&mut img, 23, 0, gold_d);
    px(&mut img, 11, 1, gold_h);
    px(&mut img, 22, 1, gold_h);
    // hair fringe
    fill_rect(&mut img, 13, 2, 8, 2, [72, 48, 32, 255]);
    // face
    px(&mut img, 14, 6, OUTLINE);
    px(&mut img, 19, 6, OUTLINE);
    px(&mut img, 15, 6, [255, 255, 255, 255]);
    fill_rect(&mut img, 15, 9, 4, 1, [200, 80, 80, 255]);
    // smile curve
    px(&mut img, 14, 9, [200, 80, 80, 255]);
    px(&mut img, 19, 9, [200, 80, 80, 255]);

    match phase {
        1 | 2 => {
            // Open laptop with code
            fill_rect(&mut img, 4, 17, 10, 2, [48, 52, 64, 255]);
            filled_body(&mut img, 5, 7, 10, 10, [36, 40, 52, 255]);
            fill_rect(&mut img, 6, 8, 8, 7, [12, 20, 28, 255]);
            let f = frame % 3;
            for row in 0..4 {
                let y = 9 + row * 1;
                let len = 2 + ((row + f as i32) % 4);
                fill_rect(
                    &mut img,
                    7,
                    y,
                    len.min(6),
                    1,
                    if row % 2 == 0 {
                        [64, 232, 120, 255]
                    } else {
                        [96, 180, 255, 255]
                    },
                );
            }
            if phase == 1 {
                let y = 17 + (frame % 2) as i32;
                fill_rect(&mut img, 7, y, 2, 2, skin);
                fill_rect(&mut img, 12, y, 2, 2, skin);
            } else {
                // Reviewing docs
                filled_body(&mut img, 22, 13, 7, 5, [248, 248, 240, 255]);
                fill_rect(&mut img, 23, 14, 5, 1, [100, 140, 220, 255]);
                fill_rect(&mut img, 23, 16, 4, 1, [160, 160, 180, 255]);
                fill_rect(&mut img, 24, 14, 2, 2, skin); // hand on paper
            }
        }
        _ => {
            // Closed laptop + coffee + plant
            filled_body(&mut img, 5, 16, 10, 3, [48, 52, 64, 255]);
            filled_body(&mut img, 24, 13, 5, 6, [220, 220, 228, 255]);
            fill_rect(&mut img, 25, 14, 3, 3, [100, 56, 32, 255]);
            px(&mut img, 29, 15, [200, 200, 210, 255]);
            fill_rect(&mut img, 21, 14, 2, 2, skin);
            // steam
            if frame % 2 == 0 {
                px(&mut img, 25, 11, [230, 230, 240, 200]);
            } else {
                px(&mut img, 27, 10, [230, 230, 240, 180]);
            }
            // tiny desk plant
            fill_rect(&mut img, 30, 15, 2, 2, [64, 160, 80, 255]);
            fill_rect(&mut img, 30, 17, 2, 2, [160, 100, 60, 255]);
        }
    }
    img
}

/// MCP server rack. When `active`, LEDs blink and status bar pulses.
///
/// Kept for unit tests / future ambient rack props (not composed in RC13 office).
#[cfg(test)]
pub fn sprite_mcp_server(active: bool, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(18, 28, Rgba(CLEAR));
    let chassis = [36, 40, 52, 255];
    let chassis_d = [24, 28, 36, 255];
    let chassis_h = [56, 64, 80, 255];
    let bezel = [20, 24, 32, 255];

    // Outer rack
    filled_body(&mut img, 1, 0, 16, 27, chassis);
    fill_rect(&mut img, 2, 1, 14, 1, chassis_h);
    fill_rect(&mut img, 2, 25, 14, 1, chassis_d);
    // Feet
    fill_rect(&mut img, 2, 27, 3, 1, chassis_d);
    fill_rect(&mut img, 13, 27, 3, 1, chassis_d);

    // Top badge strip
    fill_rect(&mut img, 3, 2, 12, 3, bezel);
    // "MCP" pixel marks
    let badge = if active && frame % 2 == 0 {
        [120, 255, 180, 255]
    } else {
        [80, 200, 140, 255]
    };
    for (x, y) in [(4, 3), (5, 3), (7, 3), (8, 3), (10, 3), (11, 3), (12, 3)] {
        px(&mut img, x, y, badge);
    }

    // Drive bays / blade rows
    for row in 0..6 {
        let y = 6 + row * 3;
        filled_body(&mut img, 3, y, 12, 3, bezel);
        // left handle
        fill_rect(&mut img, 4, y + 1, 2, 1, chassis_h);
        // LED cluster
        let on = if active {
            // chase pattern when MCP is mid-call
            (frame as usize + row as usize) % 3 == 0 || (frame % 2 == 0 && row % 2 == 0)
        } else {
            row == 0 || row == 3 // idle: a couple of steady greens
        };
        let led = if on {
            if active && frame % 2 == 0 {
                [80, 255, 140, 255]
            } else {
                [48, 200, 100, 255]
            }
        } else if active {
            [200, 80, 60, 255] // amber/red activity blips
        } else {
            [48, 56, 64, 255]
        };
        px(&mut img, 12, y + 1, led);
        if active && frame % 2 == 0 {
            px(&mut img, 13, y + 1, [120, 220, 255, 255]);
        } else {
            px(&mut img, 13, y + 1, [40, 80, 120, 255]);
        }
    }

    // Bottom activity bar
    fill_rect(&mut img, 3, 24, 12, 1, bezel);
    if active {
        let pulse = (frame % 4) as i32;
        fill_rect(&mut img, 4 + pulse, 24, 4, 1, [80, 255, 160, 255]);
        fill_rect(&mut img, 8 + pulse, 24, 3, 1, [80, 180, 255, 255]);
    } else {
        fill_rect(&mut img, 4, 24, 2, 1, [48, 120, 80, 255]);
    }
    img
}

pub fn sprite_packet() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(8, 8, Rgba(CLEAR));
    filled_body(&mut img, 0, 0, 8, 8, [255, 244, 200, 255]);
    fill_rect(&mut img, 2, 2, 4, 1, [80, 140, 220, 255]);
    fill_rect(&mut img, 2, 4, 4, 1, [80, 140, 220, 255]);
    img
}

/// Tiny potted plant (bookshelf / corner ambient prop).
pub fn sprite_plant() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(10, 12, Rgba(CLEAR));
    let pot = [176, 96, 64, 255];
    let leaf = [64, 176, 88, 255];
    let leaf_d = [40, 120, 56, 255];
    filled_body(&mut img, 2, 7, 6, 5, pot);
    fill_rect(&mut img, 3, 3, 4, 5, leaf);
    px(&mut img, 2, 4, leaf_d);
    px(&mut img, 7, 5, leaf);
    px(&mut img, 4, 1, leaf);
    px(&mut img, 5, 2, leaf_d);
    img
}

/// Coffee mug ambient prop (side table / shelf).
pub fn sprite_coffee() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(8, 8, Rgba(CLEAR));
    let mug = [220, 220, 228, 255];
    let coffee = [100, 56, 32, 255];
    filled_body(&mut img, 1, 2, 5, 5, mug);
    fill_rect(&mut img, 2, 3, 3, 2, coffee);
    // handle
    px(&mut img, 6, 3, mug);
    px(&mut img, 7, 4, mug);
    px(&mut img, 6, 5, mug);
    // steam
    px(&mut img, 2, 0, [230, 230, 240, 180]);
    px(&mut img, 4, 1, [230, 230, 240, 160]);
    img
}

fn blit_local(dest: &mut RgbaImage, sprite: &RgbaImage, dx: i32, dy: i32) {
    blit(dest, sprite, dx, dy);
}

pub fn scale_nn(src: &RgbaImage, factor: u32) -> RgbaImage {
    let factor = factor.max(1);
    let (w, h) = src.dimensions();
    let mut out = RgbaImage::new(w * factor, h * factor);
    for y in 0..h {
        for x in 0..w {
            let p = *src.get_pixel(x, y);
            for dy in 0..factor {
                for dx in 0..factor {
                    out.put_pixel(x * factor + dx, y * factor + dy, p);
                }
            }
        }
    }
    out
}

pub fn blit(dest: &mut RgbaImage, sprite: &RgbaImage, dx: i32, dy: i32) {
    let (dw, dh) = dest.dimensions();
    let (sw, sh) = sprite.dimensions();
    for sy in 0..sh {
        for sx in 0..sw {
            let p = sprite.get_pixel(sx, sy);
            if p.0[3] == 0 {
                continue;
            }
            let x = dx + sx as i32;
            let y = dy + sy as i32;
            if x < 0 || y < 0 || x as u32 >= dw || y as u32 >= dh {
                continue;
            }
            if p.0[3] >= 250 {
                dest.put_pixel(x as u32, y as u32, *p);
            } else {
                let dst = dest.get_pixel(x as u32, y as u32).0;
                let a = u32::from(p.0[3]);
                let inv = 255 - a;
                let r = (u32::from(p.0[0]) * a + u32::from(dst[0]) * inv) / 255;
                let g = (u32::from(p.0[1]) * a + u32::from(dst[1]) * inv) / 255;
                let b = (u32::from(p.0[2]) * a + u32::from(dst[2]) * inv) / 255;
                let aa = a.max(u32::from(dst[3])).min(255);
                dest.put_pixel(
                    x as u32,
                    y as u32,
                    Rgba([r as u8, g as u8, b as u8, aa as u8]),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprites_non_empty() {
        assert!(!sprite_empty_desk().as_raw().is_empty());
        assert!(!sprite_developer_at_desk(DevPalette::by_index(0), true, 1)
            .as_raw()
            .is_empty());
        let mon = sprite_square_monitor(true, 0);
        assert_eq!(mon.width(), mon.height());
        // Detailed desk sprites — keep within composable bounds
        assert!(sprite_empty_desk().width() <= 40);
        assert!(sprite_developer_at_desk(DevPalette::by_index(0), false, 0).width() <= 40);
        assert_eq!(sprite_mcp_server(true, 1).width(), 18);
        assert!(!sprite_mcp_server(false, 0).as_raw().is_empty());
        assert!(!sprite_supervisor(1, 2).as_raw().is_empty());
    }

    #[test]
    fn floor_not_diagonal_green_checker() {
        let mut img = RgbaImage::new(32, 32);
        floor_stamp::stamp_floor_patch(&mut img, 0, 0, 32, 32);
        // Should use SNES tile colors, not pure diagonal stripes of 2 fixed colors only
        let mut colors = std::collections::HashSet::new();
        for p in img.pixels() {
            colors.insert(p.0);
        }
        assert!(colors.len() >= 3, "floor should have tile bevel variation");
    }

    #[test]
    fn stamp_floor_patch_sampled_uses_bg_sample() {
        let mut bg = RgbaImage::from_pixel(64, 64, Rgba(floor_stamp::FLOOR_A));
        // Distinct floor sample region used by stamp_floor_patch_sampled (lower-left).
        for y in 50..64 {
            for x in 0..16 {
                bg.put_pixel(x, y, Rgba([40, 120, 130, 255]));
            }
        }
        let mut dest = RgbaImage::from_pixel(64, 64, Rgba(CLEAR));
        floor_stamp::stamp_floor_patch_sampled(&mut dest, Some(&bg), 8, 8, 16, 16);
        let p = dest.get_pixel(12, 12).0;
        assert_ne!(p[3], 0, "sampled stamp should write opaque pixels");
        // Should be teal-family (mixed with sample), not pure magenta garbage.
        assert!(p[1] > p[0], "green channel should dominate for teal floor");
    }

    /// B6: the typing arm is a 2px-tall rect, so an outline pass covers the
    /// top *and* bottom row — i.e. every pixel — and the arm rendered as a
    /// solid black bar. It must keep visible skin on both typing frames.
    #[test]
    fn typing_arm_keeps_visible_skin() {
        let pal = DevPalette::by_index(0);
        for frame in 0..2u8 {
            let img = sprite_developer_at_desk(pal, true, frame);
            // Arm rect minus the sleeve that covers its first two columns.
            let y = 16 + u32::from(frame % 2);
            let skin = (12..17)
                .flat_map(|x| [(x, y), (x, y + 1)])
                .filter(|(x, y)| img.get_pixel(*x, *y).0 == pal.skin)
                .count();
            assert!(skin > 0, "frame {frame}: typing arm has no skin left");
        }
    }

    /// RC16 P8: the compose sprite cache keys on the declared canonical frame,
    /// so a key collision MUST mean pixel-identical art. Pin every declared
    /// period against the real sprite bodies so they cannot drift apart.
    #[test]
    fn frame_keys_only_collide_on_identical_art() {
        let pal = DevPalette::by_index(3);
        for frame in 0..24u8 {
            for typing in [true, false] {
                let key = dev_at_desk_frame_key(typing, frame);
                assert_eq!(
                    sprite_developer_at_desk(pal, typing, frame).into_raw(),
                    sprite_developer_at_desk(pal, typing, key).into_raw(),
                    "dev_at_desk typing={typing} frame={frame} != key={key}"
                );
            }
            for with_packet in [true, false] {
                let key = walk_frame_key(frame);
                assert_eq!(
                    sprite_developer_walk(pal, frame, with_packet).into_raw(),
                    sprite_developer_walk(pal, key, with_packet).into_raw(),
                    "walk packet={with_packet} frame={frame} != key={key}"
                );
            }
            for phase in 0..4u8 {
                let key = supervisor_frame_key(phase, frame);
                assert_eq!(
                    sprite_supervisor(phase, frame).into_raw(),
                    sprite_supervisor(phase, key).into_raw(),
                    "supervisor phase={phase} frame={frame} != key={key}"
                );
            }
        }
    }

    /// The other half of the contract: a period that is too *coarse* would
    /// silently freeze an animation. Every declared key value must render art
    /// that differs from at least one sibling key.
    #[test]
    fn frame_keys_are_not_coarser_than_the_animation() {
        let pal = DevPalette::by_index(0);
        let distinct = |imgs: Vec<Vec<u8>>| -> usize {
            imgs.into_iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        };
        assert_eq!(
            distinct(
                (0..4u8)
                    .map(|f| sprite_developer_at_desk(pal, true, f).into_raw())
                    .collect()
            ),
            4,
            "typing dev must have 4 distinct frames"
        );
        assert_eq!(
            distinct(
                [0u8, 2]
                    .iter()
                    .map(|f| sprite_developer_at_desk(pal, false, *f).into_raw())
                    .collect()
            ),
            2,
            "idle dev must have 2 distinct poses"
        );
        assert_eq!(
            distinct(
                (0..2u8)
                    .map(|f| sprite_developer_walk(pal, f, false).into_raw())
                    .collect()
            ),
            2,
            "walk must have 2 distinct frames"
        );
        for (phase, period) in [(0u8, 2u8), (1, 6), (2, 3)] {
            assert_eq!(
                distinct(
                    (0..period)
                        .map(|f| sprite_supervisor(phase, f).into_raw())
                        .collect()
                ),
                usize::from(period),
                "supervisor phase {phase} must have {period} distinct frames"
            );
        }
    }

    #[test]
    fn blit_and_scale() {
        let s = sprite_packet();
        let big = scale_nn(&s, 2);
        assert_eq!(big.width(), s.width() * 2);
        // Packet is also used by walking handoff sprites.
        let walk = sprite_developer_walk(DevPalette::by_index(0), 0, true);
        assert!(!walk.as_raw().is_empty());
    }
}
