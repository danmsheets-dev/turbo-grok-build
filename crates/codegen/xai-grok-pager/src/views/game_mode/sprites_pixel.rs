//! Procedural 8-bit sprites for Game Mode (pure Rust).

use image::{Rgba, RgbaImage};

/// Developer palette: shirt + skin + hair accents.
#[derive(Debug, Clone, Copy)]
pub struct DevPalette {
    pub shirt: [u8; 4],
    pub skin: [u8; 4],
    pub hair: [u8; 4],
    pub pants: [u8; 4],
}

impl DevPalette {
    pub fn by_index(i: u8) -> Self {
        match i % 6 {
            0 => Self {
                shirt: [80, 200, 200, 255],
                skin: [220, 70, 70, 255],
                hair: [180, 40, 40, 255],
                pants: [50, 50, 70, 255],
            },
            1 => Self {
                shirt: [60, 180, 90, 255],
                skin: [90, 150, 220, 255],
                hair: [200, 140, 60, 255],
                pants: [50, 60, 120, 255],
            },
            2 => Self {
                shirt: [230, 200, 50, 255],
                skin: [160, 100, 180, 255],
                hair: [80, 50, 100, 255],
                pants: [50, 50, 70, 255],
            },
            3 => Self {
                shirt: [230, 120, 50, 255],
                skin: [100, 180, 90, 255],
                hair: [40, 100, 50, 255],
                pants: [40, 50, 90, 255],
            },
            4 => Self {
                shirt: [100, 80, 180, 255],
                skin: [120, 140, 200, 255],
                hair: [40, 40, 60, 255],
                pants: [90, 70, 40, 255],
            },
            _ => Self {
                shirt: [70, 180, 200, 255],
                skin: [230, 170, 110, 255],
                hair: [200, 90, 40, 255],
                pants: [40, 50, 90, 255],
            },
        }
    }
}

const CLEAR: [u8; 4] = [0, 0, 0, 0];
/// Floor teal matching the office mockup (not pure black).
pub const FLOOR_TEAL: [u8; 4] = [36, 92, 98, 255];
const FLOOR_TEAL_D: [u8; 4] = [28, 78, 84, 255];

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

/// Opaque floor patch (checker) to clear baked-in mockup characters.
pub fn stamp_floor_patch(dest: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32) {
    let (dw, dh) = dest.dimensions();
    for dy in 0..h {
        for dx in 0..w {
            let px_ = x + dx;
            let py = y + dy;
            if px_ < 0 || py < 0 || px_ as u32 >= dw || py as u32 >= dh {
                continue;
            }
            let c = if ((px_ + py) / 3) % 2 == 0 {
                FLOOR_TEAL
            } else {
                FLOOR_TEAL_D
            };
            dest.put_pixel(px_ as u32, py as u32, Rgba(c));
        }
    }
}

/// Square monitor bezel. `active` scrolls code when true; dark when false.
pub fn sprite_square_monitor(active: bool, frame: u8) -> RgbaImage {
    // Square: 14×14
    let mut img = RgbaImage::from_pixel(14, 14, Rgba(CLEAR));
    let bezel = [28, 30, 36, 255];
    let edge = [12, 12, 16, 255];
    fill_rect(&mut img, 0, 0, 14, 14, bezel);
    outline_rect(&mut img, 0, 0, 14, 14, edge);
    // stand
    fill_rect(&mut img, 5, 13, 4, 1, edge);
    if !active {
        fill_rect(&mut img, 2, 2, 10, 10, [18, 22, 28, 255]);
        // dim reflection
        fill_rect(&mut img, 3, 3, 3, 2, [30, 36, 44, 255]);
        return img;
    }
    // active screen
    fill_rect(&mut img, 2, 2, 10, 10, [12, 18, 22, 255]);
    let greens = [
        [40, 220, 100, 255],
        [60, 200, 80, 255],
        [30, 180, 90, 255],
        [80, 240, 120, 255],
    ];
    let reds = [[200, 60, 60, 255], [180, 40, 40, 255]];
    let scroll = (frame % 4) as i32;
    for row in 0..5 {
        let y = 3 + row * 2;
        let len = 3 + ((row + scroll) % 5);
        let col = if row % 3 == 0 {
            reds[(row as usize + frame as usize) % 2]
        } else {
            greens[(row as usize + frame as usize) % 4]
        };
        fill_rect(&mut img, 3, y, len.min(8), 1, col);
    }
    // caret blink
    if frame % 2 == 0 {
        px(&mut img, 10, 11, [220, 255, 220, 255]);
    }
    img
}

/// Empty desk + chair + dark square monitor (no developer).
pub fn sprite_empty_desk() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(36, 30, Rgba(CLEAR));
    let wood = [140, 95, 50, 255];
    let wood_d = [100, 65, 35, 255];
    let chair = [45, 45, 55, 255];
    // desk top
    fill_rect(&mut img, 8, 14, 26, 10, wood);
    outline_rect(&mut img, 8, 14, 26, 10, wood_d);
    fill_rect(&mut img, 10, 24, 3, 5, wood_d);
    fill_rect(&mut img, 28, 24, 3, 5, wood_d);
    // square monitor (off)
    let mon = sprite_square_monitor(false, 0);
    blit_local(&mut img, &mon, 16, 0);
    // empty chair
    fill_rect(&mut img, 0, 16, 8, 10, chair);
    fill_rect(&mut img, 1, 26, 5, 3, chair);
    img
}

/// Developer at desk: typing=true animates arms; monitor always square.
pub fn sprite_developer_at_desk(pal: DevPalette, typing: bool, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(36, 32, Rgba(CLEAR));
    let wood = [140, 95, 50, 255];
    let wood_d = [100, 65, 35, 255];
    let chair = [45, 45, 55, 255];
    // desk
    fill_rect(&mut img, 10, 16, 24, 10, wood);
    outline_rect(&mut img, 10, 16, 24, 10, wood_d);
    fill_rect(&mut img, 12, 26, 3, 5, wood_d);
    fill_rect(&mut img, 28, 26, 3, 5, wood_d);
    // square monitor (animated when typing / working)
    let mon = sprite_square_monitor(true, if typing { frame } else { 0 });
    // When idle (not typing), still show content but freeze scroll via frame=0
    // and slightly dim by not advancing — already handled.
    blit_local(&mut img, &mon, 18, 0);
    // chair
    fill_rect(&mut img, 0, 16, 9, 10, chair);
    // body
    fill_rect(&mut img, 2, 14, 8, 10, pal.shirt);
    // head
    fill_rect(&mut img, 3, 6, 6, 7, pal.skin);
    fill_rect(&mut img, 3, 5, 6, 2, pal.hair);
    if pal.skin[0] > 180 && pal.skin[1] < 100 {
        px(&mut img, 3, 3, pal.hair);
        px(&mut img, 4, 2, pal.hair);
        px(&mut img, 8, 3, pal.hair);
        px(&mut img, 7, 2, pal.hair);
    }
    // eyes
    px(&mut img, 4, 8, [30, 30, 30, 255]);
    px(&mut img, 7, 8, [30, 30, 30, 255]);
    // arms
    if typing {
        let y = 17 + (frame % 2) as i32;
        fill_rect(&mut img, 9, y, 6, 2, pal.skin);
        fill_rect(&mut img, 10, y + 1, 5, 1, pal.skin);
    } else {
        // resting on desk
        fill_rect(&mut img, 9, 18, 5, 2, pal.skin);
    }
    // legs
    fill_rect(&mut img, 2, 24, 3, 6, pal.pants);
    fill_rect(&mut img, 6, 24, 3, 6, pal.pants);
    img
}

/// Developer walking (optionally carrying a packet).
pub fn sprite_developer_walk(pal: DevPalette, frame: u8, with_packet: bool) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(16, 24, Rgba(CLEAR));
    fill_rect(&mut img, 5, 1, 6, 6, pal.skin);
    fill_rect(&mut img, 5, 0, 6, 2, pal.hair);
    fill_rect(&mut img, 5, 7, 6, 8, pal.shirt);
    if frame % 2 == 0 {
        fill_rect(&mut img, 3, 8, 2, 5, pal.skin);
        fill_rect(&mut img, 11, 9, 2, 5, pal.skin);
        fill_rect(&mut img, 5, 15, 3, 6, pal.pants);
        fill_rect(&mut img, 9, 16, 3, 6, pal.pants);
    } else {
        fill_rect(&mut img, 3, 9, 2, 5, pal.skin);
        fill_rect(&mut img, 11, 8, 2, 5, pal.skin);
        fill_rect(&mut img, 5, 16, 3, 6, pal.pants);
        fill_rect(&mut img, 9, 15, 3, 6, pal.pants);
    }
    if with_packet {
        fill_rect(&mut img, 11, 11, 4, 4, [240, 220, 120, 255]);
        outline_rect(&mut img, 11, 11, 4, 4, [160, 120, 40, 255]);
    }
    img
}

/// Supervisor: phase 0 idle (laptop closed + coffee), 1 working (laptop open + typing),
/// 2 reviewing (laptop open + papers).
pub fn sprite_supervisor(phase: u8, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(32, 30, Rgba(CLEAR));
    let gold = [240, 200, 70, 255];
    let skin = [240, 210, 140, 255];
    let shirt = [60, 50, 40, 255];
    let wood = [140, 95, 50, 255];
    // desk
    fill_rect(&mut img, 2, 18, 28, 8, wood);
    // chair
    fill_rect(&mut img, 11, 14, 10, 6, [80, 50, 30, 255]);
    // body
    fill_rect(&mut img, 11, 12, 10, 8, shirt);
    fill_rect(&mut img, 12, 13, 8, 4, gold);
    // head + horns
    fill_rect(&mut img, 11, 4, 10, 8, skin);
    fill_rect(&mut img, 10, 1, 2, 4, gold);
    fill_rect(&mut img, 20, 1, 2, 4, gold);
    px(&mut img, 10, 0, gold);
    px(&mut img, 21, 0, gold);
    px(&mut img, 13, 7, [40, 40, 40, 255]);
    px(&mut img, 18, 7, [40, 40, 40, 255]);
    fill_rect(&mut img, 14, 9, 4, 1, [180, 80, 80, 255]);

    match phase {
        1 | 2 => {
            // Open laptop (clamshell): base + raised screen (square-ish)
            fill_rect(&mut img, 6, 17, 10, 3, [50, 50, 58, 255]); // base
            // screen standing
            fill_rect(&mut img, 7, 8, 10, 10, [30, 32, 40, 255]);
            outline_rect(&mut img, 7, 8, 10, 10, [12, 12, 16, 255]);
            fill_rect(&mut img, 8, 9, 8, 8, [14, 20, 24, 255]);
            // animated code
            let f = frame % 3;
            for row in 0..4 {
                let y = 10 + row * 2;
                let len = 2 + ((row + f as i32) % 4);
                fill_rect(
                    &mut img,
                    9,
                    y,
                    len,
                    1,
                    if row % 2 == 0 {
                        [50, 220, 100, 255]
                    } else {
                        [80, 160, 255, 255]
                    },
                );
            }
            // typing hands
            if phase == 1 {
                let y = 17 + (frame % 2) as i32;
                fill_rect(&mut img, 8, y, 3, 2, skin);
                fill_rect(&mut img, 14, y, 3, 2, skin);
            } else {
                // reviewing: paper beside laptop
                fill_rect(&mut img, 20, 14, 6, 5, [245, 245, 240, 255]);
                outline_rect(&mut img, 20, 14, 6, 5, [180, 180, 170, 255]);
                fill_rect(&mut img, 21, 15, 4, 1, [100, 140, 200, 255]);
            }
        }
        _ => {
            // Idle: closed laptop + coffee
            fill_rect(&mut img, 8, 16, 10, 3, [50, 50, 58, 255]); // closed lid
            outline_rect(&mut img, 8, 16, 10, 3, [20, 20, 24, 255]);
            // coffee mug
            fill_rect(&mut img, 22, 14, 5, 6, [200, 200, 210, 255]);
            fill_rect(&mut img, 23, 15, 3, 3, [90, 50, 30, 255]); // coffee
            // handle
            px(&mut img, 27, 16, [180, 180, 190, 255]);
            px(&mut img, 27, 17, [180, 180, 190, 255]);
            // sip hand near mug
            fill_rect(&mut img, 20, 15, 2, 3, skin);
            // steam
            if frame % 2 == 0 {
                px(&mut img, 24, 12, [220, 220, 230, 180]);
                px(&mut img, 25, 11, [220, 220, 230, 140]);
            } else {
                px(&mut img, 25, 12, [220, 220, 230, 180]);
            }
        }
    }
    img
}

pub fn sprite_packet() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(10, 10, Rgba(CLEAR));
    fill_rect(&mut img, 1, 1, 8, 8, [250, 245, 230, 255]);
    outline_rect(&mut img, 1, 1, 8, 8, [160, 140, 100, 255]);
    fill_rect(&mut img, 3, 3, 4, 1, [80, 120, 200, 255]);
    fill_rect(&mut img, 3, 5, 4, 1, [80, 120, 200, 255]);
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
        assert!(!sprite_square_monitor(true, 2).as_raw().is_empty());
        let mon = sprite_square_monitor(true, 0);
        assert_eq!(mon.width(), mon.height());
    }

    #[test]
    fn blit_and_scale() {
        let s = sprite_packet();
        let big = scale_nn(&s, 2);
        assert_eq!(big.width(), s.width() * 2);
    }
}
