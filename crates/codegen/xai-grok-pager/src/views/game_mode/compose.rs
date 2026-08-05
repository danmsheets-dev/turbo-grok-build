//! Composite office background + sprites at **high internal resolution**.
//!
//! - `PIXEL_SCALE` denser than terminal halfblock cells for sharper SNES detail
//! - Floor clears use SNES carpet tiles (no diagonal green mask)
//! - Smaller desk sprites relative to the room

use image::imageops::FilterType;
use image::RgbaImage;

use super::sprites_pixel::{
    DevPalette, blit, celebrate_frame_key, dev_at_desk_frame_key, fail_frame_key,
    mcp_rack_frame_key, scale_nn, sprite_coffee, sprite_developer_at_desk,
    sprite_developer_celebrate, sprite_developer_fail, sprite_developer_walk, sprite_door,
    sprite_empty_desk, sprite_mcp_server, sprite_plant, sprite_supervisor,
    stamp_floor_patch_sampled, supervisor_frame_key, walk_frame_key,
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

/// Footprint cleared + rugged under the supervisor, as fractions of the canvas.
///
/// Also the supervisor's hover hit box — see [`supervisor_hit_rect`].
const SUPERVISOR_COVER_W_FRAC: f32 = 0.13;
const SUPERVISOR_COVER_H_FRAC: f32 = 0.14;

/// Door position as a fraction of frame width — actors enter (SpawnWalk) and
/// leave (ExitDoor) through it.
const DOOR_X_FRAC: f32 = 0.06;

/// Top of the door prop, as a fraction of frame height.
///
/// The mockup has no door, so the prop is placed on open carpet: the lowest
/// non-carpet pixel of the baked bookshelf in the [`DOOR_X_FRAC`] column is at
/// 0.4401h (measured off `office_bg.png`) and the ambient plant starts at
/// 0.62h, which leaves this band clear. Nothing has to be erased first — the
/// compose pass resets the whole canvas from the background every frame, so a
/// floor stamp here would only replace clean carpet with a slightly different
/// carpet and leave a visible patch.
const DOOR_Y_FRAC: f32 = 0.46;

/// `anim_t` window at each end of the walk during which the door stands open.
///
/// Bucket-aligned on purpose — see [`door_is_open`].
const DOOR_OPEN_ENTER_T: f32 = 0.25;
const DOOR_OPEN_EXIT_T: f32 = 0.75;

/// Top-left blit corners of the static ambient props, as canvas fractions.
///
/// Unlike [`DESK_ANCHORS`] / [`SUPERVISOR_ANCHOR`] / [`RACK_ANCHOR`] these are
/// *corners*, not centres — the props are blitted straight at them. Named so the
/// placement test can assert the rack clears them instead of re-typing literals.
const PLANT_L_ANCHOR: (f32, f32) = (0.06, 0.62);
const PLANT_R_ANCHOR: (f32, f32) = (0.90, 0.58);
/// Moved down from 0.40h in RC16 §3 step 2: at 0.40h the mug floated on the wall
/// *inside* the rack footprint below. 0.48h stands it on the carpet in front of
/// the rack, clear of both the rack (ends 0.46h) and the right plant (0.58h).
const COFFEE_ANCHOR: (f32, f32) = (0.88, 0.48);

/// Centre of the MCP server rack, as a fraction of the canvas.
///
/// Measured off `office_bg.png`, which already bakes a labelled "MCP SERVER"
/// rack into the right wall at x 1309..1435, y 192..498 of 1448×1086. The
/// composed sprite is centred on and scaled *into* that footprint (see
/// [`rack_scale`]) so the room keeps exactly one rack: the baked chassis frames
/// the live blades, and the two share a palette to within a few units
/// (baked #2B2D39 vs sprite #242834).
///
/// TIER NOTE: the rack is **pixel-office only**. The Unicode fallback is a
/// wall/supervisor/handoff/desks/door stack with no spare geometry to stand a
/// rack in, and Compact is a card grid with no office at all — a glyph there
/// would have to displace a desk card. So the rack tooltip must be gated on
/// `render::render_game_mode`'s `pixel_painted`, exactly like the supervisor
/// hit rect: a default `Rect` never hit-tests.
const RACK_ANCHOR: (f32, f32) = (0.947, 0.318);

/// Footprint of the baked rack, as fractions of the canvas.
///
/// Also the rack's hover hit box for RC16 §3 step 3 — same contract as
/// [`SUPERVISOR_COVER_W_FRAC`], and the reason [`rack_scale`] never lets the
/// sprite grow past it.
const RACK_COVER_W_FRAC: f32 = 0.087;
const RACK_COVER_H_FRAC: f32 = 0.283;

/// Cell rect covering the composed supervisor — the pixel office's hover hit box.
///
/// Derived from the same fractions the compose pass places the sprite with
/// ([`SUPERVISOR_ANCHOR`] centre, `SUPERVISOR_COVER_*_FRAC` footprint) so the box
/// tracks the drawn art instead of guessing at it. Both fractions are of the
/// compose *canvas*, and the canvas is `cell_w*scale` by `cell_h*2*scale` — the
/// halfblock doubling and the scale cancel in a fraction, so the identical
/// fractions apply to the cell-space stage rect.
pub(super) fn supervisor_hit_rect(stage: ratatui::layout::Rect) -> ratatui::layout::Rect {
    if stage.width == 0 || stage.height == 0 {
        return ratatui::layout::Rect::default();
    }
    let (w, h) = (f32::from(stage.width), f32::from(stage.height));
    let cover_w = (w * SUPERVISOR_COVER_W_FRAC).round().max(1.0);
    let cover_h = (h * SUPERVISOR_COVER_H_FRAC).round().max(1.0);
    let x = (w * SUPERVISOR_ANCHOR.0 - cover_w / 2.0).max(0.0) as u16;
    let y = (h * SUPERVISOR_ANCHOR.1 - cover_h / 2.0).max(0.0) as u16;
    ratatui::layout::Rect {
        x: stage.x.saturating_add(x),
        y: stage.y.saturating_add(y),
        width: (cover_w as u16).min(stage.width.saturating_sub(x)),
        height: (cover_h as u16).min(stage.height.saturating_sub(y)),
    }
}

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

/// Scale for the static ambient props (door / plants / coffee).
fn prop_scale(w: u32) -> u32 {
    desk_scale(w).max(1).min(3)
}

/// The baked rack's footprint in canvas pixels: `(x, y, w, h)`.
///
/// Single source for [`rack_scale`] and for the placement test — and, when the
/// rack tooltip lands, for its hit rect.
fn rack_cover_px(w: u32, h: u32) -> (f32, f32, f32, f32) {
    let cover_w = w as f32 * RACK_COVER_W_FRAC;
    let cover_h = h as f32 * RACK_COVER_H_FRAC;
    (
        w as f32 * RACK_ANCHOR.0 - cover_w / 2.0,
        h as f32 * RACK_ANCHOR.1 - cover_h / 2.0,
        cover_w,
        cover_h,
    )
}

/// Scale for the 18×28 [`sprite_mcp_server`] on a `w`×`h` canvas.
///
/// Derived from the baked rack's own footprint rather than from [`desk_scale`],
/// and **floored** so the sprite always fits *inside* it. That asymmetry is
/// deliberate: an undershoot is invisible (the baked chassis behind it is the
/// same near-black, and its own blade rows continue the pattern) while an
/// overshoot would push a hard dark edge onto the grey wall above and the carpet
/// below, and — at tall canvas aspect ratios, where the height term is the
/// larger one — could reach the coffee prop and desk 3's clear area. Fitting is
/// what makes the placement test's "clears every desk and prop" claim hold at
/// *every* geometry rather than at the ones that happen to round down.
///
/// The `max(1)` floor can only bite below ~216×108 canvas pixels, i.e. under the
/// smallest stage the pixel office paints at the default `PIXEL_SCALE` (72×18
/// cells) — at exactly that size 18×28 still fits the 18.8×30.6 footprint. Below
/// it (only reachable via a `GROK_GAME_PIXEL_SCALE=2` override on a tiny
/// terminal, where every prop is oversized for the same reason) the sprite can
/// spill a few pixels past the footprint.
fn rack_scale(w: u32, h: u32) -> u32 {
    let (_, _, cover_w, cover_h) = rack_cover_px(w, h);
    let fit = (cover_w / 18.0).min(cover_h / 28.0);
    fit.floor().clamp(1.0, 5.0) as u32
}

// Thread-local scaled sprite cache — Arc so blit does not clone every frame.
//
// PERF INVARIANT: compose_cell_frame must stay cheap on tick-only frames.
// Keys are stable per (kind, skin, canonical frame, scale); plant/coffee are
// static. The frame in a key is always the sprite's *canonical* frame (see
// `sprites_pixel::{dev_at_desk_frame_key, walk_frame_key, supervisor_frame_key}`)
// so frames that render identical art share one entry (RC16 P8).
use std::sync::Arc;

/// Cache cap, in entries.
///
/// Measured worst case per scale-set is **103** live keys: 36 seated devs
/// (6 skins × (4 typing frames + 2 idle poses)), 12 debug-rage devs and 12
/// celebrating devs (6 skins × 2 frames each), 24 walkers (6 skins × 2 packet
/// × 2 frames), 9 supervisors (2 idle + 4 working + 3 reviewing reachable from
/// the `(tick/4)%4` frame counter), 2 doors (open / closed), 5 MCP racks (idle
/// plus the 4 active frames the same counter reaches) and 3 statics (empty desk,
/// plant, coffee). Before RC16 P8 frame quantization the character set alone
/// needed 111, which is why a 128-entry cap thrashed.
///
/// 256 leaves room for two full scale-sets (206, i.e. mid-resize) plus ~50 keys
/// of headroom for upcoming animation sprites. It is a backstop only:
/// [`sprite_cache_begin_pass`] drops stale scales eagerly, so the map normally
/// sits at one scale-set.
const SPRITE_CACHE_CAP: usize = 256;

struct CachedSprite {
    img: Arc<RgbaImage>,
    /// Scale this sprite was rasterised at — stale scales are evicted first.
    scale: u32,
    /// Monotonic counter of the last read (LRU ordering).
    used: u64,
}

#[derive(Default)]
struct SpriteCache {
    map: std::collections::HashMap<u64, CachedSprite>,
    clock: u64,
    /// Bitmask of the scales the current compose pass uses; bit `s` is set for
    /// scale `s`. Zero means "unknown", which keeps every entry live.
    live_scales: u64,
}

impl SpriteCache {
    fn scale_is_live(&self, scale: u32) -> bool {
        self.live_scales == 0 || self.live_scales & scale_bit(scale) != 0
    }

    /// Evict one entry: any stale-scale entry first, otherwise the LRU.
    ///
    /// O(len) but only runs on insert past the cap, which the eager stale-scale
    /// sweep makes rare. It never clears the whole map, so the sprites the
    /// current pass has already blitted survive an overflow.
    fn evict_one(&mut self) {
        let victim = self
            .map
            .iter()
            .min_by_key(|(_, e)| (self.scale_is_live(e.scale), e.used))
            .map(|(k, _)| *k);
        if let Some(k) = victim {
            self.map.remove(&k);
        }
    }
}

/// Bit for `scale` in [`SpriteCache::live_scales`] (scales are 1..=5 today).
fn scale_bit(scale: u32) -> u64 {
    1u64 << scale.min(63)
}

thread_local! {
    static SPRITE_CACHE: std::cell::RefCell<SpriteCache> =
        std::cell::RefCell::new(SpriteCache::default());
}

/// Declare the scales this compose pass will draw at and drop everything built
/// at any other scale (RC16 P8).
///
/// A resize changes every derived scale at once, so the previous stage's entries
/// become garbage in a single step. Dropping them here keeps the map at ~one
/// scale-set; the old clear-at-128 eviction instead wiped the sprites the new
/// stage had *just* rasterised, turning a resize into a rebuild storm.
fn sprite_cache_begin_pass(scales: [u32; 5]) {
    let live = scales.iter().fold(0u64, |m, s| m | scale_bit(*s));
    SPRITE_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.live_scales == live {
            return;
        }
        c.live_scales = live;
        c.map.retain(|_, e| live & scale_bit(e.scale) != 0);
    });
}

fn cache_get_or_insert(key: u64, scale: u32, build: impl FnOnce() -> RgbaImage) -> Arc<RgbaImage> {
    SPRITE_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.clock += 1;
        let now = c.clock;
        if let Some(e) = c.map.get_mut(&key) {
            e.used = now;
            return Arc::clone(&e.img);
        }
        while c.map.len() >= SPRITE_CACHE_CAP {
            c.evict_one();
        }
        let img = Arc::new(build());
        c.map.insert(
            key,
            CachedSprite {
                img: Arc::clone(&img),
                scale,
                used: now,
            },
        );
        img
    })
}

#[cfg(test)]
fn sprite_cache_reset() {
    SPRITE_CACHE.with(|c| *c.borrow_mut() = SpriteCache::default());
}

#[cfg(test)]
fn sprite_cache_len() -> usize {
    SPRITE_CACHE.with(|c| c.borrow().map.len())
}

#[cfg(test)]
fn sprite_cache_has_scale(scale: u32) -> bool {
    SPRITE_CACHE.with(|c| c.borrow().map.values().any(|e| e.scale == scale))
}

fn cached_empty_desk(sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    cache_get_or_insert(0xE0u64 << 56 | sc as u64, sc, || {
        scale_nn(&sprite_empty_desk(), sc)
    })
}

fn cached_dev_at_desk(skin: u8, typing: bool, frame: u8, sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    let frame = dev_at_desk_frame_key(typing, frame);
    let key = (0xD1u64 << 56)
        | ((skin as u64) << 40)
        | ((typing as u64) << 32)
        | ((frame as u64) << 24)
        | sc as u64;
    cache_get_or_insert(key, sc, || {
        let pal = DevPalette::by_index(skin);
        scale_nn(&sprite_developer_at_desk(pal, typing, frame), sc)
    })
}

/// +12 keys per scale-set (6 skins × 2 [`fail_frame_key`] frames).
fn cached_dev_fail(skin: u8, frame: u8, sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    let frame = fail_frame_key(frame);
    let key = (0xD3u64 << 56) | ((skin as u64) << 40) | ((frame as u64) << 24) | sc as u64;
    cache_get_or_insert(key, sc, || {
        let pal = DevPalette::by_index(skin);
        scale_nn(&sprite_developer_fail(pal, frame), sc)
    })
}

/// +12 keys per scale-set (6 skins × 2 [`celebrate_frame_key`] frames).
fn cached_dev_celebrate(skin: u8, frame: u8, sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    let frame = celebrate_frame_key(frame);
    let key = (0xD4u64 << 56) | ((skin as u64) << 40) | ((frame as u64) << 24) | sc as u64;
    cache_get_or_insert(key, sc, || {
        let pal = DevPalette::by_index(skin);
        scale_nn(&sprite_developer_celebrate(pal, frame), sc)
    })
}

fn cached_walk(skin: u8, frame: u8, with_packet: bool, sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    let frame = walk_frame_key(frame);
    let key = (0xD2u64 << 56)
        | ((skin as u64) << 40)
        | ((with_packet as u64) << 32)
        | ((frame as u64) << 24)
        | sc as u64;
    cache_get_or_insert(key, sc, || {
        let pal = DevPalette::by_index(skin);
        scale_nn(&sprite_developer_walk(pal, frame, with_packet), sc)
    })
}

fn cached_supervisor(phase: u8, frame: u8, sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    let frame = supervisor_frame_key(phase, frame);
    let key = (0xA0u64 << 56) | ((phase as u64) << 32) | ((frame as u64) << 24) | sc as u64;
    cache_get_or_insert(key, sc, || scale_nn(&sprite_supervisor(phase, frame), sc))
}

fn cached_plant(sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    cache_get_or_insert(0xF1u64 << 56 | sc as u64, sc, || {
        scale_nn(&sprite_plant(), sc)
    })
}

fn cached_coffee(sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    cache_get_or_insert(0xC0u64 << 56 | sc as u64, sc, || {
        scale_nn(&sprite_coffee(), sc)
    })
}

/// +5 keys per scale-set: the single idle frame plus the four active frames the
/// `(tick / 4) % 4` counter can reach ([`mcp_rack_frame_key`] collapses the
/// sprite's full 12-frame active period onto whatever the office samples).
fn cached_rack(active: bool, frame: u8, sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    let frame = mcp_rack_frame_key(active, frame);
    let key = (0xB0u64 << 56) | ((active as u64) << 32) | ((frame as u64) << 24) | sc as u64;
    cache_get_or_insert(key, sc, || scale_nn(&sprite_mcp_server(active, frame), sc))
}

/// +2 keys per scale-set (open / closed) — the door has no frame counter, so
/// `open` is already its canonical key and it needs no `*_frame_key` fn.
fn cached_door(open: bool, sc: u32) -> Arc<RgbaImage> {
    let sc = sc.max(1);
    let key = (0xDDu64 << 56) | ((open as u64) << 32) | sc as u64;
    cache_get_or_insert(key, sc, || scale_nn(&sprite_door(open), sc))
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

/// Pose frame for [`sprite_developer_celebrate`], driven by the desk's `anim_t`.
///
/// TIMING NOTE (RC16 §4 #2): Celebrate lasts 400 ms — about 5 Slow ticks — but
/// the `tick / 4` sprite bucket every other pose rides advances only every
/// ~330 ms, so a bucket-driven celebrate pose showed **one** frame for the whole
/// celebration. `anim_t` is per-desk and advances every tick, and
/// [`super::state`]'s `phase_anim_t_is_visible` now hashes it for Celebrate, so
/// the arms pump three times across the phase — at the cost of recomposing per
/// tick *only* while a desk is celebrating. Lengthening the phase to ~1 s
/// was the alternative; it would have delayed every handoff walk by 600 ms to
/// buy a slower animation.
///
/// FINGERPRINT NOTE: the flip points (0.25 / 0.5 / 0.75) are bucket-aligned
/// against the fingerprint's `(anim_t * 20.0) as u8` quantization exactly like
/// [`door_is_open`] — 4 divides 20 — so two `anim_t` values that share a hash
/// bucket can never disagree about the pose.
fn celebrate_pose_frame(anim_t: f32) -> u8 {
    ((anim_t.clamp(0.0, 1.0) * 4.0) as u8) % 2
}

/// Falling multi-colour confetti over a celebrating desk (RC16 §4 #2).
///
/// Procedural like [`paint_fx_handoff_papers`] — no sprite, so zero cache keys.
/// `t` is the desk's `anim_t`; each piece is released a little later than the
/// one before it and all of them have landed by `t = 1`, so the burst empties
/// instead of freezing mid-air when the phase ends.
///
/// Pieces are >= 3px square for the same reason the handoff sheets are: the
/// composed frame is Nearest-downsampled by `effective_pixel_scale` (2 or 3),
/// and a 2px feature can fall between samples at scale 3.
fn paint_fx_confetti(canvas: &mut RgbaImage, cx: i32, cy: i32, t: f32, w: u32, h: u32) {
    const PIECES: usize = 10;
    const LAG: f32 = 0.06;
    const COLORS: [[u8; 4]; 5] = [
        [255, 220, 96, 255],
        [120, 255, 180, 255],
        [120, 200, 255, 255],
        [255, 120, 180, 255],
        [200, 140, 255, 255],
    ];
    let span = (w as f32 * 0.07).max(10.0);
    let fall = (h as f32 * 0.16).max(12.0);
    let quad = ((w as f32 * 0.010) as i32).clamp(3, 5);
    let (cw, ch) = canvas.dimensions();
    for i in 0..PIECES {
        let p = (t * (1.0 + LAG * (PIECES - 1) as f32) - LAG * i as f32).clamp(0.0, 1.0);
        if p <= 0.0 || p >= 1.0 {
            continue;
        }
        // Deterministic spread: 7 is coprime with PIECES, so every piece gets
        // its own lane across the desk instead of a clump.
        let lane = ((i * 7) % PIECES) as f32 / PIECES as f32 - 0.5;
        let sway = (p * std::f32::consts::PI * 2.0 + i as f32).sin() * span * 0.08;
        let x = cx as f32 + lane * span + sway;
        // Gravity: slow release, quick landing.
        let y = cy as f32 - fall + fall * p * (0.55 + 0.45 * p);
        let c = COLORS[i % COLORS.len()];
        for dy in 0..quad {
            for dx in 0..quad {
                let sx = x as i32 + dx;
                let sy = y as i32 + dy;
                if sx < 0 || sy < 0 || sx as u32 >= cw || sy as u32 >= ch {
                    continue;
                }
                canvas.put_pixel(sx as u32, sy as u32, image::Rgba(c));
            }
        }
    }
}

/// Papers changing hands during [`ActorPhase::Handoff`].
///
/// Procedural like [`paint_fx_confetti`] — no sprite, so zero cache keys. `t`
/// is the desk's `anim_t`; each sheet lags the one before it so the burst reads
/// as a small stack crossing to the supervisor's desk instead of one blob.
///
/// The arc is deliberately shallow and biased *down* from `cy`: the walker and
/// the supervisor share one anchor, which sits mid-torso, so a tall arc throws
/// the sheets across the boss's face instead of over his desk. Quads are >= 3px
/// because the composed frame is Nearest-downsampled by `effective_pixel_scale`
/// (2 or 3), and a 2px feature can fall between samples at scale 3.
fn paint_fx_handoff_papers(canvas: &mut RgbaImage, cx: i32, cy: i32, t: f32, w: u32) {
    const SHEETS: usize = 4;
    const LAG: f32 = 0.15;
    let paper: [u8; 4] = [248, 248, 240, 255];
    let ink: [u8; 4] = [96, 128, 200, 255];
    let span = (w as f32 * 0.05).max(6.0);
    let quad = ((w as f32 * 0.012) as i32).clamp(3, 5);
    let (cw, ch) = canvas.dimensions();
    for i in 0..SHEETS {
        // Stretch the flight so the last (most lagged) sheet still lands by t=1.
        let p = (t * (1.0 + LAG * (SHEETS - 1) as f32) - LAG * i as f32).clamp(0.0, 1.0);
        if p <= 0.0 || p >= 1.0 {
            continue;
        }
        let x = cx as f32 - span * 0.5 + span * p;
        let y = cy as f32 + span * 0.15 - span * 0.45 * (std::f32::consts::PI * p).sin();
        for dy in 0..quad {
            for dx in 0..quad {
                let sx = x as i32 + dx;
                let sy = y as i32 + dy;
                if sx < 0 || sy < 0 || sx as u32 >= cw || sy as u32 >= ch {
                    continue;
                }
                // One ink line per sheet so it reads as paper, not a white blob.
                let c = if dy == quad / 2 && dx > 0 { ink } else { paper };
                canvas.put_pixel(sx as u32, sy as u32, image::Rgba(c));
            }
        }
    }
}

/// Door x in canvas pixels.
fn door_x(w: u32) -> f32 {
    w as f32 * DOOR_X_FRAC
}

/// Sprite frame for desk `i`, rotated off the global `(tick/4)%4` bucket.
///
/// All six desks used to sample the one global bucket, so the whole room typed
/// on the same keystroke and celebrated on the same sparkle — which reads as a
/// looping GIF rather than a room. The offset is a pure function of the desk
/// index, so it adds **no** fingerprint input (the global bucket is already
/// hashed) and **no** cache keys (the frame domain is still 0..4).
fn desk_frame(frame: u8, desk: usize) -> u8 {
    (frame.wrapping_add(desk as u8)) % 4
}

/// Whether the office door stands open this frame.
///
/// FINGERPRINT NOTE: a pure function of inputs
/// [`GameModeState::visual_fingerprint`] already hashes — every desk's
/// occupancy and phase, plus, for SpawnWalk/ExitDoor, its `anim_t` quantized to
/// `(anim_t * 20.0) as u8`. Both thresholds are bucket-aligned (`< 0.25` is
/// bucket `< 5`, `>= 0.75` is bucket `>= 15`), so two `anim_t` values that
/// share a hash bucket can never disagree about the door. That is what makes
/// the door free: no new fingerprint input, and both phases already recompose
/// every tick.
/// Whether the MCP rack's LEDs chase this frame (RC16 §4 #5).
///
/// The signal is **real tool-call traffic**: a seated desk's `tool_calls`
/// counter going up arms [`GameModeState::rack_active_until`] for
/// `RACK_BURST`, and the rack is lit for exactly that window. RC16 §3 step 2
/// shipped a placeholder here — "any desk typing" — which lit the rack for the
/// whole life of every subagent whether or not a single tool was ever called.
///
/// FINGERPRINT / WAKEUP contract now lives with the state that owns it: see
/// [`GameModeState::rack_burst_active`]. In short — the bool is hashed (it is
/// new state, not a re-derivation of hashed inputs), it costs two recomposes
/// per burst, and the only wakeups it can add are the tail of a burst whose
/// desks all retired inside the window.
pub(super) fn rack_is_active(state: &GameModeState) -> bool {
    state.rack_burst_active()
}

/// Cell rect covering the composed MCP rack — its hover hit box.
///
/// Exact analogue of [`supervisor_hit_rect`]: derived from the same
/// [`RACK_ANCHOR`] centre and `RACK_COVER_*_FRAC` footprint the compose pass
/// places (and [`rack_scale`] fits) the sprite with, so the box tracks the
/// baked chassis rather than guessing at it. Both fractions are of the compose
/// canvas, and the halfblock doubling cancels in a fraction, so the identical
/// fractions apply to the cell-space stage rect.
///
/// The caller publishes this **only when the pixel office actually painted**
/// (see the TIER NOTE on [`RACK_ANCHOR`]); everywhere else `last_mcp_rack`
/// stays a zero-size `Rect`, which never hit-tests.
pub(super) fn rack_hit_rect(stage: ratatui::layout::Rect) -> ratatui::layout::Rect {
    if stage.width == 0 || stage.height == 0 {
        return ratatui::layout::Rect::default();
    }
    let (x, y, cover_w, cover_h) =
        rack_cover_px(u32::from(stage.width), u32::from(stage.height));
    let cover_w = cover_w.round().max(1.0);
    let cover_h = cover_h.round().max(1.0);
    let x = x.max(0.0) as u16;
    let y = y.max(0.0) as u16;
    ratatui::layout::Rect {
        x: stage.x.saturating_add(x),
        y: stage.y.saturating_add(y),
        width: (cover_w as u16).min(stage.width.saturating_sub(x)),
        height: (cover_h as u16).min(stage.height.saturating_sub(y)),
    }
}

fn door_is_open(state: &GameModeState) -> bool {
    state.desks.iter().any(|d| {
        d.is_occupied()
            && match d.phase {
                ActorPhase::SpawnWalk => d.anim_t < DOOR_OPEN_ENTER_T,
                ActorPhase::ExitDoor => d.anim_t >= DOOR_OPEN_EXIT_T,
                _ => false,
            }
    })
}

/// Walking-actor centre (canvas pixels) for `phase` at `anim_t`.
///
/// `cx`/`cy` are the actor's own desk anchor:
/// - `SpawnWalk` slides in from the door at desk height,
/// - `WalkToBoss` crosses desk → supervisor,
/// - `Handoff` stands on the rug (the *walker* does not move with `anim_t`; the
///   papers FX blitted over it does — see [`paint_fx_handoff_papers`]),
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
    let sc = desk_scale(w).max(1);
    let walk_sc = (((w as f32 * 0.05) / 14.0).max(1.0).round().min(5.0) as u32).max(1);
    let prop_sc = prop_scale(w);
    let sup_sc = (((w as f32 * 0.072) / 26.0).max(1.0).round().min(5.0) as u32).max(1);
    let rack_sc = rack_scale(w, h);
    // RC16 P8: retire sprites rasterised for a previous stage size before this
    // pass touches the cache, so a resize cannot push the live set over the cap.
    sprite_cache_begin_pass([sc, walk_sc, prop_sc, sup_sc, rack_sc]);

    // Ambient props (door / rack / plants / coffee) near room edges — cached
    // sprites. The door goes down first so the plant that shares its column
    // overlaps it at large prop scales instead of being erased by it; the rack
    // follows so the mug keeps its existing z-order in front of everything.
    {
        let door = cached_door(door_is_open(state), prop_sc);
        blit(
            canvas,
            door.as_ref(),
            door_x(w) as i32 - door.width() as i32 / 2,
            (h as f32 * DOOR_Y_FRAC) as i32,
        );
        // No floor stamp before the rack, unlike the supervisor rug and
        // `clear_desk_area`: [`RACK_ANCHOR`] is the mockup's own wall-standing
        // rack, so a carpet patch would run floor tiles up the wall, and
        // [`rack_scale`] keeps the sprite inside that footprint so there is no
        // stray baked furniture to erase in the first place.
        let rack = cached_rack(rack_is_active(state), frame, rack_sc);
        blit(
            canvas,
            rack.as_ref(),
            (w as f32 * RACK_ANCHOR.0) as i32 - rack.width() as i32 / 2,
            (h as f32 * RACK_ANCHOR.1) as i32 - rack.height() as i32 / 2,
        );
        let plant = cached_plant(prop_sc);
        let coffee = cached_coffee(prop_sc);
        blit(
            canvas,
            plant.as_ref(),
            (w as f32 * PLANT_L_ANCHOR.0) as i32,
            (h as f32 * PLANT_L_ANCHOR.1) as i32,
        );
        blit(
            canvas,
            plant.as_ref(),
            (w as f32 * PLANT_R_ANCHOR.0) as i32,
            (h as f32 * PLANT_R_ANCHOR.1) as i32,
        );
        blit(
            canvas,
            coffee.as_ref(),
            (w as f32 * COFFEE_ANCHOR.0) as i32,
            (h as f32 * COFFEE_ANCHOR.1) as i32,
        );
    }

    // Supervisor
    {
        let (sx, sy) = SUPERVISOR_ANCHOR;
        let cx = (sx * w as f32) as i32;
        let cy = (sy * h as f32) as i32;
        let cover_w = (w as f32 * SUPERVISOR_COVER_W_FRAC) as i32;
        let cover_h = (h as f32 * SUPERVISOR_COVER_H_FRAC) as i32;
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
        let spr = cached_supervisor(phase, sup_frame, sup_sc);
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
        // Break the six-desk lockstep (see `desk_frame`).
        let frame = desk_frame(frame, i);

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
                if matches!(desk.phase, ActorPhase::Handoff) {
                    paint_fx_handoff_papers(canvas, x as i32, y as i32, desk.anim_t, w);
                }
            }
            ActorPhase::Celebrate => {
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                // Pose off `anim_t`, not the sprite bucket — see
                // [`celebrate_pose_frame`] for why the bucket is too coarse here.
                let spr = cached_dev_celebrate(
                    desk.skin,
                    celebrate_pose_frame(desk.anim_t),
                    sc.max(1),
                );
                blit(
            canvas,
            spr.as_ref(),
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
                paint_fx_confetti(canvas, cx, cy, desk.anim_t, w, h);
            }
            ActorPhase::FailBeat => {
                // No FX pass: the red alert rectangle this used to paint sat
                // exactly on the monitor and hid the error screen the pose now
                // carries. The blinking error bar and its red bezel spill *are*
                // the beat (see `sprites_pixel::sprite_developer_fail`).
                clear_desk_area(canvas, bg_scaled, cx, cy, w, h);
                let spr = cached_dev_fail(desk.skin, frame, sc.max(1));
                blit(
            canvas,
            spr.as_ref(),
                    cx - spr.width() as i32 / 2,
                    cy - spr.height() as i32 / 2,
                );
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

    /// Cheap synthetic cache entry — the eviction tests only care about keys.
    fn junk(i: u64, scale: u32) -> Arc<RgbaImage> {
        cache_get_or_insert((0xFFu64 << 56) | i, scale, || RgbaImage::new(1, 1))
    }

    /// P8(a): frames that render identical art must share one entry. The walk
    /// sprite only has two limb poses, so frame 0 and frame 2 are the same
    /// The supervisor hover box must sit on the sprite the compose pass draws,
    /// not next to it: same centre anchor, same footprint fractions, always
    /// inside the stage it was derived from.
    #[test]
    fn supervisor_hit_rect_tracks_the_composed_sprite() {
        use ratatui::layout::Rect;
        for stage in [
            Rect::new(0, 0, 100, 24),
            Rect::new(3, 7, 160, 40),
            Rect::new(0, 0, 72, 18),
        ] {
            let r = supervisor_hit_rect(stage);
            assert!(r.width > 0 && r.height > 0, "{stage:?} → empty hit rect");
            assert!(
                r.x >= stage.x
                    && r.y >= stage.y
                    && r.x + r.width <= stage.x + stage.width
                    && r.y + r.height <= stage.y + stage.height,
                "{r:?} escaped {stage:?}"
            );
            // Centre within one cell of the anchor the sprite is blitted at.
            let cx = f32::from(stage.x) + f32::from(stage.width) * SUPERVISOR_ANCHOR.0;
            let cy = f32::from(stage.y) + f32::from(stage.height) * SUPERVISOR_ANCHOR.1;
            let rcx = f32::from(r.x) + f32::from(r.width) / 2.0;
            let rcy = f32::from(r.y) + f32::from(r.height) / 2.0;
            assert!((rcx - cx).abs() <= 1.0, "{r:?} off-centre in x for {stage:?}");
            assert!((rcy - cy).abs() <= 1.0, "{r:?} off-centre in y for {stage:?}");
        }
        assert_eq!(
            supervisor_hit_rect(Rect::new(0, 0, 0, 0)),
            Rect::default(),
            "degenerate stage must not produce a hover target"
        );
    }

    /// Same contract for the rack box (RC16 §3 step 3): it must cover the baked
    /// chassis the sprite is fitted into, stay inside the stage at every office
    /// geometry, and collapse to nothing on a degenerate stage — a zero-size
    /// `Rect` is how `render_game_mode` says "no rack painted here".
    #[test]
    fn rack_hit_rect_tracks_the_composed_rack() {
        use ratatui::layout::Rect;
        for stage in [
            Rect::new(0, 0, 100, 24),
            Rect::new(3, 7, 160, 40),
            Rect::new(0, 0, 72, 18),
        ] {
            let r = rack_hit_rect(stage);
            assert!(r.width > 0 && r.height > 0, "{stage:?} → empty hit rect");
            assert!(
                r.x >= stage.x
                    && r.y >= stage.y
                    && r.x + r.width <= stage.x + stage.width
                    && r.y + r.height <= stage.y + stage.height,
                "{r:?} escaped {stage:?}"
            );
            let cx = f32::from(stage.x) + f32::from(stage.width) * RACK_ANCHOR.0;
            let cy = f32::from(stage.y) + f32::from(stage.height) * RACK_ANCHOR.1;
            let rcx = f32::from(r.x) + f32::from(r.width) / 2.0;
            let rcy = f32::from(r.y) + f32::from(r.height) / 2.0;
            assert!((rcx - cx).abs() <= 1.0, "{r:?} off-centre in x for {stage:?}");
            assert!((rcy - cy).abs() <= 1.0, "{r:?} off-centre in y for {stage:?}");
        }
        assert_eq!(
            rack_hit_rect(Rect::new(0, 0, 0, 0)),
            Rect::default(),
            "degenerate stage must not produce a hover target"
        );
        // The two boxes describe different props and must never overlap, or the
        // supervisor (tested first in `hit_test`) would swallow the rack.
        let stage = Rect::new(0, 0, 120, 30);
        let sup = supervisor_hit_rect(stage);
        let rack = rack_hit_rect(stage);
        assert!(
            sup.x + sup.width <= rack.x,
            "supervisor {sup:?} overlaps rack {rack:?}"
        );
    }

    /// picture — keying on the raw frame stored it twice.
    #[test]
    fn equivalent_frames_share_one_cache_entry() {
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);

        let walk0 = cached_walk(0, 0, false, 1);
        let walk2 = cached_walk(0, 2, false, 1);
        assert_eq!(sprite_cache_len(), 1, "walk 0/2 are the same pose");
        assert!(Arc::ptr_eq(&walk0, &walk2));
        let walk1 = cached_walk(0, 1, false, 1);
        assert_eq!(sprite_cache_len(), 2, "walk 1 is the other pose");
        assert!(!Arc::ptr_eq(&walk0, &walk1));

        // Idle developers only alternate the thought bubble at `frame % 4 < 2`.
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        let idle: Vec<_> = (0..4u8)
            .map(|f| cached_dev_at_desk(0, false, f, 1))
            .collect();
        assert_eq!(sprite_cache_len(), 2, "idle dev has 2 poses, not 4");
        assert!(Arc::ptr_eq(&idle[0], &idle[1]));
        assert!(Arc::ptr_eq(&idle[2], &idle[3]));

        // Idle/waiting supervisors only alternate the coffee steam.
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        for f in 0..4u8 {
            cached_supervisor(0, f, 1);
        }
        assert_eq!(sprite_cache_len(), 2, "idle supervisor has 2 steam frames");

        // Typing developers really do animate on all four frames.
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        for f in 0..4u8 {
            cached_dev_at_desk(0, true, f, 1);
        }
        assert_eq!(sprite_cache_len(), 4, "typing dev must not collapse");
    }

    /// P8(b): eviction must not destroy the working set. The old cache called
    /// `clear()` at 128 entries, so a sprite the current frame was still
    /// blitting vanished mid-pass and had to be rebuilt immediately.
    #[test]
    fn overflow_keeps_sprites_the_current_pass_is_using() {
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        let keep = cached_empty_desk(1);
        for i in 0..(SPRITE_CACHE_CAP as u64 * 2) {
            junk(i, 1);
            assert!(
                Arc::ptr_eq(&keep, &cached_empty_desk(1)),
                "in-use sprite was evicted after {i} overflow inserts"
            );
        }
    }

    /// P8(c): a resize retires the previous stage's scale wholesale.
    #[test]
    fn scale_change_drops_stale_scale_entries() {
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        let small = cached_empty_desk(1);
        cached_plant(1);
        assert_eq!(sprite_cache_len(), 2);

        sprite_cache_begin_pass([2, 2, 2, 2, 2]);
        assert_eq!(sprite_cache_len(), 0, "old scale is garbage after a resize");
        let big = cached_empty_desk(2);
        assert!(!Arc::ptr_eq(&small, &big));

        // A pass that still uses a scale keeps that scale's entries.
        sprite_cache_begin_pass([2, 3, 3, 3, 3]);
        assert!(sprite_cache_has_scale(2), "scale 2 is still live");
        cached_plant(3);
        sprite_cache_begin_pass([2, 3, 3, 3, 3]);
        assert_eq!(
            sprite_cache_len(),
            2,
            "an unchanged scale set evicts nothing"
        );
    }

    /// P8(d): churn across scales and keys stays bounded.
    #[test]
    fn cache_never_grows_past_the_cap() {
        sprite_cache_reset();
        for scale in 1..=5u32 {
            sprite_cache_begin_pass([scale, scale, scale, scale, scale]);
            for i in 0..(SPRITE_CACHE_CAP as u64 * 2) {
                junk(i, scale);
                assert!(sprite_cache_len() <= SPRITE_CACHE_CAP);
            }
        }
    }

    /// The number the cap is budgeted against: every sprite the office can ask
    /// for at one stage size. Raise [`SPRITE_CACHE_CAP`] before adding sprites
    /// that push this past half the cap.
    #[test]
    fn worst_case_working_set_per_scale() {
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        // frame is `(tick / 4) % 4`, so 0..4 is the whole reachable domain.
        for skin in 0..6u8 {
            for frame in 0..4u8 {
                cached_dev_at_desk(skin, true, frame, 1);
                cached_dev_at_desk(skin, false, frame, 1);
                cached_walk(skin, frame, true, 1);
                cached_walk(skin, frame, false, 1);
            }
        }
        for phase in 0..3u8 {
            for frame in 0..4u8 {
                cached_supervisor(phase, frame, 1);
            }
        }
        for skin in 0..6u8 {
            for frame in 0..4u8 {
                cached_dev_fail(skin, frame, 1);
                cached_dev_celebrate(skin, frame, 1);
            }
        }
        for open in [true, false] {
            cached_door(open, 1);
        }
        for active in [true, false] {
            for frame in 0..4u8 {
                cached_rack(active, frame, 1);
            }
        }
        cached_empty_desk(1);
        cached_plant(1);
        cached_coffee(1);
        // 36 seated devs + 12 debug-rage + 12 celebrate + 24 walkers
        // + 9 supervisors + 2 doors + 5 racks + 3 statics.
        assert_eq!(sprite_cache_len(), 103);
        assert!(
            sprite_cache_len() * 2 <= SPRITE_CACHE_CAP,
            "cap must hold two scale-sets across a resize"
        );
    }

    /// The free win: six desks must not sample the same sprite frame. The
    /// offset stays inside the 0..4 domain the cache keys are budgeted for, and
    /// is a pure rotation of the global bucket (so the already-hashed bucket
    /// still determines every desk's frame).
    #[test]
    fn desk_frames_are_offset_not_lockstep() {
        for bucket in 0..4u8 {
            let frames: Vec<u8> = (0..6).map(|i| desk_frame(bucket, i)).collect();
            assert!(frames.iter().all(|f| *f < 4), "{frames:?} left the domain");
            assert_eq!(
                frames.iter().collect::<std::collections::HashSet<_>>().len(),
                4,
                "bucket {bucket}: six desks must cover all four frames, got {frames:?}"
            );
            assert_ne!(frames[0], frames[1], "adjacent desks must differ");
        }
        // Pure rotation: advancing the global bucket advances every desk.
        for i in 0..6 {
            assert_eq!(desk_frame(1, i), desk_frame(0, i + 1));
        }
    }

    /// RC16 §4 #6: the door state must be a pure function of what
    /// `visual_fingerprint` already hashes, i.e. two `anim_t` values sharing the
    /// `(anim_t * 20.0) as u8` bucket must never disagree about the door. If
    /// they could, the office would show a door state the fingerprint cannot
    /// distinguish and the swing would stick.
    #[test]
    fn door_state_never_splits_an_anim_t_hash_bucket() {
        let mut s = GameModeState::new();
        assert!(!door_is_open(&s), "an empty room keeps the door shut");

        for phase in [ActorPhase::SpawnWalk, ActorPhase::ExitDoor] {
            s.desks[0].child_session_id = Some("d".into());
            s.desks[0].phase = phase;
            let mut per_bucket: std::collections::HashMap<u8, bool> =
                std::collections::HashMap::new();
            for step in 0..=1000u32 {
                let t = step as f32 / 1000.0;
                s.desks[0].anim_t = t;
                let bucket = (t * 20.0) as u8;
                let open = door_is_open(&s);
                if let Some(prev) = per_bucket.insert(bucket, open) {
                    assert_eq!(
                        prev, open,
                        "{phase:?}: bucket {bucket} disagrees about the door at t={t}"
                    );
                }
            }
            // ...and it must actually swing at the right end of the walk.
            s.desks[0].anim_t = 0.0;
            assert_eq!(door_is_open(&s), phase == ActorPhase::SpawnWalk);
            s.desks[0].anim_t = 1.0;
            assert_eq!(door_is_open(&s), phase == ActorPhase::ExitDoor);
        }

        // A cleared seat cannot hold the door open.
        s.desks[0].child_session_id = None;
        s.desks[0].phase = ActorPhase::SpawnWalk;
        s.desks[0].anim_t = 0.0;
        assert!(!door_is_open(&s));
    }

    /// The papers FX is procedural: it must stay inside the canvas whatever the
    /// walker anchor is, and must move as `anim_t` advances (which is why
    /// Handoff hashes `anim_t` again — see `state::phase_anim_t_is_visible`).
    #[test]
    fn handoff_papers_move_and_stay_in_bounds() {
        let mut canvas = RgbaImage::new(64, 48);
        // Off-canvas anchors must not panic or write out of bounds.
        for (cx, cy) in [(-40, -40), (200, 200), (0, 0)] {
            paint_fx_handoff_papers(&mut canvas, cx, cy, 0.5, 64);
        }

        let render = |t: f32| {
            let mut c = RgbaImage::new(64, 48);
            paint_fx_handoff_papers(&mut c, 32, 32, t, 64);
            c.into_raw()
        };
        let mid = render(0.45);
        assert!(mid.iter().any(|b| *b != 0), "mid-flight must draw sheets");
        assert_ne!(mid, render(0.75), "the arc must advance with anim_t");
        // Endpoints are empty: the sheets are in flight, never parked.
        assert!(render(0.0).iter().all(|b| *b == 0));
        assert!(render(1.0).iter().all(|b| *b == 0));
    }

    /// RC16 §4 #2: the celebrate pose is driven by `anim_t`, so it is bound by
    /// the same rule the door is — two `anim_t` values that share the
    /// fingerprint's `(anim_t * 20.0) as u8` bucket must never render different
    /// poses, or the office shows a frame the fingerprint cannot distinguish.
    #[test]
    fn celebrate_pose_never_splits_an_anim_t_hash_bucket() {
        let mut per_bucket: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();
        for step in 0..=1000u32 {
            let t = step as f32 / 1000.0;
            let bucket = (t * 20.0) as u8;
            let pose = celebrate_pose_frame(t);
            if let Some(prev) = per_bucket.insert(bucket, pose) {
                assert_eq!(prev, pose, "bucket {bucket} disagrees about the pose at t={t}");
            }
        }
        // ...and the pose really does pump across the phase, which is the whole
        // point: the `tick / 4` bucket advances ~once in its 400 ms, so a
        // bucket-driven pose was a 2-frame animation showing 1 frame. The six
        // samples below are the `anim_t` values the ~12 Hz `tick_anim` produces
        // over a 400 ms Celebrate.
        let poses: Vec<u8> = (0..=5)
            .map(|i| celebrate_pose_frame(i as f32 / 5.0))
            .collect();
        assert_eq!(poses, vec![0, 0, 1, 0, 1, 0], "got {poses:?}");
        let flips = poses.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(flips >= 3, "the arms must pump, got {poses:?}");
        assert_eq!(celebrate_pose_frame(1.0), 0, "the phase ends on frame 0");
        assert_eq!(celebrate_pose_frame(-1.0), 0, "clamped below");
    }

    /// The confetti is procedural: it must stay inside the canvas whatever the
    /// desk anchor is, must fall as `anim_t` advances, and must have cleared the
    /// frame by the time the phase ends (otherwise it freezes mid-air on the
    /// last composed frame).
    #[test]
    fn confetti_falls_and_clears_by_the_end_of_the_phase() {
        let mut canvas = RgbaImage::new(64, 48);
        for (cx, cy) in [(-40, -40), (200, 200), (0, 0)] {
            paint_fx_confetti(&mut canvas, cx, cy, 0.5, 64, 48);
        }

        let render = |t: f32| {
            let mut c = RgbaImage::new(64, 48);
            paint_fx_confetti(&mut c, 32, 32, t, 64, 48);
            c.into_raw()
        };
        let early = render(0.25);
        assert!(early.iter().any(|b| *b != 0), "the burst must draw pieces");
        assert_ne!(early, render(0.6), "the fall must advance with anim_t");
        assert!(render(0.0).iter().all(|b| *b == 0), "nothing before release");
        assert!(render(1.0).iter().all(|b| *b == 0), "all pieces must land");

        // Multi-colour by construction: a single-colour burst is just sparkles.
        let mut c = RgbaImage::new(64, 48);
        paint_fx_confetti(&mut c, 32, 32, 0.5, 64, 48);
        let colors: std::collections::HashSet<[u8; 4]> =
            c.pixels().map(|p| p.0).filter(|p| p[3] != 0).collect();
        assert!(colors.len() >= 3, "confetti must be multi-colour, got {colors:?}");
    }

    /// Canvas sizes the pixel office really paints at, one per tier plus the
    /// awkward aspect ratios: `(cell_w, cell_h)` → `(canvas_w, canvas_h)`.
    fn tier_canvases() -> Vec<(u32, u32)> {
        [
            (72u16, 18u16),  // Normal, smallest stage the pixel path accepts
            (80, 24),        // Normal
            (100, 30),       // Normal
            (120, 34),       // Comfort
            (160, 44),       // Wide
            (200, 60),       // Wide, adaptive scale 2
            (240, 20),       // very wide and short
            (72, 50),        // narrow and tall
        ]
        .iter()
        .map(|(cw, ch)| {
            let s = crate::views::game_mode::sprites_pixel::effective_pixel_scale(*cw, *ch).max(1);
            (u32::from(*cw) * s, u32::from(*ch) * 2 * s)
        })
        .collect()
    }

    fn rects_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    /// RC16 §3 step 2: the rack lands on the mockup's own "MCP SERVER" rack and
    /// must touch nothing else — not a desk (whose clear-stamp would erase it),
    /// not the supervisor rug, not the door column, not a plant or the mug.
    /// Checked against the *cover* footprint, which is both what [`rack_scale`]
    /// fits into and the hit box the rack tooltip will hang off.
    #[test]
    fn rack_footprint_clears_every_desk_and_prop() {
        let plant = sprite_plant().dimensions();
        let coffee = sprite_coffee().dimensions();
        let door = sprite_door(false).dimensions();
        let sup = sprite_supervisor(0, 0).dimensions();
        let desk = sprite_empty_desk().dimensions();

        for (w, h) in tier_canvases() {
            let (fw, fh) = (w as f32, h as f32);
            let sc = desk_scale(w).max(1) as f32;
            let prop_sc = prop_scale(w) as f32;
            let sup_sc = (((fw * 0.072) / 26.0).max(1.0).round().min(5.0) as u32).max(1) as f32;
            let (rx, ry, rw, rh) = rack_cover_px(w, h);
            assert!(
                rx >= 0.0 && rx + rw <= fw && ry >= 0.0 && ry + rh <= fh,
                "{w}×{h}: rack cover ({rx}, {ry}, {rw}, {rh}) left the canvas"
            );
            // Checked region: the footprint (= the hit box) unioned with the
            // sprite actually blitted, which on a very short canvas is taller
            // than the footprint because `rack_scale` cannot go below 1.
            let s = rack_scale(w, h) as f32;
            let (sw, sh) = (18.0 * s, 28.0 * s);
            let sprite = (
                fw * RACK_ANCHOR.0 - sw / 2.0,
                fh * RACK_ANCHOR.1 - sh / 2.0,
                sw,
                sh,
            );
            let x0 = rx.min(sprite.0);
            let y0 = ry.min(sprite.1);
            let cover = (
                x0,
                y0,
                (rx + rw).max(sprite.0 + sprite.2) - x0,
                (ry + rh).max(sprite.1 + sprite.3) - y0,
            );

            let mut others: Vec<(String, (f32, f32, f32, f32))> = vec![
                (
                    "left plant".to_string(),
                    (
                        fw * PLANT_L_ANCHOR.0,
                        fh * PLANT_L_ANCHOR.1,
                        plant.0 as f32 * prop_sc,
                        plant.1 as f32 * prop_sc,
                    ),
                ),
                (
                    "right plant".to_string(),
                    (
                        fw * PLANT_R_ANCHOR.0,
                        fh * PLANT_R_ANCHOR.1,
                        plant.0 as f32 * prop_sc,
                        plant.1 as f32 * prop_sc,
                    ),
                ),
                (
                    "coffee".to_string(),
                    (
                        fw * COFFEE_ANCHOR.0,
                        fh * COFFEE_ANCHOR.1,
                        coffee.0 as f32 * prop_sc,
                        coffee.1 as f32 * prop_sc,
                    ),
                ),
                (
                    "door".to_string(),
                    (
                        door_x(w) - door.0 as f32 * prop_sc / 2.0,
                        fh * DOOR_Y_FRAC,
                        door.0 as f32 * prop_sc,
                        door.1 as f32 * prop_sc,
                    ),
                ),
            ];
            // Supervisor: the union of the rug/clear footprint and the sprite.
            let (sup_hw, sup_hh) = (
                (fw * SUPERVISOR_COVER_W_FRAC / 2.0).max(sup.0 as f32 * sup_sc / 2.0),
                (fh * SUPERVISOR_COVER_H_FRAC).max(sup.1 as f32 * sup_sc / 2.0),
            );
            others.push((
                "supervisor".to_string(),
                (
                    fw * SUPERVISOR_ANCHOR.0 - sup_hw,
                    fh * SUPERVISOR_ANCHOR.1 - sup_hh,
                    sup_hw * 2.0,
                    sup_hh * 2.0,
                ),
            ));
            // Desks: the union of `clear_desk_area` and the seated sprite.
            for (i, (ax, ay)) in DESK_ANCHORS.iter().enumerate() {
                let hw = (fw * 0.15 / 2.0).max(desk.0 as f32 * sc / 2.0);
                let hh = (fh * 0.17 / 2.0).max(desk.1 as f32 * sc / 2.0);
                others.push((
                    format!("desk {i}"),
                    (fw * ax - hw, fh * ay - hh, hw * 2.0, hh * 2.0),
                ));
            }

            for (name, other) in others {
                assert!(
                    !rects_overlap(cover, other),
                    "{w}×{h}: rack {cover:?} collides with {name} {other:?}"
                );
            }

            // ...and the fit rule holds: the sprite never grows past the baked
            // footprint. The one exception is the `max(1)` clamp on a canvas too
            // short to hold 28 rows at all, and even there the *width* — the
            // axis every neighbour is separated on — still fits.
            assert!(
                sw <= rw,
                "{w}×{h}: rack sprite is {sw} wide, footprint is {rw}"
            );
            if rh >= 28.0 {
                assert!(
                    sh <= rh,
                    "{w}×{h}: rack sprite is {sh} tall, footprint is {rh}"
                );
            } else {
                assert_eq!(s, 1.0, "{w}×{h}: only the min-scale clamp may overflow");
            }
        }
    }

    /// The rack must actually be drawn, at every tier — the mockup already bakes
    /// a rack there, so "composed" means *the composed frame differs from the
    /// background inside the rack footprint*. It must also react: idle art and
    /// active art differ, and consecutive active frames differ (which is the
    /// LED chase, and proves `cached_rack` is not collapsing them).
    #[test]
    fn rack_is_composed_and_animates_at_every_office_tier() {
        let full = load_office_background().expect("bg");
        let differing = |a: &RgbaImage, b: &RgbaImage, r: (f32, f32, f32, f32)| {
            let mut n = 0usize;
            for y in (r.1 as u32)..((r.1 + r.3) as u32).min(a.height()) {
                for x in (r.0 as u32)..((r.0 + r.2) as u32).min(a.width()) {
                    if a.get_pixel(x, y) != b.get_pixel(x, y) {
                        n += 1;
                    }
                }
            }
            n
        };

        for (cw, ch) in [(80u16, 24u16), (120, 34), (200, 60)] {
            let scale =
                crate::views::game_mode::sprites_pixel::effective_pixel_scale(cw, ch).max(1);
            let bg = scale_bg_to_cells_with_scale(&full, cw, ch, scale);
            let (w, h) = bg.dimensions();
            let cover = rack_cover_px(w, h);

            let idle = GameModeState::new();
            assert!(!rack_is_active(&idle));
            let idle_frame = compose_cell_frame(&bg, &idle, 0);
            assert!(
                differing(&bg, &idle_frame, cover) > 0,
                "{cw}×{ch}: nothing was composed over the baked rack"
            );

            let mut busy = GameModeState::new();
            busy.desks[0].child_session_id = Some("a".into());
            busy.desks[0].phase = ActorPhase::AtDeskWorking;
            // RC16 §4 #5: the rack answers to real tool calls, not to a desk
            // merely existing — arm the burst the way a sync would.
            busy.rack_active_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
            assert!(rack_is_active(&busy));
            let busy0 = compose_cell_frame(&bg, &busy, 0);
            assert!(
                differing(&idle_frame, &busy0, cover) > 0,
                "{cw}×{ch}: the rack must light up when work starts"
            );
            // `frame` is `(tick / 4) % 4`, so tick 4 is the next LED step.
            let busy1 = compose_cell_frame(&bg, &busy, 4);
            assert!(
                differing(&busy0, &busy1, cover) > 0,
                "{cw}×{ch}: the LED chase must advance with the tick bucket"
            );
        }
    }

    /// The cache must store one entry for the whole idle rack and one per
    /// reachable active frame — the same contract every animated sprite has.
    #[test]
    fn rack_cache_collapses_idle_frames_but_not_the_chase() {
        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        let idle: Vec<_> = (0..4u8).map(|f| cached_rack(false, f, 1)).collect();
        assert_eq!(sprite_cache_len(), 1, "an idle rack has one frame");
        assert!(idle.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));

        sprite_cache_reset();
        sprite_cache_begin_pass([1, 1, 1, 1, 1]);
        for f in 0..4u8 {
            cached_rack(true, f, 1);
        }
        assert_eq!(sprite_cache_len(), 4, "the chase must not collapse");
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

