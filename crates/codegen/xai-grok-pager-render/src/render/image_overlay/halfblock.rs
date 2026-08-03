//! Truecolor half-block image raster for terminals without Kitty/iTerm2.
//!
//! Each terminal cell maps to two vertical pixels via the upper-half-block
//! glyph `▀` (U+2580): foreground = top sample, background = bottom sample.
//! This path paints into the ratatui [`Buffer`] only — no APC/OSC escapes —
//! so it survives ConPTY on Windows and works in VS Code / Windows Terminal.

use image::imageops::FilterType;
use image::{Rgba, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// Upper half block — fg paints the top half of the cell, bg the bottom.
const HALF_BLOCK: &str = "\u{2580}";

/// Decode `image_bytes` and paint a half-block approximation into `area`.
///
/// Returns `true` when at least one cell was written. Returns `false` if
/// the area is empty or the bytes cannot be decoded as an image.
pub fn paint_halfblock_image(buf: &mut Buffer, area: Rect, image_bytes: &[u8]) -> bool {
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let img = match image::load_from_memory(image_bytes) {
        Ok(img) => img.to_rgba8(),
        Err(_) => return false,
    };
    paint_halfblock_rgba(buf, area, &img)
}

/// Paint an already-decoded RGBA image into `area` as half-blocks.
///
/// If `img` is exactly `area.width × area.height*2`, samples are used
/// directly (no resize) — preferred hot path for Game Mode.
pub fn paint_halfblock_rgba(buf: &mut Buffer, area: Rect, img: &RgbaImage) -> bool {
    if area.width == 0 || area.height == 0 {
        return false;
    }

    let target_w = u32::from(area.width);
    let target_h = u32::from(area.height).saturating_mul(2).max(1);

    let (src_w, src_h) = img.dimensions();
    let use_direct = src_w == target_w && src_h == target_h;

    // Only allocate/resize when dimensions don't match.
    // Integer-multiple high-res frames (Game Mode PIXEL_SCALE) use Nearest so
    // SNES pixel art stays sharp instead of Triangle-blurring.
    let owned;
    let pixels: &RgbaImage = if use_direct {
        img
    } else {
        let integer_scale = src_w % target_w == 0
            && src_h % target_h == 0
            && src_w / target_w == src_h / target_h
            && src_w / target_w >= 2;
        let filter = if integer_scale {
            FilterType::Nearest
        } else {
            FilterType::Triangle
        };
        owned = image::imageops::resize(img, target_w, target_h, filter);
        &owned
    };

    for row in 0..area.height {
        for col in 0..area.width {
            let x = u32::from(col);
            let y_top = u32::from(row).saturating_mul(2);
            let y_bot = y_top.saturating_add(1);
            let top = *pixels.get_pixel(x, y_top.min(target_h.saturating_sub(1)));
            let bot = if y_bot < target_h {
                *pixels.get_pixel(x, y_bot)
            } else {
                top
            };
            let cell_x = area.x.saturating_add(col);
            let cell_y = area.y.saturating_add(row);
            if let Some(cell) = buf.cell_mut((cell_x, cell_y)) {
                cell.set_symbol(HALF_BLOCK);
                cell.set_style(
                    Style::default()
                        .fg(rgba_to_color(&top))
                        .bg(rgba_to_color(&bot)),
                );
            }
        }
    }
    true
}

/// Composite semi-transparent pixels onto black so RGB cells stay opaque.
fn rgba_to_color(p: &Rgba<u8>) -> Color {
    let a = u32::from(p[3]);
    if a >= 255 {
        return Color::Rgb(p[0], p[1], p[2]);
    }
    if a == 0 {
        return Color::Rgb(0, 0, 0);
    }
    Color::Rgb(
        ((u32::from(p[0]) * a) / 255) as u8,
        ((u32::from(p[1]) * a) / 255) as u8,
        ((u32::from(p[2]) * a) / 255) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba, RgbaImage};

    fn solid_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let img: RgbaImage = ImageBuffer::from_pixel(width, height, Rgba(color));
        let mut buf = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut buf),
            image::ImageFormat::Png,
        )
        .expect("encode png");
        buf
    }

    #[test]
    fn empty_area_returns_false() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        let png = solid_png(4, 4, [255, 0, 0, 255]);
        assert!(!paint_halfblock_image(
            &mut buf,
            Rect::new(0, 0, 0, 5),
            &png
        ));
        assert!(!paint_halfblock_image(
            &mut buf,
            Rect::new(0, 0, 5, 0),
            &png
        ));
    }

    #[test]
    fn corrupt_bytes_return_false() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
        assert!(!paint_halfblock_image(
            &mut buf,
            Rect::new(0, 0, 8, 4),
            b"not-an-image"
        ));
    }

    #[test]
    fn paints_half_block_glyph_with_solid_color() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        let png = solid_png(8, 8, [200, 50, 25, 255]);
        assert!(paint_halfblock_image(
            &mut buf,
            Rect::new(0, 0, 4, 2),
            &png
        ));
        let cell = buf.cell((0, 0)).expect("cell");
        assert_eq!(cell.symbol(), HALF_BLOCK);
        assert_eq!(cell.fg, Color::Rgb(200, 50, 25));
        assert_eq!(cell.bg, Color::Rgb(200, 50, 25));
    }

    #[test]
    fn direct_rgba_skips_resize_when_sized() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        // 2 cols × 2 rows of halfblock samples
        let img: RgbaImage = ImageBuffer::from_pixel(2, 2, Rgba([10, 20, 30, 255]));
        assert!(paint_halfblock_rgba(&mut buf, Rect::new(0, 0, 2, 1), &img));
        assert_eq!(buf.cell((0, 0)).unwrap().fg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn transparent_pixels_composite_toward_black() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        // 50% red over black => ~half intensity.
        let png = solid_png(2, 2, [255, 0, 0, 128]);
        assert!(paint_halfblock_image(
            &mut buf,
            Rect::new(0, 0, 1, 1),
            &png
        ));
        let cell = buf.cell((0, 0)).expect("cell");
        match cell.fg {
            Color::Rgb(r, g, b) => {
                assert!(r > 100 && r < 160, "expected ~127 red, got {r}");
                assert_eq!(g, 0);
                assert_eq!(b, 0);
            }
            other => panic!("expected Rgb, got {other:?}"),
        }
    }
}
