// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{Evidence, RunSummary};
use crate::progress::{EventKind, ProgressEvent};
use crossterm::event::{self, Event, KeyCode};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct App {
    pub phase: String,
    pub message: String,
    pub percent: u8,
    pub elapsed_ms: u64,
    pub recent: Vec<RunSummary>,
    pub show_details: bool,
}

impl App {
    #[must_use]
    pub fn new(recent: Vec<RunSummary>) -> Self {
        Self {
            phase: "ready".into(),
            message: "Select a command or press q to exit".into(),
            percent: 0,
            elapsed_ms: 0,
            recent,
            show_details: false,
        }
    }

    pub fn apply(&mut self, event: &ProgressEvent) {
        self.phase.clone_from(&event.phase);
        self.message.clone_from(&event.message);
        self.elapsed_ms = event.elapsed_ms;
        if let Some(percent) = event.percent {
            self.percent = percent;
        }
    }
}

pub fn run(evidence: &Evidence, request_id: &str) -> Result<(), String> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, evidence, request_id);
    ratatui::restore();
    result
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    evidence: &Evidence,
    request_id: &str,
) -> Result<(), String> {
    let mut app = App::new(evidence.recent_runs(false, 8)?);
    let started = ProgressEvent {
        v: 1,
        kind: EventKind::Started,
        run: request_id.into(),
        seq: 0,
        elapsed_ms: 0,
        phase: "interactive".into(),
        message: "Operator interface ready".into(),
        percent: Some(0),
    };
    evidence.record_event(&started)?;
    app.apply(&started);
    loop {
        terminal
            .draw(|frame| render(frame, &app))
            .map_err(|error| error.to_string())?;
        if event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
            if let Event::Key(key) = event::read().map_err(|error| error.to_string())? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Enter => app.show_details = !app.show_details,
                    _ => {}
                }
            }
        }
    }
    let completed = ProgressEvent {
        v: 1,
        kind: EventKind::Completed,
        run: request_id.into(),
        seq: 1,
        elapsed_ms: app.elapsed_ms,
        phase: "interactive".into(),
        message: "Operator interface closed".into(),
        percent: Some(100),
    };
    evidence.record_event(&completed)
}

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new(format!(
            "{} · {} · {} ms",
            app.phase, app.message, app.elapsed_ms
        ))
        .block(
            Block::default()
                .title("MiSTer MagiK agent-cli")
                .borders(Borders::ALL),
        ),
        areas[0],
    );
    frame.render_widget(
        Gauge::default()
            .block(Block::default().title("Progress").borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Cyan))
            .percent(u16::from(app.percent)),
        areas[1],
    );
    let items: Vec<_> = app
        .recent
        .iter()
        .map(|run| {
            ListItem::new(Line::from(format!(
                "{}  {}",
                run.outcome.as_deref().unwrap_or("running"),
                run.id
            )))
        })
        .collect();
    let title = if app.show_details {
        "Recent runs · details visible"
    } else {
        "Recent runs · Enter toggles details"
    };
    frame.render_widget(
        List::new(items).block(Block::default().title(title).borders(Borders::ALL)),
        areas[2],
    );
    frame.render_widget(Paragraph::new("q/Esc exit · Enter details"), areas[3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_progress_and_recent_runs() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(vec![RunSummary {
            id: "run-1".into(),
            started_ms: 1,
            completed_ms: Some(2),
            parse_status: "parsed".into(),
            outcome: Some("passed".into()),
        }]);
        app.apply(&ProgressEvent {
            v: 1,
            kind: EventKind::Progress,
            run: "run-2".into(),
            seq: 1,
            elapsed_ms: 500,
            phase: "transfer".into(),
            message: "Uploading ARM binary".into(),
            percent: Some(40),
        });
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Uploading ARM binary"));
        assert!(rendered.contains("40%"));
        assert!(rendered.contains("run-1"));
    }
}
