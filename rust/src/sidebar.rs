//! 왼쪽 사이드바. R3 — spaces(작업 공간)와 agents(참여자 상태).
//!
//! herdr 의 배치를 따른다. 위는 작업 디렉터리·브랜치, 아래는 참여자별 상태다.
//! 상태는 추측하지 않고 **관측 가능한 사실만** 쓴다 — 프로세스 생존, 출력이
//! 최근에 있었는지. "무엇을 하는 중인지"는 우리가 알 수 없다.

use crate::app::{App, PaneState};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub const WIDTH: u16 = 26;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(3)])
        .split(area);

    draw_spaces(f, rows[0], app);
    draw_agents(f, rows[1], app);
}

fn draw_spaces(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" spaces ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::from(Span::styled(
        app.workspace_name.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if !app.branch.is_empty() {
        lines.push(Line::from(Span::styled(
            app.branch.clone(),
            Style::default().fg(Color::Magenta),
        )));
    }
    lines.push(Line::from(Span::styled(
        shorten(&app.workspace_path, inner.width as usize),
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_agents(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" agents ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, p) in app.panes.iter().enumerate() {
        let focused = i == app.focus;
        let st = p.state();
        let (dot, color) = match st {
            PaneState::Idle => ("○", Color::DarkGray),
            PaneState::Starting => ("◍", Color::Yellow),
            PaneState::Working => ("●", Color::Green),
            PaneState::Quiet => ("◌", Color::Cyan),
            PaneState::Exited => ("×", Color::Red),
        };
        let name_style = if focused {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{dot} "), Style::default().fg(color)),
            Span::styled(p.title.clone(), name_style),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  {}  ·  alt+{}", st.label(), i + 1),
            Style::default().fg(color),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// 경로가 길면 앞을 줄여 끝을 보여준다. 끝이 더 유용하다.
fn shorten(s: &str, width: usize) -> String {
    if width == 0 || s.chars().count() <= width {
        return s.to_string();
    }
    let keep: String = s.chars().skip(s.chars().count() - width + 1).collect();
    format!("…{keep}")
}
