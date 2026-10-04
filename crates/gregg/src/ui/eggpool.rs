//! Pure, compact rendering for the optional `EggPool` summary pane.

use std::time::Instant;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::eggpool::{EggpoolFetchOutcome, EggpoolHealthFetchOutcome};
use crate::state::{AppState, EggpoolState, EggpoolWorkerState};
use crate::ui::text::truncate_width;

/// Separator between the header's bounded tokens.
const HEADER_GAP: usize = 4;

#[allow(clippy::too_many_lines)]
pub fn render(f: &mut Frame, area: Rect, state: &AppState) {
    let Some(eggpool) = state.eggpool.as_ref() else {
        return;
    };
    if area.width < 18 || area.height < 6 {
        super::diagnostics::render_too_small(f, area);
        return;
    }
    let header = header_line(eggpool, usize::from(area.width));
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
        let message = if eggpool.worker_state == EggpoolWorkerState::WorkerUnavailable {
            "EggPool worker unavailable".to_owned()
        } else if eggpool.worker_state == EggpoolWorkerState::Refreshing {
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
    if let Some(footer) = footer_line(eggpool, usize::from(area.width)) {
        f.render_widget(
            Paragraph::new(footer),
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

/// One header line: identity, current health, and the selected window.
///
/// Priority is identity/window usability: the health token is dropped
/// before either of them is truncated, and the four metric rows never
/// wrap or shift.
fn header_line(eggpool: &EggpoolState, width: usize) -> String {
    let identity = eggpool
        .endpoint
        .name
        .as_deref()
        .unwrap_or(&eggpool.endpoint.host);
    let label = format!("EggPool — {}", truncate_width(identity, width));
    let window = format!("Window: {}", eggpool.period.display_label());
    let health = health_token(eggpool);
    if fits(width, &[&label, &health, &window]) {
        return join(&[&label, &health, &window]);
    }
    if fits(width, &[&label, &window]) {
        return join(&[&label, &window]);
    }
    // Neither the health token nor a full window fits: keep the identity
    // and the window label, truncated to the available width.
    truncate_width(&join(&[&label, &window]), width)
}

/// Bounded current-health token, or an empty string when nothing is known.
///
/// The plain status word is the primary signal; a retained snapshot with a
/// failed refresh is marked stale rather than presented as current.
fn health_token(eggpool: &EggpoolState) -> String {
    match (eggpool.health.as_ref(), eggpool.last_health_error.as_ref()) {
        (Some(snapshot), None) => format!("Health: {}", snapshot.proxy.as_str()),
        (Some(snapshot), Some(_)) => format!("Health: {} (stale)", snapshot.proxy.as_str()),
        (None, Some(error)) => format!("Health: {}", health_error_text(error)),
        (None, None) => "Health: unknown".to_owned(),
    }
}

fn gap() -> String {
    " ".repeat(HEADER_GAP)
}

fn fits(width: usize, parts: &[&str]) -> bool {
    let separators = HEADER_GAP * parts.len().saturating_sub(1);
    parts.iter().map(|part| part.chars().count()).sum::<usize>() + separators <= width
}

fn join(parts: &[&str]) -> String {
    parts.join(&gap())
}

/// One bounded footer line, chosen by diagnostic priority.
///
/// 1. local worker diagnostics, 2. summary refresh failure or staleness,
/// 3. compact provider counts, 4. nothing.
fn footer_line(eggpool: &EggpoolState, width: usize) -> Option<String> {
    let text = match eggpool.worker_state {
        EggpoolWorkerState::WorkerUnavailable => "worker unavailable".to_owned(),
        EggpoolWorkerState::Refreshing => "refreshing".to_owned(),
        EggpoolWorkerState::Idle => match eggpool.last_error.as_ref() {
            Some(error) => {
                let updated = eggpool.last_success_at.map_or("updated".to_string(), |at| {
                    format!("Updated {}", clock_text(at))
                });
                format!("{updated} — refresh failed: {}", outcome_text(error))
            }
            None => provider_counts_text(eggpool),
        },
    };
    if text.is_empty() {
        None
    } else {
        Some(truncate_width(&text, width))
    }
}

/// Bounded provider context, or an empty string when it is not worth the
/// footer budget.
fn provider_counts_text(eggpool: &EggpoolState) -> String {
    let Some(snapshot) = eggpool.health.as_ref() else {
        return String::new();
    };
    let counts = snapshot.provider_counts();
    if counts.iter().all(|count| *count == 0) {
        return "Providers: none reported".to_owned();
    }
    let words = ["ready", "degraded", "unavailable", "disabled", "unknown"];
    let parts: Vec<String> = counts
        .iter()
        .zip(words)
        .filter(|(count, _)| **count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect();
    format!("Providers: {}", parts.join(" · "))
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

/// Bounded health diagnostic text. Raw bodies, credentials, and upstream
/// error text are never rendered.
fn health_error_text(outcome: &EggpoolHealthFetchOutcome) -> &'static str {
    match outcome {
        EggpoolHealthFetchOutcome::AuthenticationRequired => "auth required",
        EggpoolHealthFetchOutcome::Forbidden => "forbidden",
        EggpoolHealthFetchOutcome::Unsupported => "unsupported",
        EggpoolHealthFetchOutcome::InvalidApiKey => "invalid api key",
        EggpoolHealthFetchOutcome::Timeout => "timed out",
        // Every local transport failure renders the same bounded word: a
        // transport failure is never a `EggPool`-reported proxy status.
        EggpoolHealthFetchOutcome::ConnectionRefused
        | EggpoolHealthFetchOutcome::DnsFailure
        | EggpoolHealthFetchOutcome::NetworkError => "unreachable",
        EggpoolHealthFetchOutcome::HttpStatus(code) if *code == 503 => "unavailable",
        EggpoolHealthFetchOutcome::BodyTooLarge => "response too large",
        EggpoolHealthFetchOutcome::DecodeError => "invalid response",
        EggpoolHealthFetchOutcome::UnsupportedSchema => "schema unsupported",
        EggpoolHealthFetchOutcome::InvalidStatus => "invalid status",
        EggpoolHealthFetchOutcome::InvalidEndpoint => "invalid endpoint",
        EggpoolHealthFetchOutcome::HttpStatus(_) => "unavailable",
        EggpoolHealthFetchOutcome::Online(_) => "unknown",
    }
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
    use crate::eggpool::{EggpoolHealthSnapshot, EggpoolProviderHealth, EggpoolProxyHealth};
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
        AppState::synthetic(&config)
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
    fn snapshot(proxy: crate::eggpool::EggpoolProxyHealth) -> EggpoolHealthSnapshot {
        EggpoolHealthSnapshot {
            schema_version: 1,
            proxy,
            available: true,
            reason_code: None,
            uptime_seconds: None,
            model_count: None,
            routable_accounts: None,
            enabled_accounts: None,
            providers: vec![
                provider_row("a", EggpoolProviderHealth::Ready),
                provider_row("b", EggpoolProviderHealth::Ready),
                provider_row("c", EggpoolProviderHealth::Degraded),
                provider_row("d", EggpoolProviderHealth::Unavailable),
            ],
        }
    }

    fn provider_row(id: &str, status: EggpoolProviderHealth) -> crate::eggpool::EggpoolProviderRow {
        crate::eggpool::EggpoolProviderRow {
            id: id.into(),
            status,
            observation: None,
        }
    }

    fn with_summary(state: &mut AppState) {
        state.eggpool.as_mut().unwrap().summary = Some(crate::eggpool::EggpoolSummary {
            accounted_tokens: 1_250_000,
            cache_read_ratio: None,
            output_tokens_per_second: 12.5,
            avg_ttft_ms: None,
            period: crate::eggpool::EggpoolPeriod::Hour,
        });
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
        assert_eq!(
            health_error_text(&EggpoolHealthFetchOutcome::AuthenticationRequired),
            "auth required"
        );
        assert_eq!(
            health_error_text(&EggpoolHealthFetchOutcome::Unsupported),
            "unsupported"
        );
    }

    #[test]
    fn pending_buffer_identifies_window_without_secret() {
        let output = buffer(&state(), 80, 8);
        assert!(output.contains("EggPool — Main EggPool"));
        assert!(output.contains("Health: unknown"));
        assert!(output.contains("Window: 1 hour"));
        assert!(output.contains("Loading summary…"));
        assert!(!output.contains("SECRET_ENV"));
    }

    #[test]
    fn success_buffer_has_exact_four_metric_labels() {
        let mut state = state();
        with_summary(&mut state);
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

    #[test]
    fn proxy_health_is_a_plain_header_token_with_bounded_provider_counts() {
        for (proxy, word) in [
            (EggpoolProxyHealth::Ready, "ready"),
            (EggpoolProxyHealth::Degraded, "degraded"),
            (EggpoolProxyHealth::Unready, "unready"),
        ] {
            let mut state = state();
            with_summary(&mut state);
            state.eggpool.as_mut().unwrap().health = Some(snapshot(proxy));
            let output = buffer(&state, 100, 8);
            assert!(
                output.contains(&format!("Health: {word}")),
                "missing health token: {output}"
            );
            assert!(output.contains("Providers: 2 ready · 1 degraded · 1 unavailable"));
            // The four metric labels are unchanged by the health plane.
            assert!(output.contains("Accounted tokens"));
            assert!(output.contains("Avg TTFT"));
        }
    }

    #[test]
    fn failed_health_refresh_keeps_but_marks_the_snapshot_stale() {
        let mut state = state();
        with_summary(&mut state);
        state.eggpool.as_mut().unwrap().health = Some(snapshot(EggpoolProxyHealth::Degraded));
        state.eggpool.as_mut().unwrap().last_health_error =
            Some(EggpoolHealthFetchOutcome::ConnectionRefused);
        let output = buffer(&state, 100, 8);
        assert!(output.contains("Health: degraded (stale)"), "{output}");
        assert!(output.contains("Accounted tokens"));
    }

    #[test]
    fn health_failures_without_a_snapshot_render_a_bounded_reason() {
        let mut state = state();
        with_summary(&mut state);
        state.eggpool.as_mut().unwrap().last_health_error =
            Some(EggpoolHealthFetchOutcome::AuthenticationRequired);
        let output = buffer(&state, 100, 8);
        assert!(output.contains("Health: auth required"), "{output}");
        // No provider counts without a snapshot.
        assert!(!output.contains("Providers:"));
    }

    #[test]
    fn worker_unavailable_outranks_health_and_summary_diagnostics() {
        let mut state = state();
        with_summary(&mut state);
        state.eggpool.as_mut().unwrap().health = Some(snapshot(EggpoolProxyHealth::Ready));
        state.eggpool.as_mut().unwrap().last_error = Some(EggpoolFetchOutcome::Timeout);
        state.eggpool.as_mut().unwrap().worker_state = EggpoolWorkerState::WorkerUnavailable;
        let output = buffer(&state, 100, 8);
        assert!(output.contains("worker unavailable"), "{output}");
        assert!(!output.contains("refresh failed"));
        assert!(!output.contains("Providers:"));
    }

    #[test]
    fn summary_failure_outranks_provider_counts() {
        let mut state = state();
        with_summary(&mut state);
        state.eggpool.as_mut().unwrap().health = Some(snapshot(EggpoolProxyHealth::Ready));
        state.eggpool.as_mut().unwrap().last_error = Some(EggpoolFetchOutcome::Timeout);
        let output = buffer(&state, 100, 8);
        assert!(
            output.contains("refresh failed: request timed out"),
            "{output}"
        );
        assert!(!output.contains("Providers:"));
    }

    #[test]
    fn narrow_panes_keep_identity_and_window_and_drop_health_first() {
        let mut state = state();
        with_summary(&mut state);
        state.eggpool.as_mut().unwrap().health = Some(snapshot(EggpoolProxyHealth::Degraded));
        let output = buffer(&state, 40, 8);
        assert!(output.contains("EggPool — Main EggPool"), "{output}");
        assert!(output.contains("Window: 1 hour"), "{output}");
        assert!(!output.contains("Health:"), "{output}");
        assert!(output.contains("Accounted tokens"), "{output}");

        // Below the identity/window budget the line truncates instead of
        // wrapping, and the four metric rows still render.
        let output = buffer(&state, 24, 8);
        assert!(output.contains("EggPool — Main EggP"), "{output}");
        assert!(output.contains("Accounted tokens"), "{output}");
    }

    #[test]
    fn unknown_health_with_no_snapshot_reports_a_stable_bounded_word() {
        let mut state = state();
        with_summary(&mut state);
        state.eggpool.as_mut().unwrap().health = Some(EggpoolHealthSnapshot {
            providers: Vec::new(),
            ..snapshot(EggpoolProxyHealth::Ready)
        });
        let output = buffer(&state, 100, 8);
        assert!(output.contains("Health: ready"), "{output}");
        assert!(output.contains("Providers: none reported"), "{output}");
    }
}
