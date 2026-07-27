use super::header::{self, ActivityState, HeaderData};
use hwall_core::{
    render, MonitorCollector, MonitorPoll, MonitorRequestResult, MonitorWorker, Snapshot,
    SnapshotStatistics,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::cmp;
use std::io;
use std::time::{Duration, Instant};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Search,
}

pub(super) struct App {
    worker: MonitorWorker,
    snapshot: Snapshot,
    statistics: SnapshotStatistics,
    report: String,
    styled_lines: Vec<Line<'static>>,
    report_line_count: u16,
    report_width: u16,
    interval: Duration,
    next_refresh: Instant,
    refresh_in_flight: bool,
    discard_in_flight_update: bool,
    force_refresh_pending: bool,
    worker_disconnected: bool,
    last_collection_duration: Option<Duration>,
    last_render_duration: Duration,
    paused: bool,
    verbose: bool,
    quit: bool,
    dirty: bool,
    vertical_scroll: u16,
    horizontal_scroll: u16,
    max_vertical_scroll: u16,
    max_horizontal_scroll: u16,
    viewport_height: u16,
    mode: InputMode,
    search_input: String,
    search_query: String,
    current_match: Option<usize>,
    help: bool,
    status_message: String,
}

impl App {
    pub(super) fn new(
        collector: MonitorCollector,
        interval: Duration,
        verbose: bool,
    ) -> io::Result<Self> {
        let snapshot = collector.initial_snapshot();
        let worker = MonitorWorker::spawn(collector)?;
        let mut statistics = SnapshotStatistics::new();
        statistics.observe(&snapshot);

        let mut app = Self {
            worker,
            snapshot,
            statistics,
            report: String::new(),
            styled_lines: Vec::new(),
            report_line_count: 0,
            report_width: 0,
            interval,
            next_refresh: Instant::now() + interval,
            refresh_in_flight: false,
            discard_in_flight_update: false,
            force_refresh_pending: false,
            worker_disconnected: false,
            last_collection_duration: None,
            last_render_duration: Duration::ZERO,
            paused: false,
            verbose,
            quit: false,
            dirty: true,
            vertical_scroll: 0,
            horizontal_scroll: 0,
            max_vertical_scroll: 0,
            max_horizontal_scroll: 0,
            viewport_height: 1,
            mode: InputMode::Normal,
            search_input: String::new(),
            search_query: String::new(),
            current_match: None,
            help: false,
            status_message: String::new(),
        };
        app.rebuild_report();
        Ok(app)
    }

    pub(super) fn tick(&mut self) {
        self.receive_collection_updates();
        self.request_refresh_when_due();
    }

    pub(super) fn should_quit(&self) -> bool {
        self.quit
    }

    pub(super) fn needs_redraw(&self) -> bool {
        self.dirty
    }

    pub(super) fn mark_drawn(&mut self) {
        self.dirty = false;
    }

    pub(super) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn receive_collection_updates(&mut self) {
        if self.worker_disconnected {
            return;
        }

        loop {
            match self.worker.poll() {
                MonitorPoll::Update(update) => {
                    self.refresh_in_flight = false;
                    self.last_collection_duration = Some(update.elapsed);
                    let now = Instant::now();
                    if now >= self.next_refresh {
                        self.next_refresh = now + self.interval;
                    }

                    let discarded_for_pause = self.discard_in_flight_update;
                    let apply_update = !self.paused && !discarded_for_pause;
                    self.discard_in_flight_update = false;
                    if apply_update {
                        self.snapshot = update.snapshot;
                        self.statistics.observe(&self.snapshot);
                        self.rebuild_report();
                        self.status_message = if update.forced_rediscovery {
                            "Full hardware rediscovery completed".to_owned()
                        } else {
                            String::new()
                        };
                    }

                    if !self.paused && (self.force_refresh_pending || discarded_for_pause) {
                        self.next_refresh = Instant::now();
                    }
                    self.dirty = true;
                }
                MonitorPoll::Idle => break,
                MonitorPoll::Disconnected => {
                    self.refresh_in_flight = false;
                    self.worker_disconnected = true;
                    self.status_message = "Collector worker stopped unexpectedly".to_owned();
                    self.dirty = true;
                    break;
                }
            }
        }
    }

    fn request_refresh_when_due(&mut self) {
        if self.paused
            || self.refresh_in_flight
            || self.worker_disconnected
            || (Instant::now() < self.next_refresh && !self.force_refresh_pending)
        {
            return;
        }

        let force_rediscovery = self.force_refresh_pending;
        match self.worker.request(force_rediscovery) {
            MonitorRequestResult::Accepted => {
                self.refresh_in_flight = true;
                self.discard_in_flight_update = false;
                self.next_refresh = Instant::now() + self.interval;
                self.dirty = true;
                self.force_refresh_pending = false;
                if force_rediscovery {
                    self.status_message = "Full hardware rediscovery in progress…".to_owned();
                    self.dirty = true;
                }
            }
            MonitorRequestResult::Busy => {
                self.next_refresh = Instant::now() + WORKER_POLL_INTERVAL;
            }
            MonitorRequestResult::Disconnected => {
                self.worker_disconnected = true;
                self.status_message = "Collector worker is unavailable".to_owned();
                self.dirty = true;
            }
        }
    }

    pub(super) fn poll_timeout(&self) -> Duration {
        if self.refresh_in_flight {
            return WORKER_POLL_INTERVAL;
        }
        if self.paused || self.worker_disconnected {
            return Duration::from_secs(1);
        }
        self.next_refresh
            .saturating_duration_since(Instant::now())
            .min(WORKER_POLL_INTERVAL)
    }

    fn rebuild_report(&mut self) {
        let started = Instant::now();
        let previous_match = self.current_match;
        self.report = render::live(&self.snapshot, &self.statistics, self.verbose);
        self.reconcile_current_match(previous_match);
        self.rebuild_styled_lines();
        self.last_render_duration = started.elapsed();
        self.clamp_scroll();
        self.dirty = true;
    }

    fn rebuild_styled_lines(&mut self) {
        let query = self.search_query.to_lowercase();
        self.report_width = self
            .report
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0)
            .min(u16::MAX as usize) as u16;
        self.styled_lines = self
            .report
            .lines()
            .enumerate()
            .map(|(index, line)| {
                if self.current_match == Some(index) {
                    Line::styled(
                        line.to_owned(),
                        Style::default().fg(Color::Black).bg(Color::Yellow),
                    )
                } else if !query.is_empty() && line.to_lowercase().contains(&query) {
                    Line::styled(line.to_owned(), Style::default().fg(Color::Yellow))
                } else {
                    Line::raw(line.to_owned())
                }
            })
            .collect();
        self.report_line_count = self.styled_lines.len().min(u16::MAX as usize) as u16;
    }

    fn reconcile_current_match(&mut self, previous_match: Option<usize>) {
        if self.search_query.is_empty() {
            self.current_match = None;
            return;
        }
        let query = self.search_query.to_lowercase();
        let matches: Vec<usize> = self
            .report
            .lines()
            .enumerate()
            .filter_map(|(index, line)| line.to_lowercase().contains(&query).then_some(index))
            .collect();
        self.current_match = previous_match.and_then(|previous| {
            matches
                .iter()
                .copied()
                .min_by_key(|candidate| candidate.abs_diff(previous))
        });
    }

    pub(super) fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        let inner_height = chunks[1].height.saturating_sub(2);
        let inner_width = chunks[1].width.saturating_sub(2);
        self.viewport_height = inner_height.max(1);
        self.recalculate_scroll_limits(inner_height, inner_width);

        let total_lines = usize::from(self.report_line_count.max(1));
        let first_line = usize::from(self.vertical_scroll) + 1;
        let last_line =
            (first_line + usize::from(self.viewport_height).saturating_sub(1)).min(total_lines);
        let mode = if self.verbose {
            "diagnostic"
        } else {
            "hardware"
        };
        let state = if self.paused {
            ActivityState::Paused
        } else {
            ActivityState::Live
        };
        header::render(
            frame,
            chunks[0],
            HeaderData {
                state,
                mode,
                interval: self.interval,
                collection_latency: self.last_collection_duration,
                render_latency: self.last_render_duration,
                samples: self.statistics.sample_rounds(),
                first_line,
                last_line,
                total_lines,
            },
        );

        let start = usize::from(self.vertical_scroll).min(self.styled_lines.len());
        let end = (start + usize::from(self.viewport_height)).min(self.styled_lines.len());
        let visible_lines = self.styled_lines[start..end].to_vec();
        let report = Paragraph::new(visible_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Hardware report "),
            )
            .scroll((0, self.horizontal_scroll));
        frame.render_widget(report, chunks[1]);

        let footer_text = match self.mode {
            InputMode::Search => format!("/{}", self.search_input),
            InputMode::Normal if !self.status_message.is_empty() => self.status_message.clone(),
            InputMode::Normal => concat!(
                "↑↓/jk move  PgUp/PgDn page  ←→/hl sideways  / search  ",
                "n/N match  Space pause  x reset stats  v diagnostic  r rediscover  ? help  q quit"
            )
            .to_owned(),
        };
        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Left);
        frame.render_widget(footer, chunks[2]);

        if self.help {
            self.draw_help(frame, area);
        }
    }

    fn draw_help(&self, frame: &mut Frame<'_>, area: Rect) {
        let popup = centered_rect(76, 72, area);
        frame.render_widget(Clear, popup);
        let help = Paragraph::new(
            "Navigation\n\
             ↑ / k       move up one line\n\
             ↓ / j       move down one line\n\
             Page Up     move up one screen\n\
             Page Down   move down one screen\n\
             b / f       previous / next screen\n\
             Ctrl-U/D    previous / next half-screen\n\
             Home / g    first line\n\
             End / G     last line\n\
             ← / h       scroll left\n\
             → / l       scroll right\n\n\
             Monitoring\n\
             Space       pause or resume updates\n\
             x           reset minimum / maximum / average\n\
             r           queue full rediscovery\n\
             v           hardware / diagnostic view\n\n\
             Search\n\
             /           enter a search\n\
             n / N       next / previous match\n\n\
             q / Esc     close help or quit\n\
             ?           toggle this help",
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Keyboard help "),
        )
        .alignment(Alignment::Left);
        frame.render_widget(help, popup);
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        if self.help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.help = false,
                _ => {}
            }
            self.dirty = true;
            return;
        }

        match self.mode {
            InputMode::Search => self.handle_search_key(key),
            InputMode::Normal => self.handle_normal_key(key),
        }
        self.dirty = true;
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.search_input.clear();
            }
            KeyCode::Enter => {
                self.search_query = self.search_input.trim().to_owned();
                self.mode = InputMode::Normal;
                self.current_match = None;
                self.rebuild_styled_lines();
                if self.search_query.is_empty() {
                    self.status_message = "Search cleared".to_owned();
                } else if !self.find_match(false) {
                    self.status_message = format!("No match for ‘{}’", self.search_query);
                }
            }
            KeyCode::Backspace => {
                self.search_input.pop();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_input.push(character);
            }
            _ => {}
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_down(1),
            KeyCode::PageUp | KeyCode::Char('b') => self.scroll_up(self.page_size()),
            KeyCode::PageDown | KeyCode::Char('f') => self.scroll_down(self.page_size()),
            KeyCode::Home | KeyCode::Char('g') => self.vertical_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.vertical_scroll = self.max_vertical_scroll,
            KeyCode::Left | KeyCode::Char('h') => self.scroll_left(4),
            KeyCode::Right | KeyCode::Char('l') => self.scroll_right(4),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_up(self.page_size() / 2)
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_down(self.page_size() / 2)
            }
            KeyCode::Char(' ') => {
                self.paused = !self.paused;
                self.status_message = if self.paused {
                    self.discard_in_flight_update = self.refresh_in_flight;
                    "Updates paused".to_owned()
                } else {
                    self.next_refresh = Instant::now();
                    "Updates resumed".to_owned()
                };
            }
            KeyCode::Char('x') => {
                self.statistics.reset_with(&self.snapshot);
                self.rebuild_report();
                self.status_message = "Live statistics reset".to_owned();
            }
            KeyCode::Char('r') => {
                self.force_refresh_pending = true;
                self.next_refresh = Instant::now();
                self.status_message = if self.refresh_in_flight {
                    "Full hardware rediscovery queued".to_owned()
                } else {
                    "Full hardware rediscovery requested".to_owned()
                };
            }
            KeyCode::Char('v') => {
                self.verbose = !self.verbose;
                self.rebuild_report();
                self.status_message = if self.verbose {
                    "Diagnostic inventory enabled".to_owned()
                } else {
                    "Full hardware view enabled".to_owned()
                };
            }
            KeyCode::Char('/') => {
                self.mode = InputMode::Search;
                self.search_input = self.search_query.clone();
            }
            KeyCode::Char('n') => {
                if !self.find_match(false) && !self.search_query.is_empty() {
                    self.status_message = format!("No match for ‘{}’", self.search_query);
                }
            }
            KeyCode::Char('N') => {
                if !self.find_match(true) && !self.search_query.is_empty() {
                    self.status_message = format!("No match for ‘{}’", self.search_query);
                }
            }
            KeyCode::Char('?') => self.help = true,
            _ => {}
        }
    }

    fn page_size(&self) -> u16 {
        self.viewport_height.saturating_sub(1).max(1)
    }

    fn scroll_up(&mut self, amount: u16) {
        self.vertical_scroll = self.vertical_scroll.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: u16) {
        self.vertical_scroll = self
            .vertical_scroll
            .saturating_add(amount)
            .min(self.max_vertical_scroll);
    }

    fn scroll_left(&mut self, amount: u16) {
        self.horizontal_scroll = self.horizontal_scroll.saturating_sub(amount);
    }

    fn scroll_right(&mut self, amount: u16) {
        self.horizontal_scroll = self
            .horizontal_scroll
            .saturating_add(amount)
            .min(self.max_horizontal_scroll);
    }

    fn recalculate_scroll_limits(&mut self, viewport_height: u16, viewport_width: u16) {
        self.max_vertical_scroll = self.report_line_count.saturating_sub(viewport_height);
        self.max_horizontal_scroll = self.report_width.saturating_sub(viewport_width);
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        self.vertical_scroll = self.vertical_scroll.min(self.max_vertical_scroll);
        self.horizontal_scroll = self.horizontal_scroll.min(self.max_horizontal_scroll);
    }

    fn find_match(&mut self, reverse: bool) -> bool {
        if self.search_query.is_empty() {
            return false;
        }
        let query = self.search_query.to_lowercase();
        let matches: Vec<usize> = self
            .report
            .lines()
            .enumerate()
            .filter_map(|(index, line)| line.to_lowercase().contains(&query).then_some(index))
            .collect();
        let Some((&first, &last)) = matches.first().zip(matches.last()) else {
            self.current_match = None;
            self.rebuild_styled_lines();
            return false;
        };

        let selected = if reverse {
            match self.current_match {
                Some(current) => matches
                    .iter()
                    .rev()
                    .copied()
                    .find(|index| *index < current)
                    .unwrap_or(last),
                None => last,
            }
        } else {
            match self.current_match {
                Some(current) => matches
                    .iter()
                    .copied()
                    .find(|index| *index > current)
                    .unwrap_or(first),
                None => first,
            }
        };

        self.current_match = Some(selected);
        self.vertical_scroll = cmp::min(
            selected.min(u16::MAX as usize) as u16,
            self.max_vertical_scroll,
        );
        let match_number = matches
            .iter()
            .position(|index| *index == selected)
            .unwrap_or(0)
            + 1;
        self.status_message = format!(
            "Match {match_number} of {} for ‘{}’",
            matches.len(),
            self.search_query
        );
        self.rebuild_styled_lines();
        true
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
