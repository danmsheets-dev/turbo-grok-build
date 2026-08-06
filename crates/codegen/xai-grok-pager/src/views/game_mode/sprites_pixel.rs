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
/// Ceramic + brew of the desk mug (RC2 §4 #7). Shared by both idle poses of
/// [`sprite_developer_at_desk`] so the mug does not change colour when it moves
/// from the desktop to the developer's face.
const MUG: [u8; 4] = [232, 236, 244, 255];
const COFFEE: [u8; 4] = [96, 56, 36, 255];

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

/// Scale a colour to `pct`% of itself, keeping alpha (shadow / highlight steps).
fn shade(c: [u8; 4], pct: u16) -> [u8; 4] {
    let m = |v: u8| (u16::from(v) * pct / 100).min(255) as u8;
    [m(c[0]), m(c[1]), m(c[2]), c[3]]
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

/// Screen-light bleed onto the monitor bezel, per active frame (RC2 §4 #4).
///
/// Indexed by `frame % 4` — the period [`sprite_square_monitor`] already
/// animates — so the glow is *baked into frames the sprite cache already
/// stores*: no new cache keys, no new fingerprint inputs. Frame 3 is the
/// "compile flash" (one bright beat per cycle), which is why the ramp is not
/// monotonic.
const MONITOR_GLOW: [[u8; 3]; 4] = [
    [8, 26, 18],
    [14, 44, 30],
    [10, 32, 22],
    [56, 120, 96],
];

/// Error-mode counterpart of [`MONITOR_GLOW`] (RC2 §4 #1).
///
/// Indexed by `frame % 2` — the period the error screen blinks at — so the red
/// spill costs no cache keys either. Deliberately red-dominant: the fail beat
/// has to read as "this desk broke" from across a downsampled office.
const MONITOR_ERROR_GLOW: [[u8; 3]; 2] = [[40, 8, 12], [96, 14, 20]];

/// What a monitor screen is showing.
///
/// `Off` is the dark empty-desk screen, `Active` scrolls code, and `Error`
/// (RC2 §4 #1) stacks red error bars for [`sprite_developer_fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorMode {
    Off,
    Active,
    Error,
}

/// Square monitor bezel, per [`MonitorMode`].
pub fn sprite_square_monitor(mode: MonitorMode, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(12, 12, Rgba(CLEAR));
    let bezel = [40, 44, 56, 255];
    let edge = OUTLINE;
    filled_body(&mut img, 0, 0, 12, 11, bezel);
    fill_rect(&mut img, 4, 11, 4, 1, edge); // stand
    // Each arm draws its screen and yields the rim spill for that mode.
    let g = match mode {
        MonitorMode::Off => {
            fill_rect(&mut img, 2, 2, 8, 7, [20, 26, 34, 255]);
            fill_rect(&mut img, 3, 3, 2, 1, [40, 48, 60, 255]);
            return img;
        }
        MonitorMode::Active => {
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
            MONITOR_GLOW[(frame % 4) as usize]
        }
        MonitorMode::Error => {
            fill_rect(&mut img, 2, 2, 8, 7, [30, 12, 16, 255]);
            let bar = [255, 72, 72, 255];
            let bar_d = [168, 40, 48, 255];
            // A stack of stack-trace bars; the top one is the blinking beat.
            for row in 0..3i32 {
                let y = 3 + row * 2;
                let len = 6 - row;
                fill_rect(
                    &mut img,
                    3,
                    y,
                    len,
                    1,
                    if row == 0 && frame % 2 == 0 { bar } else { bar_d },
                );
            }
            if frame % 2 == 0 {
                px(&mut img, 9, 3, [255, 220, 220, 255]);
            }
            MONITOR_ERROR_GLOW[(frame % 2) as usize]
        }
    };
    // Glow rim: the screen's light spilling onto the bezel, brightening and
    // dimming across the frames the caller already animates. The bezel is
    // 2px thick on every side (screen is 8×7 inside a 12×11 body), which is the
    // minimum that survives the Nearest downsample to terminal cells.
    for y in 0..11i32 {
        for x in 0..12i32 {
            if (2..10).contains(&x) && (2..9).contains(&y) {
                continue; // screen interior — this is a rim, not a wash
            }
            let p = img.get_pixel(x as u32, y as u32).0;
            px(
                &mut img,
                x,
                y,
                [
                    p[0].saturating_add(g[0]),
                    p[1].saturating_add(g[1]),
                    p[2].saturating_add(g[2]),
                    255,
                ],
            );
        }
    }
    img
}

/// Office door prop — `open` swings the leaf back and shows the dark opening.
///
/// Two states, no frame counter: `open` *is* the canonical cache key, so unlike
/// the animated character sprites this one declares no `*_frame_key` fn. Placed
/// by [`super::compose`] at the entry/exit point SpawnWalk and ExitDoor use.
pub fn sprite_door(open: bool) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(12, 16, Rgba(CLEAR));
    let jamb = [96, 64, 40, 255];
    let wood = [150, 100, 58, 255];
    let wood_h = [178, 126, 76, 255];
    let dark = [16, 14, 22, 255];
    let brass = [255, 208, 72, 255];

    filled_body(&mut img, 0, 0, 12, 16, jamb);
    if open {
        // Doorway seen through: dark opening with the leaf swung to the jamb.
        fill_rect(&mut img, 2, 2, 8, 12, dark);
        fill_rect(&mut img, 2, 2, 3, 12, wood);
        fill_rect(&mut img, 2, 2, 3, 1, wood_h);
        px(&mut img, 4, 8, brass);
    } else {
        fill_rect(&mut img, 2, 2, 8, 12, wood);
        fill_rect(&mut img, 2, 2, 8, 1, wood_h);
        // Panel seams — 2px tall so the downsample cannot swallow them.
        fill_rect(&mut img, 3, 5, 6, 2, jamb);
        fill_rect(&mut img, 3, 10, 6, 2, jamb);
        fill_rect(&mut img, 8, 7, 2, 2, brass);
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
    let mon = sprite_square_monitor(MonitorMode::Off, 0);
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

/// Canonical `frame` for [`sprite_developer_at_desk`] (RC2 P8).
///
/// Two frames with the same key render byte-identical images, so the compose
/// sprite cache keys on this instead of the raw frame and stores no duplicates.
/// Declared here so it cannot drift from the sprite body:
/// - typing reads `frame % 2` (keys, mouse, arms, slim monitor) and forwards
///   `frame` to [`sprite_square_monitor`], which reads `% 4` — period **4**;
/// - idle pins the monitor to frame 0 and only reads `frame % 4 < 2` — **2**
///   distinct poses, so odd frames collapse onto their even neighbour.
///
/// RC2 §4 #7 gave the two idle poses their content (mug on the desk + thinking
/// bubble, versus mug raised to the face for a sip) **without** widening the
/// period: the sip reads the same `frame % 4 < 2` discriminator the bubble
/// always did, so the coffee cycle costs zero additional cache keys.
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
    let mon = sprite_square_monitor(MonitorMode::Active, if typing { frame } else { 0 });
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
    let sipping = !typing && frame % 4 >= 2;
    if typing {
        let y = 16 + (frame % 2) as i32;
        // No outline at this thickness: `outline_rect` paints the top *and*
        // bottom row, which on a 2px-tall rect is every pixel — the arm came
        // out as a solid black bar with no skin left (RC2 B6). The idle arm
        // below is likewise outline-free.
        fill_rect(&mut img, 11, y, 6, 2, pal.skin);
        // sleeve
        fill_rect(&mut img, 10, y, 2, 2, pal.shirt);
    } else if sipping {
        // Coffee sip (RC2 §4 #7): the forearm folds up and the mug that sits on
        // the desk in the other idle pose is at the face instead. Drawn after
        // the head so it reads as being in front of it.
        //
        // `fill_rect`, not `filled_body`: a 1px outline around a 4px sprite is
        // seven eighths of the sprite, and the mug came out as a dark smudge
        // that read like a beard — the same trap as the typing arm (RC2 B6) and
        // the fail pose's palms.
        fill_rect(&mut img, 10, 15, 2, 3, pal.shirt); // sleeve
        fill_rect(&mut img, 10, 12, 2, 3, pal.skin); // forearm
        fill_rect(&mut img, 8, 8, 4, 4, MUG);
        fill_rect(&mut img, 8, 8, 4, 1, COFFEE); // brew at the rim
        px(&mut img, 12, 10, MUG); // handle
    } else {
        fill_rect(&mut img, 11, 17, 5, 2, pal.skin);
        fill_rect(&mut img, 10, 17, 2, 2, pal.shirt);
    }
    // legs under desk
    fill_rect(&mut img, 4, 21, 2, 6, pal.pants);
    fill_rect(&mut img, 8, 21, 2, 6, pal.pants);
    fill_rect(&mut img, 3, 26, 3, 2, OUTLINE);
    fill_rect(&mut img, 8, 26, 3, 2, OUTLINE);

    // Thinking bubble + the mug parked on the desk: the other half of the idle
    // cycle from the sip above. Both hang off the same `frame % 4 < 2` test, so
    // the mug is never in two places at once and the pose count stays at 2.
    if !typing && !sipping {
        px(&mut img, 12, 2, [230, 230, 240, 255]);
        px(&mut img, 13, 1, [230, 230, 240, 255]);
        filled_body(&mut img, 14, 0, 5, 3, [240, 240, 248, 255]);
        px(&mut img, 15, 1, OUTLINE);
        px(&mut img, 17, 1, OUTLINE);
        // Mug on the desktop, clear of the keyboard (y 19..) and the resting
        // arm (which ends at x 15). Solid, for the same reason the raised mug
        // above is.
        fill_rect(&mut img, 17, 15, 3, 3, MUG);
        fill_rect(&mut img, 17, 15, 3, 1, COFFEE); // brew seen from above
        px(&mut img, 20, 16, MUG); // handle
    }
    img
}

/// Canonical `frame` for [`sprite_developer_fail`] (RC2 §4 #1) — the shudder
/// and the error monitor's blink both read `frame % 2`, so the period is **2**.
pub fn fail_frame_key(frame: u8) -> u8 {
    frame % 2
}

/// Developer after a failed subagent: head in hands, red error monitor.
///
/// Same station furniture as [`sprite_developer_at_desk`] (the desk must not
/// move when the phase flips), but the figure is hunched with both palms over
/// its face and the keyboard is dead. Two frames, a 1px shudder apart.
pub fn sprite_developer_fail(pal: DevPalette, frame: u8) -> RgbaImage {
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
    // Abandoned keyboard — no key ever lights on this pose.
    filled_body(&mut img, 18, 19, 10, 3, [40, 44, 56, 255]);
    for kx in 0..5 {
        px(&mut img, 19 + kx * 2, 20, [72, 80, 96, 255]);
    }
    // mouse
    filled_body(&mut img, 29, 19, 3, 2, [36, 40, 52, 255]);
    // sticky note / papers
    filled_body(&mut img, 31, 16, 3, 3, [255, 240, 140, 255]);
    px(&mut img, 32, 17, [80, 80, 100, 255]);

    // Red error monitor + a second screen that went dark red with it
    let mon = sprite_square_monitor(MonitorMode::Error, frame);
    blit_local(&mut img, &mon, 19, 1);
    filled_body(&mut img, 30, 3, 4, 9, [40, 44, 56, 255]);
    fill_rect(&mut img, 31, 4, 2, 6, [56, 16, 22, 255]);

    // Chair
    filled_body(&mut img, 1, 14, 10, 9, chair);
    fill_rect(&mut img, 2, 15, 8, 3, chair_d);
    fill_rect(&mut img, 4, 25, 4, 2, metal);
    px(&mut img, 2, 27, metal);
    px(&mut img, 9, 27, metal);

    // Slumped body: the torso stays on the chair and everything above it drops
    // into the hands, shuddering 1px between the two frames.
    filled_body(&mut img, 3, 13, 8, 8, pal.shirt);
    px(&mut img, 6, 15, pal.accent);
    let hy = 7 + (frame % 2) as i32;
    filled_body(&mut img, 4, hy, 7, 8, pal.skin);
    fill_rect(&mut img, 4, hy - 1, 7, 3, pal.hair);
    px(&mut img, 3, hy + 1, pal.hair);
    px(&mut img, 11, hy + 1, pal.hair);
    // Both palms clamped over the *whole* face: no eyes, no mouth, which is the
    // entire pose. Shaded a step off the skin — palms drawn in the plain skin
    // tone merged into the head and read as an ordinary face with a wide mouth,
    // and OUTLINE finger pixels read as eyes.
    let palm = shade(pal.skin, 82);
    fill_rect(&mut img, 3, hy + 3, 9, 5, palm);
    fill_rect(&mut img, 3, hy + 3, 9, 1, shade(pal.skin, 108));
    for finger in [5, 7, 9] {
        fill_rect(&mut img, finger, hy + 4, 1, 4, shade(pal.skin, 58));
    }
    // Forearms rising from the torso into the palms.
    fill_rect(&mut img, 2, hy + 8, 3, 3, pal.shirt);
    fill_rect(&mut img, 9, hy + 8, 3, 3, pal.shirt);
    // legs under desk
    fill_rect(&mut img, 4, 21, 2, 6, pal.pants);
    fill_rect(&mut img, 8, 21, 2, 6, pal.pants);
    fill_rect(&mut img, 3, 26, 3, 2, OUTLINE);
    fill_rect(&mut img, 8, 26, 3, 2, OUTLINE);
    img
}

/// Canonical `frame` for [`sprite_developer_celebrate`] (RC2 §4 #2) — the hop
/// and the arm raise both read `frame % 2`, so the period is **2**.
pub fn celebrate_frame_key(frame: u8) -> u8 {
    frame % 2
}

/// Developer out of the chair with both arms up after a successful subagent.
///
/// The chair is drawn empty behind the figure so the pose reads as *standing*,
/// and the monitor is pinned to the [`MONITOR_GLOW`] compile-flash frame: a
/// frame-dependent monitor would push this sprite's period from 2 to 4 and
/// double its cache footprint for a screen nobody is looking at.
pub fn sprite_developer_celebrate(pal: DevPalette, frame: u8) -> RgbaImage {
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
    filled_body(&mut img, 18, 19, 10, 3, [40, 44, 56, 255]);
    for kx in 0..5 {
        px(&mut img, 19 + kx * 2, 20, [72, 80, 96, 255]);
    }
    filled_body(&mut img, 29, 19, 3, 2, [36, 40, 52, 255]);
    filled_body(&mut img, 31, 16, 3, 3, [255, 240, 140, 255]);
    px(&mut img, 32, 17, [80, 80, 100, 255]);

    // Monitor stuck on the green compile flash, plus the slim second screen.
    let mon = sprite_square_monitor(MonitorMode::Active, 3);
    blit_local(&mut img, &mon, 19, 1);
    filled_body(&mut img, 30, 3, 4, 9, [40, 44, 56, 255]);
    fill_rect(&mut img, 31, 4, 2, 6, [20, 48, 40, 255]);

    // Empty chair — the figure has jumped clear of it.
    filled_body(&mut img, 1, 14, 10, 9, chair);
    fill_rect(&mut img, 2, 15, 8, 3, chair_d);
    fill_rect(&mut img, 4, 25, 4, 2, metal);
    px(&mut img, 2, 27, metal);
    px(&mut img, 9, 27, metal);

    // 2px hop between the frames; the arms swing a further 4px on top of it so
    // the \o/ is unmistakable at terminal resolution.
    let hop = (frame % 2) as i32 * 2;
    filled_body(&mut img, 3, 11 - hop, 8, 9, pal.shirt);
    fill_rect(
        &mut img,
        5,
        11 - hop,
        4,
        1,
        [
            pal.shirt[0].saturating_add(20),
            pal.shirt[1].saturating_add(20),
            pal.shirt[2].saturating_add(20),
            255,
        ],
    );
    px(&mut img, 6, 13 - hop, pal.accent);
    // Head, tipped back
    filled_body(&mut img, 4, 3 - hop, 7, 8, pal.skin);
    fill_rect(&mut img, 4, 2 - hop, 7, 3, pal.hair);
    px(&mut img, 3, 4 - hop, pal.hair);
    px(&mut img, 11, 4 - hop, pal.hair);
    // Eyes squeezed shut + open grin
    fill_rect(&mut img, 5, 6 - hop, 2, 1, OUTLINE);
    fill_rect(&mut img, 8, 6 - hop, 2, 1, OUTLINE);
    fill_rect(&mut img, 6, 9 - hop, 3, 2, [160, 70, 70, 255]);
    // Arms: sleeve off the shoulder, bare hand above and outboard of it.
    let arm_y = 6 - hop * 2;
    fill_rect(&mut img, 2, arm_y + 3, 2, 4, pal.shirt);
    fill_rect(&mut img, 1, arm_y, 2, 4, pal.skin);
    fill_rect(&mut img, 11, arm_y + 3, 2, 4, pal.shirt);
    fill_rect(&mut img, 12, arm_y, 2, 4, pal.skin);
    // Legs, off the seat
    fill_rect(&mut img, 4, 20 - hop, 2, 6, pal.pants);
    fill_rect(&mut img, 8, 20 - hop, 2, 6, pal.pants);
    fill_rect(&mut img, 3, 25 - hop, 3, 2, OUTLINE);
    fill_rect(&mut img, 8, 25 - hop, 3, 2, OUTLINE);
    img
}

/// Canonical `frame` for [`sprite_developer_walk`] (RC2 P8) — the limb swap is
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

/// Canonical `frame` for [`sprite_supervisor`] (RC2 P8), per phase:
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
            // Steam. Two pixels wide, not one: this animation has been in the
            // sprite since RC13 and nobody ever saw it, first because compose
            // pinned the idle frame to 0 (RC2 §4 #7 un-pins it) and second
            // because a 1px wisp cannot survive the Nearest downsample.
            if frame % 2 == 0 {
                fill_rect(&mut img, 25, 11, 2, 1, [230, 230, 240, 220]);
                px(&mut img, 26, 10, [230, 230, 240, 170]);
            } else {
                fill_rect(&mut img, 26, 10, 2, 1, [230, 230, 240, 220]);
                px(&mut img, 25, 11, [230, 230, 240, 170]);
            }
            // tiny desk plant
            fill_rect(&mut img, 30, 15, 2, 2, [64, 160, 80, 255]);
            fill_rect(&mut img, 30, 17, 2, 2, [160, 100, 60, 255]);
        }
    }
    img
}

/// Canonical `frame` for [`sprite_mcp_server`] (RC2 §3 step 2).
///
/// Idle art reads no frame at all, so every idle frame collapses onto one key.
/// The active art reads `frame % 2` (badge, LED brightness, link LED),
/// `(frame + row) % 3` (the blade chase) and `frame % 4` (the pulse bar), so its
/// period is `lcm(2, 3, 4)` = **12**. The office only ever feeds it the
/// `(tick / 4) % 4` bucket, so 4 of those 12 are reachable in practice.
pub fn mcp_rack_frame_key(active: bool, frame: u8) -> u8 {
    if active { frame % 12 } else { 0 }
}

/// MCP server rack. When `active`, LEDs chase and the status bar pulses.
///
/// Composed by [`super::compose`] onto the "MCP SERVER" rack the mockup bakes
/// into the right wall (RC2 §3 step 2). Until then this was `#[cfg(test)]`
/// scaffolding with no call site.
///
/// FEATURE SIZE: every animated element is >= 2×2 sprite pixels. The composed
/// frame is Nearest-downsampled by [`effective_pixel_scale`] (2 or 3) and this
/// prop draws at scale 1 on ordinary terminals (see `compose::rack_scale`), so
/// the 1px LEDs and 1px pulse bar it shipped with could fall between samples and
/// leave a dead grey box. That is also why there are four 4px blade rows rather
/// than the original six 3px ones: [`filled_body`] spends the first and last row
/// of a bay on its outline, so a 3px bay has exactly *one* interior row and
/// nowhere to put a 2px LED.
pub fn sprite_mcp_server(active: bool, frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(18, 28, Rgba(CLEAR));
    let chassis = [36, 40, 52, 255];
    let chassis_d = [24, 28, 36, 255];
    let chassis_h = [56, 64, 80, 255];
    let bezel = [20, 24, 32, 255];

    // Outer rack
    filled_body(&mut img, 1, 0, 16, 27, chassis);
    fill_rect(&mut img, 2, 1, 14, 1, chassis_h);
    fill_rect(&mut img, 2, 26, 14, 1, chassis_d);
    // Feet
    fill_rect(&mut img, 2, 27, 3, 1, chassis_d);
    fill_rect(&mut img, 13, 27, 3, 1, chassis_d);

    // Top badge strip
    fill_rect(&mut img, 3, 2, 12, 3, bezel);
    // "MCP" marks
    let badge = if active && frame % 2 == 0 {
        [120, 255, 180, 255]
    } else {
        [80, 200, 140, 255]
    };
    fill_rect(&mut img, 4, 3, 2, 2, badge);
    fill_rect(&mut img, 7, 3, 2, 2, badge);
    fill_rect(&mut img, 10, 3, 3, 2, badge);

    // Drive bays / blade rows
    for row in 0..4 {
        let y = 6 + row * 4;
        filled_body(&mut img, 3, y, 12, 4, bezel);
        // left handle
        fill_rect(&mut img, 4, y + 1, 2, 2, chassis_h);
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
        fill_rect(&mut img, 9, y + 1, 2, 2, led);
        let link = if active && frame % 2 == 0 {
            [120, 220, 255, 255]
        } else {
            [40, 80, 120, 255]
        };
        fill_rect(&mut img, 12, y + 1, 2, 2, link);
    }

    // Bottom activity bar
    fill_rect(&mut img, 3, 23, 12, 2, bezel);
    if active {
        let pulse = (frame % 4) as i32;
        fill_rect(&mut img, 4 + pulse, 23, 4, 2, [80, 255, 160, 255]);
        fill_rect(&mut img, 8 + pulse, 23, 3, 2, [80, 180, 255, 255]);
    } else {
        fill_rect(&mut img, 4, 23, 2, 2, [48, 120, 80, 255]);
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

/// Canonical `frame` for [`sprite_roomba`] (RC2 §4 #11).
///
/// The sprite reads `frame % 2` and nothing else — the status lamp blinks and
/// the side brush swaps corners on the same discriminator — so the whole period
/// is **2** and the floor robot costs two cache keys per scale.
pub fn roomba_frame_key(frame: u8) -> u8 {
    frame % 2
}

/// Office floor-cleaning robot: a squat disc with a status lamp and a side brush.
///
/// Composed by [`super::compose`] on the strip of carpet nearest the viewer, and
/// blitted *after* the desks because that strip is in front of them (RC2 §4 #11).
///
/// FEATURE SIZE: both animated elements are 2×2 sprite pixels. The robot draws at
/// scale 1 on ordinary terminals and the composed frame is then Nearest-
/// downsampled by [`effective_pixel_scale`] (2 or 3), so a 1px lamp could fall
/// between samples and leave a dead grey lozenge — the same trap
/// [`sprite_mcp_server`] hit. The lamp is **3×3**, not the 2×2 minimum the rest
/// of this file uses: Nearest picks `floor((out + 0.5) * 3)`, i.e. one source row
/// in every three, and a 2px band really does fall between those samples at some
/// canvas heights (it did, measured, at a 300×180 canvas — the robot rendered as
/// a grey lozenge with no light on it at all). Only a 3px span is guaranteed to
/// contain a sample on both axes, and the lamp is the one feature that says the
/// robot is alive. The bumper ring is a mid grey rather than a bright one for a
/// related reason: at terminal resolution the whole robot is ~5×3 pixels, so
/// whatever is brightest *becomes* the robot.
pub fn sprite_roomba(frame: u8) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(14, 8, Rgba(CLEAR));
    // Deliberately dark and low-contrast except for the lamp. Once the office is
    // downsampled the whole robot is ~5×3 terminal pixels, so whatever is
    // brightest *is* the robot — a pale bumper ring read as a grey lozenge with
    // no character at all until it was taken down to a mid grey.
    let shell = [52, 58, 74, 255];
    let shell_hi = [74, 82, 100, 255];
    let shell_d = [34, 38, 50, 255];
    let bumper = [104, 112, 132, 255];
    let bristle = [188, 160, 96, 255];
    let lit = frame % 2 == 0;

    // Disc seen from a low angle: lit top, bumper ring at its widest, dark
    // underside, then a soft contact shadow so it sits on the carpet.
    fill_rect(&mut img, 3, 0, 8, 1, shell_hi);
    fill_rect(&mut img, 1, 1, 12, 1, shell_hi);
    fill_rect(&mut img, 0, 2, 14, 1, shell);
    fill_rect(&mut img, 0, 3, 14, 2, bumper);
    fill_rect(&mut img, 1, 5, 12, 1, shell_d);
    fill_rect(&mut img, 3, 6, 8, 1, [26, 30, 40, 200]);
    fill_rect(&mut img, 4, 7, 6, 1, [22, 26, 34, 120]);

    // Status lamp: a 3×3 dome, the one thing that has to survive the downsample.
    let lamp = if lit {
        [128, 255, 184, 255]
    } else {
        [40, 128, 92, 255]
    };
    fill_rect(&mut img, 6, 1, 3, 3, lamp);
    // Side brush, alternating corners — a second animated feature so the robot
    // still reads as *moving* on the frame where the lamp is between blinks.
    fill_rect(&mut img, if lit { 0 } else { 12 }, 4, 2, 2, bristle);
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
        let mon = sprite_square_monitor(MonitorMode::Active, 0);
        assert_eq!(mon.width(), mon.height());
        // Detailed desk sprites — keep within composable bounds
        assert!(sprite_empty_desk().width() <= 40);
        assert!(sprite_developer_at_desk(DevPalette::by_index(0), false, 0).width() <= 40);
        assert_eq!(sprite_mcp_server(true, 1).width(), 18);
        assert!(!sprite_mcp_server(false, 0).as_raw().is_empty());
        assert!(!sprite_supervisor(1, 2).as_raw().is_empty());
    }

    /// RC2 §3 step 2: the rack's cache key must be the sprite's real period —
    /// no wider (duplicate entries for identical art) and no narrower (two
    /// different pictures sharing one entry, i.e. a frozen animation).
    #[test]
    fn mcp_rack_frame_key_matches_the_sprite_period() {
        let idle: Vec<RgbaImage> = (0..24u8).map(|f| sprite_mcp_server(false, f)).collect();
        for f in 0..24u8 {
            assert_eq!(mcp_rack_frame_key(false, f), 0);
            assert_eq!(
                idle[f as usize].as_raw(),
                idle[0].as_raw(),
                "the idle rack must not animate (frame {f})"
            );
        }

        let active: Vec<RgbaImage> = (0..24u8).map(|f| sprite_mcp_server(true, f)).collect();
        for a in 0..24u8 {
            for b in 0..24u8 {
                let same_key = mcp_rack_frame_key(true, a) == mcp_rack_frame_key(true, b);
                let same_art = active[a as usize].as_raw() == active[b as usize].as_raw();
                assert_eq!(
                    same_key, same_art,
                    "frames {a}/{b}: key says same={same_key}, art says same={same_art}"
                );
            }
        }
        // All 12 of the active period really are distinct pictures, and none of
        // them is the idle rack — the LEDs have to read as "MCP is busy".
        assert_eq!(
            (0..12u8)
                .map(|f| active[f as usize].as_raw().clone())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            12
        );
        assert!((0..12u8).all(|f| active[f as usize].as_raw() != idle[0].as_raw()));
    }

    /// The rack draws at scale 1 on ordinary terminals and the composed frame is
    /// then Nearest-downsampled by 2 or 3, so a 1px feature can fall between
    /// samples. Every animated element must be at least 2×2.
    #[test]
    fn mcp_rack_animated_features_survive_the_downsample() {
        let a = sprite_mcp_server(true, 0);
        let b = sprite_mcp_server(true, 1);
        let (w, h) = a.dimensions();
        let mut runs = 0usize;
        for y in 0..h - 1 {
            for x in 0..w - 1 {
                let quad = [(0u32, 0u32), (1, 0), (0, 1), (1, 1)];
                if quad
                    .iter()
                    .all(|(dx, dy)| a.get_pixel(x + dx, y + dy) != b.get_pixel(x + dx, y + dy))
                {
                    runs += 1;
                }
            }
        }
        assert!(
            runs > 0,
            "no 2×2 block changes between frames — the chase would vanish at terminal res"
        );
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

    /// RC2 P8: the compose sprite cache keys on the declared canonical frame,
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
            let key = fail_frame_key(frame);
            assert_eq!(
                sprite_developer_fail(pal, frame).into_raw(),
                sprite_developer_fail(pal, key).into_raw(),
                "fail frame={frame} != key={key}"
            );
            let key = celebrate_frame_key(frame);
            assert_eq!(
                sprite_developer_celebrate(pal, frame).into_raw(),
                sprite_developer_celebrate(pal, key).into_raw(),
                "celebrate frame={frame} != key={key}"
            );
            let key = roomba_frame_key(frame);
            assert_eq!(
                sprite_roomba(frame).into_raw(),
                sprite_roomba(key).into_raw(),
                "roomba frame={frame} != key={key}"
            );
        }
    }

    /// RC2 §4 #11: the floor robot is ~5×3 pixels once the office is
    /// downsampled, so its lamp is the whole animation. Nearest picks one source
    /// row (and column) in every `effective_pixel_scale`, which is 3 on ordinary
    /// terminals — so the lamp must span 3px on **both** axes or a stage whose
    /// geometry lands the samples between its rows renders a lightless lozenge.
    #[test]
    fn roomba_lamp_survives_a_stride_three_downsample() {
        let a = sprite_roomba(0);
        let b = sprite_roomba(1);
        assert_eq!(a.dimensions(), (14, 8));
        assert_ne!(a.as_raw(), b.as_raw(), "the two frames must differ");

        // Every 3×3 sample phase must see the lamp change.
        for oy in 0..3u32 {
            for ox in 0..3u32 {
                let mut moved = false;
                let mut y = oy;
                while y < 8 {
                    let mut x = ox;
                    while x < 14 {
                        if a.get_pixel(x, y) != b.get_pixel(x, y) {
                            moved = true;
                        }
                        x += 3;
                    }
                    y += 3;
                }
                assert!(
                    moved,
                    "sample phase ({ox}, {oy}) sees no animation at all"
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
        assert_eq!(
            distinct(
                (0..2u8)
                    .map(|f| sprite_developer_fail(pal, f).into_raw())
                    .collect()
            ),
            2,
            "the debug-rage pose must have 2 distinct frames"
        );
        assert_eq!(
            distinct(
                (0..2u8)
                    .map(|f| sprite_developer_celebrate(pal, f).into_raw())
                    .collect()
            ),
            2,
            "the celebrate pose must have 2 distinct frames"
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

    /// RC2 §4 #4: the monitor glow must live *inside* the four frames the
    /// sprite already animates (so it costs no cache keys), must actually vary
    /// across them, must peak on the compile-flash frame, and must leave an
    /// inactive monitor — i.e. the empty desk — completely alone.
    #[test]
    fn monitor_glow_rides_the_existing_typing_frames() {
        // A bezel pixel that is neither screen interior nor a corner.
        let bezel_lum = |img: &RgbaImage| -> u32 {
            let p = img.get_pixel(6, 0).0;
            u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
        };
        let lums: Vec<u32> = (0..4u8)
            .map(|f| bezel_lum(&sprite_square_monitor(MonitorMode::Active, f)))
            .collect();
        assert!(
            lums.iter().collect::<std::collections::HashSet<_>>().len() == 4,
            "glow must differ on all four frames, got {lums:?}"
        );
        assert_eq!(
            lums.iter().copied().max(),
            Some(lums[3]),
            "frame 3 is the compile flash and must be the brightest: {lums:?}"
        );

        // Inactive monitors keep the flat bezel (empty desk art is unchanged).
        let off = sprite_square_monitor(MonitorMode::Off, 0);
        assert_eq!(
            bezel_lum(&off),
            bezel_lum(&sprite_square_monitor(MonitorMode::Off, 3))
        );

        // The rim must not bleed into the screen interior: (2,2) is inside the
        // screen and left of the scrolling code, so it stays the base color.
        for f in 0..4u8 {
            assert_eq!(
                sprite_square_monitor(MonitorMode::Active, f).get_pixel(2, 2).0,
                [12, 20, 28, 255],
                "frame {f}: glow washed the screen instead of rimming it"
            );
        }

        // ...and the glow must survive the trip into the seated developer.
        let pal = DevPalette::by_index(0);
        let dev: Vec<u32> = (0..4u8)
            .map(|f| {
                let img = sprite_developer_at_desk(pal, true, f);
                let p = img.get_pixel(25, 1).0; // monitor blitted at (19, 1)
                u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
            })
            .collect();
        assert_eq!(dev, lums, "typing dev must show the same bezel glow ramp");
    }

    /// RC2 §4 #1: the fail beat must be its own pose — the face buried in both
    /// palms (so no eye highlight survives) over a red error monitor — not the
    /// ordinary seated developer with a flash painted over him.
    #[test]
    fn fail_pose_buries_the_face_and_reddens_the_monitor() {
        let pal = DevPalette::by_index(2);
        let f0 = sprite_developer_fail(pal, 0);
        let f1 = sprite_developer_fail(pal, 1);
        assert_ne!(f0.as_raw(), f1.as_raw(), "the shudder must move pixels");
        assert_ne!(
            f0.as_raw(),
            sprite_developer_at_desk(pal, false, 0).as_raw(),
            "the fail beat must not reuse the ordinary seated pose"
        );

        // The seated developer has a white eye highlight; this pose has no eyes.
        let white = |img: &RgbaImage| img.pixels().any(|p| p.0 == [255, 255, 255, 255]);
        assert!(white(&sprite_developer_at_desk(pal, false, 0)));
        assert!(!white(&f0), "frame 0: the palms must cover the eyes");
        assert!(!white(&f1), "frame 1: the palms must cover the eyes");

        // ...and the palms really cover the face, in a tone that separates them
        // from the head: plain-skin palms merged into it and read as a face.
        for (img, frame) in [(&f0, 0u8), (&f1, 1)] {
            let hy = 7 + u32::from(frame % 2);
            for y in (hy + 3)..(hy + 8) {
                for x in 4..11u32 {
                    let p = img.get_pixel(x, y).0;
                    assert_ne!(p, pal.skin, "frame {frame}: bare face at ({x},{y})");
                    assert_ne!(p, OUTLINE, "frame {frame}: an eye survived at ({x},{y})");
                }
            }
        }

        // Error screen and its rim spill are red-dominant, and the bar blinks.
        let err0 = sprite_square_monitor(MonitorMode::Error, 0);
        let err1 = sprite_square_monitor(MonitorMode::Error, 1);
        assert_ne!(err0.as_raw(), err1.as_raw(), "the error bar must blink");
        for img in [&err0, &err1] {
            let screen = img.get_pixel(2, 2).0;
            assert!(
                screen[0] > screen[1] && screen[0] > screen[2],
                "error screen must be red-dominant, got {screen:?}"
            );
            let bezel = img.get_pixel(6, 0).0;
            assert!(
                bezel[0] > bezel[1] && bezel[0] > bezel[2],
                "error glow must spill red onto the bezel, got {bezel:?}"
            );
        }
        // ...and the other modes are untouched by the new branch.
        let active = sprite_square_monitor(MonitorMode::Active, 0).get_pixel(2, 2).0;
        assert!(active[2] > active[0], "active screen must stay cool-toned");
        assert_eq!(
            sprite_square_monitor(MonitorMode::Off, 0).get_pixel(2, 2).0,
            [20, 26, 34, 255],
            "an off screen must not pick up any glow"
        );
    }

    /// RC2 §4 #2: the celebrate pose must raise both arms clear of the
    /// shoulders and throw them higher on the second frame — two frames that
    /// differ only by a hop would read as a twitch, not a cheer.
    #[test]
    fn celebrate_pose_throws_both_arms_up() {
        let pal = DevPalette::by_index(0);
        // Arm columns only: the head spans x4..10 and the torso x3..10, so skin
        // found outside those columns can only be a raised hand.
        let arm_cols = [[1u32, 2], [12, 13]];
        let arm_top = |img: &RgbaImage| -> Option<u32> {
            (0..img.height()).find(|y| {
                arm_cols
                    .iter()
                    .flatten()
                    .any(|x| img.get_pixel(*x, *y).0 == pal.skin)
            })
        };
        let f0 = sprite_developer_celebrate(pal, 0);
        let f1 = sprite_developer_celebrate(pal, 1);
        let t0 = arm_top(&f0).expect("frame 0 must raise an arm");
        let t1 = arm_top(&f1).expect("frame 1 must raise an arm");
        assert!(t1 < t0, "frame 1 must throw the arms higher ({t1} vs {t0})");

        // Both arms, not one.
        for (img, label) in [(&f0, 0), (&f1, 1)] {
            for cols in arm_cols {
                assert!(
                    (0..img.height())
                        .any(|y| cols.iter().any(|x| img.get_pixel(*x, y).0 == pal.skin)),
                    "frame {label}: columns {cols:?} have no raised hand"
                );
            }
        }
        // The chair is still drawn behind the figure — this pose is standing.
        assert!(
            f0.pixels().any(|p| p.0 == [56, 64, 88, 255]),
            "the empty chair must still be drawn"
        );
        assert_ne!(
            f0.as_raw(),
            sprite_developer_at_desk(pal, false, 0).as_raw(),
            "celebrate must not reuse the ordinary seated pose"
        );
    }

    /// RC2 §4 #6: two distinct door states, both within the composable size
    /// budget the other ambient props use.
    #[test]
    fn door_has_two_distinct_states() {
        let closed = sprite_door(false);
        let open = sprite_door(true);
        assert_eq!(closed.dimensions(), (12, 16));
        assert_eq!(open.dimensions(), closed.dimensions());
        assert_ne!(
            closed.as_raw(),
            open.as_raw(),
            "the swing must actually change pixels"
        );
        // The opening must be visibly darker than the closed leaf.
        let lum = |img: &RgbaImage, x: u32, y: u32| {
            let p = img.get_pixel(x, y).0;
            u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])
        };
        assert!(
            lum(&open, 8, 8) < lum(&closed, 6, 3),
            "an open door must show a dark opening"
        );
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
