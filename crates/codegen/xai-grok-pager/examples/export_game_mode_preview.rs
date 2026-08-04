//! Export a static Game Mode pixel frame for visual QA.
//!
//! ```text
//! cargo run -p xai-grok-pager --example export_game_mode_preview
//! ```
//!
//! Writes `target/game-mode-preview.png` (and prints the path).

use std::path::PathBuf;
use std::time::Duration;

use xai_grok_pager::views::game_mode::{
    ActorPhase, DeskAgentSnapshot, GameModeState, compose_cell_frame, encode_png,
    load_office_background, scale_bg_to_cells,
};

fn snap(
    id: &str,
    label: &str,
    ty: &str,
    running: bool,
    activity: &str,
) -> DeskAgentSnapshot {
    DeskAgentSnapshot {
        child_session_id: id.into(),
        label: label.into(),
        subagent_type: ty.into(),
        running,
        failed: false,
        elapsed: Duration::from_secs(42),
        tokens: 12_000,
        tool_calls: 5,
        activity: activity.into(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bg = load_office_background()?;
    // Comfort-ish terminal stage → high internal resolution.
    let cell_w = 120u16;
    let cell_h = 32u16;
    let bg_scaled = scale_bg_to_cells(&bg, cell_w, cell_h);

    let mut state = GameModeState::new();
    state.open = true;
    let agents = vec![
        snap("a", "explore", "explore", true, "Reading…"),
        snap("b", "plan", "plan", true, "Thinking"),
        snap("c", "test", "general", true, "cargo test"),
        snap("d", "review", "code-reviewer", true, "Reviewing"),
        snap("e", "fix", "general", true, "Editing"),
        snap("f", "docs", "general", true, "Writing"),
    ];
    state.sync_from_snapshots(
        &agents,
        true,
        xai_grok_pager::views::game_mode::GameTier::Comfort,
        false,
    );
    // Ensure all seated as working (not still in spawn walk).
    for d in &mut state.desks {
        if d.is_occupied() {
            d.phase = ActorPhase::AtDeskWorking;
            d.anim_t = 0.5;
        }
    }
    state.tick = 12;

    let frame = compose_cell_frame(&bg_scaled, &state, state.tick);
    let png = encode_png(&frame)?;
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/game-mode-preview.png");
    let out = out.canonicalize().unwrap_or(out);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Prefer workspace target/
    let out = PathBuf::from("target/game-mode-preview.png");
    std::fs::create_dir_all("target")?;
    std::fs::write(&out, &png)?;
    println!("wrote {} ({} bytes, {}x{})", out.display(), png.len(), frame.width(), frame.height());
    Ok(())
}
