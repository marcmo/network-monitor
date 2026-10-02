use std::{io, time::Duration};

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{mpsc, watch};

use crate::{
    app::Control,
    ui::{View, draw},
};

pub const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(500);

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let guard = Self { active: true };
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let raw_result = disable_raw_mode();
        let screen_result = execute!(io::stdout(), LeaveAlternateScreen, Show);
        raw_result.and(screen_result)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub fn run(
    mut views: watch::Receiver<Option<View>>,
    controls: mpsc::Sender<Control>,
) -> io::Result<()> {
    let mut guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let mut redraw = true;
    let mut quitting = false;
    while let Ok(changed) = views.has_changed() {
        redraw |= changed;
        if redraw {
            let view = views.borrow_and_update().clone();
            if let Some(view) = view {
                terminal.draw(|frame| draw(frame, &view))?;
            }
            redraw = false;
        }
        if event::poll(INPUT_POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press
                        && !quitting
                        && (key.code == KeyCode::Char('q')
                            || (key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL))) =>
                {
                    let _ = controls.try_send(Control::Quit);
                    quitting = true;
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }
    }
    guard.restore()
}
