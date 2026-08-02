//! Fast pixel Game Mode playground — cell-res compose + direct halfblocks.
//!
//! ```text
//! cargo run -p xai-grok-pager --bin game-mode-playground
//! ```

use std::io::{self, stdout};
use std::time::{Duration, Instant};

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use xai_grok_pager::views::game_mode::{
    ActorPhase, DeskAgentSnapshot, GameModeState, GameTier, compose_cell_frame, load_office_background,
    scale_bg_to_cells,
};
use xai_grok_pager_render::render::image_overlay::paint_halfblock_rgba;

struct Scenario {
    name: &'static str,
    agents: Vec<DeskAgentSnapshot>,
    supervisor_working: bool,
}

fn snap(
    id: &str,
    label: &str,
    ty: &str,
    running: bool,
    failed: bool,
    secs: u64,
    tokens: u64,
    tools: u32,
    activity: &str,
) -> DeskAgentSnapshot {
    DeskAgentSnapshot {
        child_session_id: id.into(),
        label: label.into(),
        subagent_type: ty.into(),
        running,
        failed,
        elapsed: Duration::from_secs(secs),
        tokens,
        tool_calls: tools,
        activity: activity.into(),
    }
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "Idle office (mockup only)",
            agents: vec![],
            supervisor_working: false,
        },
        Scenario {
            name: "3 working (badges only)",
            agents: vec![
                snap("a", "explore", "explore", true, false, 45, 12_400, 7, "Reading…"),
                snap("b", "plan", "plan", true, false, 120, 88_000, 14, "Thinking"),
                snap("c", "test", "general", true, false, 30, 4_200, 3, "cargo test"),
            ],
            supervisor_working: true,
        },
        Scenario {
            name: "Handoff walk",
            agents: vec![
                snap("d1", "done-job", "explore", false, false, 90, 9_000, 5, ""),
                snap("d2", "still-going", "plan", true, false, 40, 3_000, 2, "Writing"),
            ],
            supervisor_working: false,
        },
    ]
}

struct App {
    scenarios: Vec<Scenario>,
    active: usize,
    state: GameModeState,
    bg_full: image::RgbaImage,
    auto: bool,
    last_tick: Instant,
    status: String,
}

impl App {
    fn new() -> io::Result<Self> {
        let bg_full = load_office_background().map_err(io::Error::other)?;
        let mut app = Self {
            scenarios: scenarios(),
            active: 0,
            state: GameModeState::new(),
            bg_full,
            auto: true,
            last_tick: Instant::now(),
            status: String::new(),
        };
        app.state.open = true;
        app.apply_scenario();
        Ok(app)
    }

    fn apply_scenario(&mut self) {
        let name = self.scenarios[self.active].name;
        let agents = self.scenarios[self.active].agents.clone();
        let supervisor_working = self.scenarios[self.active].supervisor_working;
        self.state = GameModeState::new();
        self.state.open = true;
        let tier = GameTier::Comfort;
        let running: Vec<_> = agents.iter().filter(|a| a.running).cloned().collect();
        self.state
            .sync_from_snapshots(&running, supervisor_working, tier);
        self.state
            .sync_from_snapshots(&agents, supervisor_working, tier);
        if name.contains("Handoff") {
            for d in &mut self.state.desks {
                if d.child_session_id.as_deref() == Some("d1") {
                    d.phase = ActorPhase::Celebrate;
                    d.anim_t = 0.0;
                    d.phase_started = Instant::now();
                }
            }
            self.state.had_success = true;
        }
        self.state.tick = 0;
        self.status = format!("Scenario: {name}");
    }

    fn tick_once(&mut self) {
        let tier = GameTier::Comfort;
        self.state.tick_anim(tier);
        let agents = self.scenarios[self.active].agents.clone();
        let supervisor_working = self.scenarios[self.active].supervisor_working;
        self.state
            .sync_from_snapshots(&agents, supervisor_working, tier);
        self.last_tick = Instant::now();
    }
}

fn main() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new()?;

    let result = (|| -> io::Result<()> {
        loop {
            terminal.draw(|f| {
                let area = f.area();
                let chunks = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(8),
                    Constraint::Length(1),
                ])
                .split(area);

                f.render_widget(
                    Paragraph::new(format!(
                        "Game Mode FAST  {}  n/p scenario  Space pause  q quit",
                        app.status
                    ))
                    .style(
                        Style::default()
                            .fg(Color::Rgb(255, 220, 120))
                            .add_modifier(Modifier::BOLD),
                    ),
                    chunks[0],
                );

                let stage = chunks[1];
                let ready = app
                    .state
                    .ensure_pixel_frame(stage.width, stage.height);
                if ready {
                    if let Some(frame) = app.state.pixel_frame.as_ref() {
                        paint_halfblock_rgba(f.buffer_mut(), stage, frame);
                    }
                } else {
                    // Fallback: scale full bg once for this paint
                    let bg = scale_bg_to_cells(&app.bg_full, stage.width, stage.height);
                    let frame = compose_cell_frame(&bg, &app.state, app.state.tick);
                    paint_halfblock_rgba(f.buffer_mut(), stage, &frame);
                }

                f.render_widget(
                    Paragraph::new(format!(
                        "desks={} wall={} tick={}  [cell-res / no PNG]",
                        app.state.active_desk_count(),
                        app.state.wall.title(),
                        app.state.tick
                    ))
                    .style(Style::default().fg(Color::Rgb(160, 170, 180))),
                    chunks[2],
                );
            })?;

            if event::poll(Duration::from_millis(30))? {
                match event::read()? {
                    Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                break;
                            }
                            KeyCode::Char('n') => {
                                app.active = (app.active + 1) % app.scenarios.len();
                                app.apply_scenario();
                            }
                            KeyCode::Char('p') => {
                                app.active =
                                    (app.active + app.scenarios.len() - 1) % app.scenarios.len();
                                app.apply_scenario();
                            }
                            KeyCode::Char(' ') => app.auto = !app.auto,
                            KeyCode::Char('t') => app.tick_once(),
                            _ => {}
                        }
                    }
                    Event::Resize(_, _) => {
                        app.state.pixel_bg_scaled = None;
                        app.state.pixel_frame = None;
                    }
                    _ => {}
                }
            }

            if app.auto && app.last_tick.elapsed() >= Duration::from_millis(120) {
                app.tick_once();
            }
        }
        Ok(())
    })();

    terminal::disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    result
}
