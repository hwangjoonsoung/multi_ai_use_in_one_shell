//! 앱 상태와 이벤트 루프. R2 — 에이전트 셋을 각각 PTY 에 띄우고 패널로 나눈다.
//!
//! 화면은 두 가지다.
//!   Idle  — 질문 입력만 있는 시작 화면
//!   Panes — 참여자마다 자기 칸. 그 안은 에이전트의 실제 TUI 다.
//!
//! 키는 기본적으로 **포커스된 패널의 자식에게 그대로** 간다. 그래야 권한 승인·
//! 슬래시 자동완성·esc 중단이 동작한다. 우리 조작은 tmux 처럼 프리픽스로 뺀다.

use crate::{pty::PtySession, vtscreen::VtScreen};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};
use std::time::{Duration, Instant};

pub struct Pane {
    pub id: String,
    pub title: String,
    pub session: Option<PtySession>,
    /// 마지막으로 맞춰둔 크기. 달라졌을 때만 resize 한다.
    size: (u16, u16),
}

impl Pane {
    fn new(id: &str, title: &str) -> Self {
        Self { id: id.into(), title: title.into(), session: None, size: (0, 0) }
    }

    fn status(&self) -> &'static str {
        match &self.session {
            None => "대기",
            Some(s) if s.finished() => "종료",
            Some(_) => "실행 중",
        }
    }
}

pub enum Mode {
    Idle,
    Panes,
}

pub struct App {
    pub panes: Vec<Pane>,
    pub focus: usize,
    pub mode: Mode,
    /// 시작 화면에서 타이핑 중인 질문
    pub input: String,
    pub question: String,
    pub status: String,
    /// 프리픽스를 누른 직후인가. 다음 키를 우리 명령으로 해석한다.
    pub prefix: bool,
    pub quit: bool,
    /// 에이전트 TUI 가 뜨기 전에 키를 넣으면 삼켜진다. 잠깐 기다렸다 주입한다.
    inject: Option<(String, Instant)>,
}

impl App {
    pub fn new(agents: &[(&str, &str)]) -> Self {
        Self {
            panes: agents.iter().map(|(id, t)| Pane::new(id, t)).collect(),
            focus: 0,
            mode: Mode::Idle,
            input: String::new(),
            question: String::new(),
            status: String::new(),
            prefix: false,
            quit: false,
            inject: None,
        }
    }

    /// 질문을 확정하고 참여자들을 PTY 에 띄운다.
    ///
    /// 프롬프트는 기동 인자가 아니라 **키 입력으로 주입한다.** 대화형 세션이라
    /// 그래야 하고, 덕분에 Windows 명령행 길이 한계도 걸리지 않는다.
    pub fn start_round(&mut self, question: &str, area: Rect) {
        self.question = question.to_string();
        self.mode = Mode::Panes;
        let (h, w) = pane_size(area, self.panes.len());

        let mut failed = Vec::new();
        for p in self.panes.iter_mut() {
            match PtySession::spawn(&p.id, h, w) {
                Ok(s) => {
                    p.session = Some(s);
                    p.size = (h, w);
                }
                Err(e) => failed.push(format!("{}: {}", p.title, e)),
            }
        }
        self.status = if failed.is_empty() {
            "Ctrl+] 프리픽스 · 1/2/3 포커스 · n 새 질문 · q 종료".into()
        } else {
            format!("기동 실패 — {}", failed.join(", "))
        };
        self.inject = Some((question.to_string(), Instant::now()));
    }

    /// 살아 있는 세션의 출력을 먹이고, 대기 중인 주입을 처리한다.
    pub fn tick(&mut self) {
        for p in self.panes.iter_mut() {
            if let Some(s) = p.session.as_mut() {
                s.pump();
            }
        }
        // 에이전트가 화면을 그리기 시작한 뒤에 넣어야 삼켜지지 않는다.
        if let Some((text, at)) = self.inject.clone() {
            if at.elapsed() >= Duration::from_millis(1500) {
                for p in self.panes.iter_mut() {
                    if let Some(s) = p.session.as_mut() {
                        let _ = s.write(text.as_bytes());
                        let _ = s.write(b"\r");
                    }
                }
                self.inject = None;
            }
        }
    }

    pub fn focused(&mut self) -> Option<&mut PtySession> {
        self.panes.get_mut(self.focus).and_then(|p| p.session.as_mut())
    }

    /// 화면 크기가 바뀌면 각 PTY 도 자기 칸 크기로 맞춘다. 안 맞추면 자식이
    /// 자기 화면을 잘못 그린다.
    pub fn sync_sizes(&mut self, area: Rect) {
        let (h, w) = pane_size(area, self.panes.len());
        for p in self.panes.iter_mut() {
            if p.size != (h, w) {
                if let Some(s) = p.session.as_mut() {
                    let _ = s.resize(h, w);
                    p.size = (h, w);
                }
            }
        }
    }

    // ---------- 렌더 ----------

    pub fn draw(&mut self, f: &mut Frame) {
        match self.mode {
            Mode::Idle => self.draw_idle(f),
            Mode::Panes => self.draw_panes(f),
        }
    }

    fn draw_idle(&self, f: &mut Frame) {
        let area = f.area();
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(35),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(5),
                Constraint::Min(0),
            ])
            .split(area);

        f.render_widget(
            Paragraph::new("multi_ai_cli")
                .style(Style::default().add_modifier(Modifier::BOLD))
                .centered(),
            rows[1],
        );
        let names: Vec<&str> = self.panes.iter().map(|p| p.title.as_str()).collect();
        f.render_widget(
            Paragraph::new(names.join("  ·  "))
                .style(Style::default().fg(Color::DarkGray))
                .centered(),
            rows[2],
        );

        let w = area.width.min(80);
        let box_area = Rect {
            x: area.x + area.width.saturating_sub(w) / 2,
            y: rows[4].y,
            width: w,
            height: 5,
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" 질문을 입력하세요 ");
        let inner = block.inner(box_area);
        f.render_widget(block, box_area);
        f.render_widget(Paragraph::new(self.input.as_str()), inner);

        // 커서를 입력 끝에 둔다.
        let len = self.input.chars().count() as u16;
        let width = inner.width.max(1);
        let cx = inner.x + len % width;
        let cy = inner.y + len / width;
        f.set_cursor_position((
            cx.min(inner.right().saturating_sub(1)),
            cy.min(inner.bottom().saturating_sub(1)),
        ));
    }

    fn draw_panes(&mut self, f: &mut Frame) {
        let area = f.area();
        let vert = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(3), Constraint::Length(1)])
            .split(area);

        f.render_widget(
            Paragraph::new(self.question.as_str())
                .style(Style::default().add_modifier(Modifier::BOLD))
                .block(Block::default().borders(Borders::BOTTOM)),
            vert[0],
        );

        let n = self.panes.len().max(1);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, n as u32); n])
            .split(vert[1]);

        let focus = self.focus;
        for (i, p) in self.panes.iter().enumerate() {
            let focused = i == focus;
            let border = if focused { Color::Cyan } else { Color::DarkGray };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(if focused { BorderType::Thick } else { BorderType::Plain })
                .border_style(Style::default().fg(border))
                .title(format!(" {} [{}] ", p.title, p.status()));
            let inner = block.inner(cols[i]);
            f.render_widget(block, cols[i]);

            if let Some(s) = p.session.as_ref() {
                let screen = s.screen();
                f.render_widget(VtScreen::new(&screen), inner);
                if focused && !screen.hide_cursor() {
                    let (r, c) = screen.cursor_position();
                    f.set_cursor_position((
                        (inner.x + c).min(inner.right().saturating_sub(1)),
                        (inner.y + r).min(inner.bottom().saturating_sub(1)),
                    ));
                }
            }
        }

        let hint = if self.prefix {
            "프리픽스 눌림 — 1/2/3 포커스 · n 새 질문 · q 종료".to_string()
        } else {
            self.status.clone()
        };
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            vert[2],
        );
    }
}

/// 패널 하나에 줄 PTY 크기. 테두리와 상하단 줄을 뺀 값이다.
fn pane_size(area: Rect, n: usize) -> (u16, u16) {
    let n = n.max(1) as u16;
    let w = (area.width / n).saturating_sub(2).max(20);
    let h = area.height.saturating_sub(5).max(6);
    (h, w)
}
