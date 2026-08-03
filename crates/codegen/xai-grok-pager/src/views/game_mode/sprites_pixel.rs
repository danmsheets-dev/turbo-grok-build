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
/// Override with env `GROK_GAME_PIXEL_SCALE` = 2|3|4 (default 3).
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

/// Default scale constant (for docs/tests). Prefer [`pixel_scale()`] at runtime.
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

/// SNES office carpet — warm teal tiles (matches mockup floor).
pub const FLOOR_A: [u8; 4] = [42, 108, 112, 255];
pub const FLOOR_B: [u8; 4] = [36, 96, 100, 255];
pub const FLOOR_HI: [u8; 4] = [56, 128, 132, 255];
pub const FLOOR_LO: [u8; 4] = [28, 80, 84, 255];
/// Legacy alias used by older call sites.
pub const FLOOR_TEAL: [u8; 4] = FLOOR_A;

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

/// SNES carpet tile (8×8) — diamond-ish checker with edge bevel.
pub fn floor_tile_at(wx: i32, wy: i32) -> [u8; 4] {
    let tx = wx.rem_euclid(8);
    let ty = wy.rem_euclid(8);
    // Tile index for soft checker of 8×8 cells
    let tile = ((wx.div_euclid(8)) + (wy.div_euclid(8))).rem_euclid(2);
    let base = if tile == 0 { FLOOR_A } else { FLOOR_B };
    // Bevel: light NW, dark SE
    if tx == 0 || ty == 0 {
        return FLOOR_HI;
    }
    if tx == 7 || ty == 7 {
        return FLOOR_LO;
    }
    // Subtle inner dither for 16-bit texture
    if (tx + ty) % 5 == 0 {
        FLOOR_LO
    } else if (tx * 3 + ty) % 7 == 0 {
        FLOOR_HI
    } else {
        base
    }
}

/// Opaque SNES floor patch (replaces the old diagonal green checker).
/// Prefer [`stamp_floor_patch_sampled`] when a background is available.
#[allow(dead_code)]
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

/// Sample-aware floor stamp: prefer real background floor colors when available,
/// fall back to procedural SNES tiles.
pub fn stamp_floor_patch_sampled(
    dest: &mut RgbaImage,
    bg: Option<&RgbaImage>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let (dw, dh) = dest.dimensions();
    // Prefer sampling a known-clean floor region (bottom-left of office).
    let sample = bg.and_then(|img| {
        let (bw, bh) = img.dimensions();
        if bw < 8 || bh < 8 {
            return None;
        }
        // Average a few floor pixels from lower-left (usually empty carpet).
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
                // Procedural variation around sampled floor color
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

/// Empty desk + chair + dark square monitor (compact SNES size).
pub fn sprite_empty_desk() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(28, 24, Rgba(CLEAR));
    let wood = [176, 120, 64, 255];
    let wood_d = [120, 76, 40, 255];
    let chair = [64, 72, 96, 255];
    // desk top with outline
    filled_body(&mut img, 6, 12, 20, 7, wood);
    fill_rect(&mut img, 8, 19, 2, 4, wood_d);
    fill_rect(&mut img, 22, 19, 2, 4, wood_d);
    // wood grain ticks
    for gx in [10, 14, 18] {
        px(&mut img, gx, 14, wood_d);
        px(&mut img, gx + 1, 16, wood_d);
    }
    let mon = sprite_square_monitor(false, 0);
    blit_local(&mut img, &mon, 12, 0);
    // chair
    filled_body(&mut img, 0, 13, 7, 7, chair);
    fill_rect(&mut img, 1, 20, 4, 3, chair);
    img
}

/// Developer at desk: typing=true animates arms; smaller SNES proportions.
pub fn sprite_developer_at_desk(pal: DevPalette, typing: bool, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(28, 26, Rgba(CLEAR));
    let wood = [176, 120, 64, 255];
    let wood_d = [120, 76, 40, 255];
    let chair = [64, 72, 96, 255];
    filled_body(&mut img, 8, 13, 18, 7, wood);
    fill_rect(&mut img, 10, 20, 2, 4, wood_d);
    fill_rect(&mut img, 22, 20, 2, 4, wood_d);
    let mon = sprite_square_monitor(true, if typing { frame } else { 0 });
    blit_local(&mut img, &mon, 14, 0);
    // chair
    filled_body(&mut img, 0, 13, 8, 7, chair);
    // body
    filled_body(&mut img, 1, 11, 7, 8, pal.shirt);
    // head
    filled_body(&mut img, 2, 4, 6, 7, pal.skin);
    fill_rect(&mut img, 2, 3, 6, 2, pal.hair);
    // eyes
    px(&mut img, 3, 6, OUTLINE);
    px(&mut img, 6, 6, OUTLINE);
    // smile
    px(&mut img, 4, 8, [180, 80, 80, 255]);
    px(&mut img, 5, 8, [180, 80, 80, 255]);
    // arms
    if typing {
        let y = 14 + (frame % 2) as i32;
        fill_rect(&mut img, 8, y, 5, 2, pal.skin);
        outline_rect(&mut img, 8, y, 5, 2, OUTLINE);
    } else {
        fill_rect(&mut img, 8, 15, 4, 2, pal.skin);
    }
    // legs
    fill_rect(&mut img, 2, 19, 2, 5, pal.pants);
    fill_rect(&mut img, 5, 19, 2, 5, pal.pants);
    // shoe
    fill_rect(&mut img, 1, 23, 3, 2, OUTLINE);
    fill_rect(&mut img, 5, 23, 3, 2, OUTLINE);
    // badge accent
    px(&mut img, 3, 13, pal.accent);
    img
}

/// Developer walking (optionally carrying a packet).
pub fn sprite_developer_walk(pal: DevPalette, frame: u8, with_packet: bool) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(14, 20, Rgba(CLEAR));
    filled_body(&mut img, 4, 1, 6, 6, pal.skin);
    fill_rect(&mut img, 4, 0, 6, 2, pal.hair);
    filled_body(&mut img, 4, 7, 6, 7, pal.shirt);
    px(&mut img, 5, 3, OUTLINE);
    px(&mut img, 8, 3, OUTLINE);
    if frame % 2 == 0 {
        fill_rect(&mut img, 2, 8, 2, 4, pal.skin);
        fill_rect(&mut img, 10, 9, 2, 4, pal.skin);
        fill_rect(&mut img, 4, 14, 2, 5, pal.pants);
        fill_rect(&mut img, 8, 15, 2, 5, pal.pants);
    } else {
        fill_rect(&mut img, 2, 9, 2, 4, pal.skin);
        fill_rect(&mut img, 10, 8, 2, 4, pal.skin);
        fill_rect(&mut img, 4, 15, 2, 5, pal.pants);
        fill_rect(&mut img, 8, 14, 2, 5, pal.pants);
    }
    if with_packet {
        filled_body(&mut img, 10, 10, 4, 4, [255, 236, 160, 255]);
        fill_rect(&mut img, 11, 11, 2, 1, [80, 140, 220, 255]);
    }
    img
}

/// Supervisor: phase 0 idle, 1 working, 2 reviewing — boss gold accents.
pub fn sprite_supervisor(phase: u8, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(26, 24, Rgba(CLEAR));
    let gold = [255, 208, 72, 255];
    let gold_d = [200, 150, 40, 255];
    let skin = [255, 220, 168, 255];
    let shirt = [72, 56, 48, 255];
    let wood = [176, 120, 64, 255];
    filled_body(&mut img, 1, 15, 24, 6, wood);
    filled_body(&mut img, 9, 11, 8, 5, [96, 64, 40, 255]); // chair
    filled_body(&mut img, 9, 10, 8, 7, shirt);
    // gold vest stripe
    fill_rect(&mut img, 10, 11, 6, 3, gold);
    // head + horns
    filled_body(&mut img, 9, 3, 8, 7, skin);
    fill_rect(&mut img, 8, 0, 2, 4, gold);
    fill_rect(&mut img, 16, 0, 2, 4, gold);
    px(&mut img, 8, 0, gold_d);
    px(&mut img, 17, 0, gold_d);
    px(&mut img, 11, 5, OUTLINE);
    px(&mut img, 14, 5, OUTLINE);
    fill_rect(&mut img, 11, 7, 4, 1, [200, 80, 80, 255]);

    match phase {
        1 | 2 => {
            // Open laptop
            fill_rect(&mut img, 4, 14, 8, 2, [48, 52, 64, 255]);
            filled_body(&mut img, 5, 6, 8, 8, [36, 40, 52, 255]);
            fill_rect(&mut img, 6, 7, 6, 6, [16, 24, 32, 255]);
            let f = frame % 3;
            for row in 0..3 {
                let y = 8 + row * 2;
                let len = 2 + ((row + f as i32) % 3);
                fill_rect(
                    &mut img,
                    7,
                    y,
                    len,
                    1,
                    if row % 2 == 0 {
                        [64, 232, 120, 255]
                    } else {
                        [96, 180, 255, 255]
                    },
                );
            }
            if phase == 1 {
                let y = 14 + (frame % 2) as i32;
                fill_rect(&mut img, 6, y, 2, 2, skin);
                fill_rect(&mut img, 11, y, 2, 2, skin);
            } else {
                filled_body(&mut img, 17, 11, 5, 4, [248, 248, 240, 255]);
                fill_rect(&mut img, 18, 12, 3, 1, [100, 140, 220, 255]);
            }
        }
        _ => {
            // Closed laptop + coffee
            filled_body(&mut img, 5, 13, 8, 3, [48, 52, 64, 255]);
            filled_body(&mut img, 18, 11, 4, 5, [220, 220, 228, 255]);
            fill_rect(&mut img, 19, 12, 2, 2, [100, 56, 32, 255]);
            px(&mut img, 22, 13, [200, 200, 210, 255]);
            fill_rect(&mut img, 16, 12, 2, 2, skin);
            if frame % 2 == 0 {
                px(&mut img, 19, 9, [230, 230, 240, 200]);
            } else {
                px(&mut img, 20, 9, [230, 230, 240, 180]);
            }
        }
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
        // Smaller than old 36×32 desk sprites
        assert!(sprite_empty_desk().width() <= 30);
        assert!(sprite_developer_at_desk(DevPalette::by_index(0), false, 0).width() <= 30);
    }

    #[test]
    fn floor_not_diagonal_green_checker() {
        let mut img = RgbaImage::new(32, 32);
        stamp_floor_patch(&mut img, 0, 0, 32, 32);
        // Should use SNES tile colors, not pure diagonal stripes of 2 fixed colors only
        let mut colors = std::collections::HashSet::new();
        for p in img.pixels() {
            colors.insert(p.0);
        }
        assert!(colors.len() >= 3, "floor should have tile bevel variation");
    }

    #[test]
    fn blit_and_scale() {
        let s = sprite_packet();
        let big = scale_nn(&s, 2);
        assert_eq!(big.width(), s.width() * 2);
    }
}
