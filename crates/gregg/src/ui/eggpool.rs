//! Pure, compact rendering for the optional `EggPool` summary pane.

use std::time::Instant;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::eggpool::EggpoolFetchOutcome;
use crate::state::{AppState, EggpoolStatus};
use crate::ui::text::truncate_width;

#[allow(clippy::too_many_lines)]
pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let Some(eggpool) = state.eggpool.as_ref() else {
        return;
    };
    if area.width < 18 || area.height < 6 {
        super::diagnostics::render_too_small(f, area);
        return;
    }
    let identity = eggpool
        .endpoint
        .name
        .as_deref()
        .unwrap_or(&eggpool.endpoint.host);
    let header = format!(
        "EggPool — {}    Window: {}",
        truncate_width(identity, usize::from(area.width)),
        eggpool.period.display_label()
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::raw(header))),
        Rect { height: 1, ..area },
    );

    let body = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let Some(summary) = eggpool.summary.as_ref() else {
        let message = if eggpool.status == EggpoolStatus::WorkerUnavailable {
            "EggPool worker unavailable".to_owned()
        } else if eggpool.status == EggpoolStatus::Busy {
            "EggPool worker busy; retry".to_owned()
        } else if eggpool.status == EggpoolStatus::Refreshing {
            "Loading summary…".to_owned()
        } else if let Some(error) = eggpool.last_error.as_ref() {
            outcome_text(error)
        } else {
            "Loading summary…".to_owned()
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message,
                Style::default().fg(Color::Yellow),
            ))),
            body,
        );
        if area.height >= 7 {
            super::diagnostics::render_key_hint(f, area, state);
        }
        return;
    };

    let metrics = [
        format!(
            "Accounted tokens  {}",
            format_count(summary.accounted_tokens)
        ),
        format!(
            "Cache read share  {}",
            summary
                .cache_read_ratio
                .map_or("—".into(), |v| format!("{:.1}%", v * 100.0))
        ),
        format!(
            "Output tok/s      {}",
            format_rate(summary.output_tokens_per_second)
        ),
        format!(
            "Avg TTFT          {}",
            summary.avg_ttft_ms.map_or("—".into(), format_duration)
        ),
    ];
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1); 4])
        .split(body);
    for (metric, row) in metrics.iter().zip(rows.iter()) {
        f.render_widget(Paragraph::new(Line::from(Span::raw(metric))), *row);
    }
    let footer_y = body
        .y
        .saturating_add(4)
        .min(area.bottom().saturating_sub(1));
    if eggpool.status == EggpoolStatus::WorkerUnavailable {
        f.render_widget(
            Paragraph::new("worker unavailable"),
            Rect {
                x: area.x,
                y: footer_y,
                width: area.width,
                height: 1,
            },
        );
    } else if eggpool.status == EggpoolStatus::Busy {
        f.render_widget(
            Paragraph::new("worker busy"),
            Rect {
                x: area.x,
                y: footer_y,
                width: area.width,
                height: 1,
            },
        );
    } else if eggpool.status == EggpoolStatus::Refreshing {
        f.render_widget(
            Paragraph::new("refreshing"),
            Rect {
                x: area.x,
                y: footer_y,
                width: area.width,
                height: 1,
            },
        );
    } else if let Some(error) = eggpool.last_error.as_ref() {
        let updated = eggpool.last_success_at.map_or("updated".to_string(), |at| {
            format!("Updated {}", clock_text(at))
        });
        let text = format!("{updated} — refresh failed: {}", outcome_text(error));
        f.render_widget(
            Paragraph::new(text),
            Rect {
                x: area.x,
                y: footer_y,
                width: area.width,
                height: 1,
            },
        );
    }
    if area.height >= 7 {
        super::diagnostics::render_key_hint(f, area, state);
    }
}

#[allow(clippy::cast_precision_loss)]
fn format_count(value: u64) -> String {
    const UNITS: [&str; 7] = ["", "K", "M", "B", "T", "P", "E"];
    let mut value = value as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        value.round().to_string()
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn format_rate(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "—".into();
    }
    if value < 1000.0 {
        format!("{value:.1} tok/s")
    } else {
        format!("{} tok/s", format_count(value as u64))
    }
}

fn format_duration(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        "—".into()
    } else {
        format!("{value:.1} ms")
    }
}

fn clock_text(_at: Instant) -> String {
    "recently".into()
}

fn outcome_text(outcome: &EggpoolFetchOutcome) -> String {
    match outcome {
        EggpoolFetchOutcome::MissingApiKeyEnv { name } => {
            format!("API key environment variable {name} is not set")
        }
        EggpoolFetchOutcome::Unauthorized => "authentication required or key rejected".into(),
        EggpoolFetchOutcome::Forbidden => "access forbidden".into(),
        EggpoolFetchOutcome::StatsUnavailable => {
            "stats unavailable — enable EggPool dashboard/statistics routes".into()
        }
        EggpoolFetchOutcome::Timeout => "request timed out".into(),
        EggpoolFetchOutcome::ConnectionRefused => "connection refused".into(),
        EggpoolFetchOutcome::DnsFailure => "DNS lookup failed".into(),
        EggpoolFetchOutcome::NetworkError => "network error".into(),
        EggpoolFetchOutcome::HttpStatus(code) => format!("HTTP {code}"),
        EggpoolFetchOutcome::BodyTooLarge => "response too large".into(),
        EggpoolFetchOutcome::DecodeError => "invalid JSON response".into(),
        EggpoolFetchOutcome::InvalidSummary => "invalid summary response".into(),
        EggpoolFetchOutcome::Cancelled => "refresh cancelled".into(),
        EggpoolFetchOutcome::InvalidEndpoint => "invalid endpoint".into(),
        EggpoolFetchOutcome::Online(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, EggpoolEntry, EggpoolScheme};
    use crate::state::AppState;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn state() -> AppState {
        let config = Config {
            eggpool: Some(EggpoolEntry {
                id: "pool".into(),
                host: "pool.local".into(),
                port: 11300,
                scheme: EggpoolScheme::Http,
                name: Some("Main EggPool".into()),
                api_key_env: Some("SECRET_ENV".into()),
            }),
            ..Config::default()
        };
        AppState::from_config(&config)
    }

    fn buffer(state: &AppState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), state))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn large_count_is_bounded() {
        assert_eq!(format_count(u64::MAX), "18.4E");
    }
    #[test]
    fn errors_do_not_expose_raw_outcomes() {
        assert_eq!(
            outcome_text(&EggpoolFetchOutcome::Timeout),
            "request timed out"
        );
    }

    #[test]
    fn pending_buffer_identifies_window_without_secret() {
        let output = buffer(&state(), 80, 8);
        assert!(output.contains("EggPool — Main EggPool"));
        assert!(output.contains("Window: 1 hour"));
        assert!(output.contains("Loading summary…"));
        assert!(!output.contains("SECRET_ENV"));
    }

    #[test]
    fn success_buffer_has_exact_four_metric_labels() {
        let mut state = state();
        state.eggpool.as_mut().unwrap().summary = Some(crate::eggpool::EggpoolSummary {
            accounted_tokens: 1_250_000,
            cache_read_ratio: None,
            output_tokens_per_second: 12.5,
            avg_ttft_ms: None,
            period: crate::eggpool::EggpoolPeriod::Hour,
        });
        let output = buffer(&state, 100, 8);
        for label in [
            "Accounted tokens",
            "Cache read share",
            "Output tok/s",
            "Avg TTFT",
        ] {
            assert!(output.contains(label), "missing {label}: {output}");
        }
        assert!(output.contains("1.2M"));
        assert!(!output.contains("SECRET_ENV"));
    }
}
