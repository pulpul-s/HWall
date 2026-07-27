use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Duration;

const STATE_WIDTH: usize = 8;
const LATENCY_WIDTH: usize = 7;
const COUNT_WIDTH: usize = 6;
const LINE_WIDTH: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActivityState {
    Live,
    Paused,
}

impl ActivityState {
    fn label(self) -> &'static str {
        match self {
            Self::Live => "LIVE",
            Self::Paused => "PAUSED",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Live => Color::Green,
            Self::Paused => Color::Yellow,
        }
    }
}

pub(super) struct HeaderData<'a> {
    pub state: ActivityState,
    pub mode: &'a str,
    pub interval: Duration,
    pub collection_latency: Option<Duration>,
    pub render_latency: Duration,
    pub samples: u64,
    pub first_line: usize,
    pub last_line: usize,
    pub total_lines: usize,
}

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, data: HeaderData<'_>) {
    let layout = build_layout(area.width, &data);
    let right_width = text_width(&layout.state)
        .saturating_add(text_width(&layout.metrics))
        .min(usize::from(area.width)) as u16;

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_width)])
        .split(area);

    let left = Paragraph::new(layout.left)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Left);
    frame.render_widget(left, chunks[0]);

    let right = Paragraph::new(Line::from(vec![
        Span::styled(
            layout.state,
            Style::default()
                .fg(data.state.color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(layout.metrics, Style::default().fg(Color::DarkGray)),
    ]))
    .alignment(Alignment::Right);
    frame.render_widget(right, chunks[1]);
}

#[derive(Debug, PartialEq, Eq)]
struct HeaderLayout {
    left: String,
    state: String,
    metrics: String,
}

fn build_layout(width: u16, data: &HeaderData<'_>) -> HeaderLayout {
    let full_left = format!(
        "HWall  •  {}  •  refresh {:.3}s",
        data.mode,
        data.interval.as_secs_f64()
    );
    let mode_left = format!("HWall  •  {}", data.mode);
    let minimal_left = "HWall".to_owned();

    let state = format!("{:<width$}", data.state.label(), width = STATE_WIDTH);
    let width = usize::from(width);
    let left = if text_width(&full_left).saturating_add(STATE_WIDTH) <= width {
        full_left
    } else if text_width(&mode_left).saturating_add(STATE_WIDTH) <= width {
        mode_left
    } else {
        minimal_left
    };

    let collection = latency_field(data.collection_latency);
    let rendering = latency_field(Some(data.render_latency));
    let samples = bounded_number(data.samples, COUNT_WIDTH);
    let first = bounded_number(data.first_line as u64, LINE_WIDTH);
    let last = bounded_number(data.last_line as u64, LINE_WIDTH);
    let total = bounded_number(data.total_lines as u64, LINE_WIDTH);

    let full_metrics = format!(
        " • samples {samples} • collect {collection} • render {rendering} • lines {first}-{last}/{total}"
    );
    let medium_metrics = format!(" • samples {samples} • C {collection} • R {rendering}");
    let compact_metrics = format!(" • samples {samples} • {first}-{last}/{total}");
    let available_metrics = width
        .saturating_sub(text_width(&left))
        .saturating_sub(STATE_WIDTH);
    let metrics = [full_metrics, medium_metrics, compact_metrics]
        .into_iter()
        .find(|candidate| text_width(candidate) <= available_metrics)
        .unwrap_or_default();

    HeaderLayout {
        left,
        state,
        metrics,
    }
}

fn latency_field(duration: Option<Duration>) -> String {
    let value = match duration {
        None => "startup".to_owned(),
        Some(duration) if duration < Duration::from_millis(1) => {
            format!("{}µs", duration.as_micros().min(999))
        }
        Some(duration) if duration < Duration::from_secs(1) => {
            format!("{}ms", duration.as_millis().min(999))
        }
        Some(duration) if duration < Duration::from_secs(100) => {
            format!("{:.2}s", duration.as_secs_f64())
        }
        Some(_) => ">99.9s".to_owned(),
    };
    format!("{:>width$}", value, width = LATENCY_WIDTH)
}

fn bounded_number(value: u64, width: usize) -> String {
    let text = value.to_string();
    if text.len() <= width {
        format!("{:>width$}", text, width = width)
    } else {
        format!("{}+", "9".repeat(width.saturating_sub(1)))
    }
}

fn text_width(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(
        state: ActivityState,
        collection: Duration,
        rendering: Duration,
    ) -> HeaderData<'static> {
        HeaderData {
            state,
            mode: "hardware",
            interval: Duration::from_secs(1),
            collection_latency: Some(collection),
            render_latency: rendering,
            samples: 42,
            first_line: 1,
            last_line: 40,
            total_lines: 367,
        }
    }

    #[test]
    fn volatile_state_does_not_change_left_segment() {
        let live = build_layout(
            140,
            &data(
                ActivityState::Live,
                Duration::from_millis(7),
                Duration::from_millis(2),
            ),
        );
        let paused = build_layout(
            140,
            &data(
                ActivityState::Paused,
                Duration::from_millis(987),
                Duration::from_micros(450),
            ),
        );

        assert_eq!(live.left, paused.left);
        assert_eq!(live.state.chars().count(), paused.state.chars().count());
        assert_eq!(live.metrics.chars().count(), paused.metrics.chars().count());
    }

    #[test]
    fn latency_fields_have_fixed_width() {
        assert_eq!(latency_field(None).chars().count(), LATENCY_WIDTH);
        assert_eq!(
            latency_field(Some(Duration::from_micros(450)))
                .chars()
                .count(),
            LATENCY_WIDTH
        );
        assert_eq!(
            latency_field(Some(Duration::from_millis(27)))
                .chars()
                .count(),
            LATENCY_WIDTH
        );
        assert_eq!(
            latency_field(Some(Duration::from_millis(1_250)))
                .chars()
                .count(),
            LATENCY_WIDTH
        );
    }

    #[test]
    fn narrow_headers_drop_metrics_before_identity() {
        let compact = build_layout(
            50,
            &data(
                ActivityState::Paused,
                Duration::from_millis(27),
                Duration::from_millis(2),
            ),
        );

        assert_eq!(compact.left, "HWall  •  hardware  •  refresh 1.000s");
        assert!(compact.metrics.is_empty());
        assert_eq!(compact.state, "PAUSED  ");
    }

    #[test]
    fn chosen_segments_fit_without_overlapping() {
        let sample = data(
            ActivityState::Live,
            Duration::from_millis(73),
            Duration::from_millis(2),
        );
        for width in 15..=180 {
            let layout = build_layout(width, &sample);
            let used =
                text_width(&layout.left) + text_width(&layout.state) + text_width(&layout.metrics);
            assert!(used <= usize::from(width), "width {width} used {used}");
        }
    }
}
