//! Responsive Game Mode layout: tiers, letterbox stage, desk rects.

use ratatui::layout::{Constraint, Layout, Rect};

/// Visual density tier driven by stage size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameTier {
    /// Card grid only — no free-walk office.
    Compact,
    /// Full office, small sprites.
    Normal,
    /// Mockup-like spacing, standard sprites.
    Comfort,
    /// Extra padding; still 6 desks.
    Wide,
}

impl GameTier {
    pub fn uses_office_art(self) -> bool {
        !matches!(self, Self::Compact)
    }

    pub fn sprite_set(self) -> SpriteSet {
        match self {
            Self::Compact => SpriteSet::Small,
            Self::Normal => SpriteSet::Small,
            Self::Comfort | Self::Wide => SpriteSet::Medium,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpriteSet {
    Small,
    Medium,
}

/// Computed layout for one frame.
#[derive(Debug, Clone)]
pub struct GameLayout {
    pub tier: GameTier,
    /// Full area passed to game mode (above composer).
    pub stage: Rect,
    /// Centered content rect (letterboxed inside stage).
    pub content: Rect,
    pub wall: Rect,
    pub supervisor: Rect,
    pub handoff: Rect,
    /// Six desk rects in row-major order (0..2 top, 3..5 bottom).
    pub desks: [Rect; 6],
    pub door: Rect,
    pub status_strip: Rect,
    /// True when letterbox margins exist.
    pub has_margins: bool,
}

const MIN_STAGE_W: u16 = 72;
const MIN_STAGE_H: u16 = 18;
const COMFORT_W: u16 = 120;
const COMFORT_H: u16 = 28;
const WIDE_W: u16 = 160;
const WIDE_H: u16 = 36;

const STATUS_STRIP_H: u16 = 1;
const WALL_H_COMPACT: u16 = 1;
const WALL_H_OFFICE: u16 = 3;

/// Pick tier from stage size (more constrained dimension wins).
pub fn game_tier(stage: Rect) -> GameTier {
    let w = stage.width;
    let h = stage.height;
    if w < MIN_STAGE_W || h < MIN_STAGE_H {
        GameTier::Compact
    } else if w < COMFORT_W || h < COMFORT_H {
        GameTier::Normal
    } else if w < WIDE_W || h < WIDE_H {
        GameTier::Comfort
    } else {
        GameTier::Wide
    }
}

/// Compute full layout. `area` is the region above the agent composer.
pub fn compute(area: Rect) -> GameLayout {
    if area.width == 0 || area.height == 0 {
        return empty_layout(area);
    }

    // Peel status strip from bottom of area.
    let strip_h = STATUS_STRIP_H.min(area.height);
    let stage_h = area.height.saturating_sub(strip_h);
    let stage = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: stage_h,
    };
    let status_strip = Rect {
        x: area.x,
        y: area.y.saturating_add(stage_h),
        width: area.width,
        height: strip_h,
    };

    let tier = game_tier(stage);
    let content = letterbox_content(stage, tier);
    let has_margins = content != stage;

    if !tier.uses_office_art() {
        return compact_layout(area, stage, content, status_strip, tier, has_margins);
    }

    office_layout(area, stage, content, status_strip, tier, has_margins)
}

fn empty_layout(area: Rect) -> GameLayout {
    GameLayout {
        tier: GameTier::Compact,
        stage: area,
        content: area,
        wall: area,
        supervisor: area,
        handoff: area,
        desks: [area; 6],
        door: area,
        status_strip: Rect::default(),
        has_margins: false,
    }
}

fn letterbox_content(stage: Rect, tier: GameTier) -> Rect {
    if !tier.uses_office_art() {
        return stage;
    }
    // Cap max content so Wide doesn't stretch sprites past design size.
    let (max_w, max_h) = match tier {
        GameTier::Compact => (stage.width, stage.height),
        GameTier::Normal => (118, 26),
        GameTier::Comfort => (158, 34),
        GameTier::Wide => (180, 42),
    };
    let content_w = stage.width.min(max_w);
    let content_h = stage.height.min(max_h);
    let x = stage.x + (stage.width.saturating_sub(content_w)) / 2;
    let y = stage.y + (stage.height.saturating_sub(content_h)) / 2;
    Rect {
        x,
        y,
        width: content_w,
        height: content_h,
    }
}

fn compact_layout(
    _area: Rect,
    stage: Rect,
    content: Rect,
    status_strip: Rect,
    tier: GameTier,
    has_margins: bool,
) -> GameLayout {
    let wall_h = WALL_H_COMPACT.min(content.height);
    let rest_h = content.height.saturating_sub(wall_h);
    let wall = Rect {
        x: content.x,
        y: content.y,
        width: content.width,
        height: wall_h,
    };
    let body = Rect {
        x: content.x,
        y: content.y.saturating_add(wall_h),
        width: content.width,
        height: rest_h,
    };

    // Top row: supervisor banner (1/4) + nothing special
    let sup_h = (body.height / 4).max(1).min(body.height);
    let desk_region_h = body.height.saturating_sub(sup_h);
    let supervisor = Rect {
        x: body.x,
        y: body.y,
        width: body.width,
        height: sup_h,
    };
    let desk_region = Rect {
        x: body.x,
        y: body.y.saturating_add(sup_h),
        width: body.width,
        height: desk_region_h,
    };
    let desks = split_desks(desk_region);
    let door = Rect::default();
    let handoff = supervisor;

    GameLayout {
        tier,
        stage,
        content,
        wall,
        supervisor,
        handoff,
        desks,
        door,
        status_strip,
        has_margins,
    }
}

fn office_layout(
    _area: Rect,
    stage: Rect,
    content: Rect,
    status_strip: Rect,
    tier: GameTier,
    has_margins: bool,
) -> GameLayout {
    let wall_h = WALL_H_OFFICE.min(content.height);
    let after_wall = content.height.saturating_sub(wall_h);
    // supervisor ~28%, handoff ~12%, desks rest
    let sup_h = ((after_wall as u32 * 28) / 100).max(4) as u16;
    let handoff_h = ((after_wall as u32 * 12) / 100).max(2) as u16;
    let desk_h = after_wall
        .saturating_sub(sup_h)
        .saturating_sub(handoff_h)
        .max(4);

    let wall = Rect {
        x: content.x,
        y: content.y,
        width: content.width,
        height: wall_h,
    };
    let supervisor = Rect {
        x: content.x,
        y: content.y.saturating_add(wall_h),
        width: content.width,
        height: sup_h.min(after_wall),
    };
    let handoff = Rect {
        x: content.x,
        y: supervisor.y.saturating_add(supervisor.height),
        width: content.width,
        height: handoff_h.min(content.height.saturating_sub(wall_h + supervisor.height)),
    };
    let desk_region = Rect {
        x: content.x,
        y: handoff.y.saturating_add(handoff.height),
        width: content.width.saturating_sub(6).max(1), // leave door column
        height: desk_h.min(
            content
                .height
                .saturating_sub(wall_h + supervisor.height + handoff.height),
        ),
    };
    let door = Rect {
        x: content.x.saturating_add(content.width.saturating_sub(6)),
        y: desk_region.y,
        width: 6.min(content.width),
        height: desk_region.height,
    };
    let desks = split_desks(desk_region);

    GameLayout {
        tier,
        stage,
        content,
        wall,
        supervisor,
        handoff,
        desks,
        door,
        status_strip,
        has_margins,
    }
}

fn split_desks(region: Rect) -> [Rect; 6] {
    if region.width == 0 || region.height == 0 {
        return [Rect::default(); 6];
    }
    let rows = Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(region);
    let mut out = [Rect::default(); 6];
    for (ri, row) in rows.iter().enumerate() {
        let cols = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .split(*row);
        for (ci, col) in cols.iter().enumerate() {
            out[ri * 3 + ci] = *col;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn tier_compact_below_minimum() {
        assert_eq!(game_tier(area(60, 20)), GameTier::Compact);
        assert_eq!(game_tier(area(80, 16)), GameTier::Compact);
    }

    #[test]
    fn tier_normal_comfort_wide() {
        assert_eq!(game_tier(area(100, 24)), GameTier::Normal);
        assert_eq!(game_tier(area(130, 30)), GameTier::Comfort);
        assert_eq!(game_tier(area(180, 40)), GameTier::Wide);
    }

    #[test]
    fn layout_desks_non_overlapping_and_inside_content() {
        for (w, h) in [(80, 24), (100, 30), (120, 32), (160, 40), (200, 50)] {
            let layout = compute(area(w, h));
            assert_eq!(layout.status_strip.height, 1);
            assert!(layout.stage.height + layout.status_strip.height <= h);
            for (i, d) in layout.desks.iter().enumerate() {
                if d.width == 0 || d.height == 0 {
                    continue;
                }
                assert!(
                    d.x >= layout.content.x,
                    "desk {i} x outside content at {w}x{h}"
                );
                assert!(
                    d.y >= layout.content.y,
                    "desk {i} y outside content at {w}x{h}"
                );
                for (j, other) in layout.desks.iter().enumerate() {
                    if i >= j || other.width == 0 {
                        continue;
                    }
                    let overlap = rects_overlap(*d, *other);
                    assert!(
                        !overlap,
                        "desks {i} and {j} overlap at {w}x{h}: {d:?} vs {other:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn compact_still_has_six_desk_slots() {
        let layout = compute(area(60, 22));
        assert_eq!(layout.tier, GameTier::Compact);
        assert_eq!(layout.desks.len(), 6);
    }

    #[test]
    fn zero_area_does_not_panic() {
        let layout = compute(Rect::default());
        assert_eq!(layout.tier, GameTier::Compact);
    }

    fn rects_overlap(a: Rect, b: Rect) -> bool {
        let ax2 = a.x.saturating_add(a.width);
        let ay2 = a.y.saturating_add(a.height);
        let bx2 = b.x.saturating_add(b.width);
        let by2 = b.y.saturating_add(b.height);
        a.x < bx2 && b.x < ax2 && a.y < by2 && b.y < ay2
    }
}
