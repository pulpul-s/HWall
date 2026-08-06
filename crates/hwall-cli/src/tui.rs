mod app;
mod header;

use app::App;
use hwall_app::TerminalView;
use hwall_core::MonitorCollector;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::io;
use std::time::Duration;

const MAX_INPUT_EVENTS_PER_TURN: usize = 64;

pub(crate) fn run(
    collector: MonitorCollector,
    interval: Duration,
    verbose: bool,
    view: TerminalView,
) -> io::Result<()> {
    let mut app = App::new(collector, interval, verbose, view)?;
    ratatui::run(|terminal| run_loop(terminal, &mut app))
}

fn run_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        app.tick();
        if app.should_quit() {
            return Ok(());
        }

        if app.needs_redraw() {
            terminal.draw(|frame| app.draw(frame))?;
            app.mark_drawn();
        }

        if event::poll(app.poll_timeout())? {
            for _ in 0..MAX_INPUT_EVENTS_PER_TURN {
                match event::read()? {
                    Event::Key(key)
                        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        app.handle_key(key);
                    }
                    Event::Resize(_, _) => app.mark_dirty(),
                    _ => {}
                }

                if app.should_quit() || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
}
