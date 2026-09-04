//! 앱 상태와 화면. 「공간 × 세션」 구조다.
//!
//! 화면 모드
//!   Idle     — 질문 입력만 있는 시작 화면
//!   Panes    — 선택된 공간의 세션들. 그 안은 에이전트의 실제 TUI 다.
//!   Picker   — `+` 를 눌러 어떤 에이전트를 띄울지 고르는 중
//!   NewSpace — 새 공간의 경로를 입력하는 중
//!
//! 키는 기본적으로 **포커스된 세션의 자식에게 그대로** 간다. 그래야 권한 승인·
//! 슬래시 자동완성·esc 중단이 동작한다. 우리 조작은 tmux 처럼 프리픽스로 뺀다.
//!
//! 세션이 넷 이상이면 나란히 두지 않고 **탭**으로 바꾼다. 좁은 칸에 에이전트
//! TUI 를 밀어 넣으면 자식이 자기 화면을 접어버려 읽을 수 없기 때문이다.

use crate::{
    model::{Session, Space},
    pty::PtySession,
    vtscreen::VtScreen,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

/// 이보다 많아지면 나란히 두지 않고 탭으로 바꾼다.
pub const MAX_SPLIT: usize = 3;

pub enum Mode {
    Idle,
    Panes,
    Picker,
    NewSpace,
}

/// 마우스 클릭이 무엇을 가리키는가.
pub enum Hit {
    /// 세션에 포커스
    Focus(usize),
    /// 세션 닫기
    Close(usize),
    /// 공간 선택
    Space(usize),
    /// 새 공간 추가
    AddSpace,
    /// 새 세션 추가 (+)
    AddSession,
}

pub struct App {
    /// 띄울 수 있는 에이전트 (id, 표시명)
    pub agents: Vec<(String, String)>,

    pub spaces: Vec<Space>,
    pub active_space: usize,
    pub sessions: Vec<Session>,
    /// 포커스된 세션 (sessions 의 전역 인덱스)
    pub focus: usize,

    pub mode: Mode,
    /// 시작 화면·공간 추가에서 타이핑 중인 문자열
    pub input: String,
    pub status: String,
    pub prefix: bool,
    pub quit: bool,

    /// 사이드바 스크롤 오프셋 (줄 단위)
    pub space_scroll: u16,
    pub agent_scroll: u16,

    // ---- 마지막 렌더의 클릭 대상. 마우스 판정에 쓴다. ----
    pane_hit: Vec<(usize, Rect)>,
    close_hit: Vec<(usize, Rect)>,
    space_hit: Vec<(usize, Rect)>,
    agent_hit: Vec<(usize, Rect)>,
    add_space_hit: Option<Rect>,
    add_session_hit: Option<Rect>,
    /// 사이드바의 두 칸 영역. 휠 스크롤을 어디에 적용할지 가른다.
    spaces_area: Rect,
    agents_area: Rect,
}

impl App {
    pub fn new(agents: &[(&str, &str)]) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            agents: agents.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            spaces: vec![Space::new(cwd)],
            active_space: 0,
            sessions: Vec::new(),
            focus: 0,
            mode: Mode::Idle,
            input: String::new(),
            status: String::new(),
            prefix: false,
            quit: false,
            space_scroll: 0,
            agent_scroll: 0,
            pane_hit: Vec::new(),
            close_hit: Vec::new(),
            space_hit: Vec::new(),
            agent_hit: Vec::new(),
            add_space_hit: None,
            add_session_hit: None,
            spaces_area: Rect::ZERO,
            agents_area: Rect::ZERO,
        }
    }

    pub fn active_path(&self) -> PathBuf {
        self.spaces
            .get(self.active_space)
            .map(|s| s.path.clone())
            .unwrap_or_default()
    }

    /// 지금 보이는 세션들 — 선택된 공간에 속한 것만.
    ///
    /// 공간을 바꾸면 다른 공간의 세션은 **죽이지 않고 숨긴다.** 돌아오면 그대로다.
    pub fn visible(&self) -> Vec<usize> {
        (0..self.sessions.len())
            .filter(|&i| self.sessions[i].space == self.active_space)
            .collect()
    }

    /// 탭 모드인가 — 보이는 세션이 MAX_SPLIT 을 넘는가.
    pub fn tabbed(&self) -> bool {
        self.visible().len() > MAX_SPLIT
    }

    // ---------- 공간 ----------

    pub fn add_space(&mut self, path: &str) -> Result<(), String> {
        let p = PathBuf::from(path.trim());
        if !p.is_dir() {
            return Err(format!("디렉터리가 아니다: {}", p.display()));
        }
        let p = p.canonicalize().unwrap_or(p);
        if let Some(i) = self.spaces.iter().position(|s| s.path == p) {
            self.select_space(i);
            return Ok(());
        }
        self.spaces.push(Space::new(p));
        self.select_space(self.spaces.len() - 1);
        Ok(())
    }

    /// 공간을 고른다. 그 공간의 세션이 보이고, 포커스도 그 안으로 옮긴다.
    pub fn select_space(&mut self, i: usize) {
        if i >= self.spaces.len() {
            return;
        }
        self.active_space = i;
        self.agent_scroll = 0;
        let vis = self.visible();
        if !vis.contains(&self.focus) {
            self.focus = vis.first().copied().unwrap_or(0);
        }
        self.mode = if vis.is_empty() { Mode::Idle } else { Mode::Panes };
    }

    // ---------- 세션 ----------

    /// 지금 공간에서 에이전트 하나를 띄운다.
    pub fn spawn_session(&mut self, agent_id: &str, area: Rect, prompt: Option<&str>) {
        let title = self
            .agents
            .iter()
            .find(|(id, _)| id == agent_id)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| agent_id.to_string());

        let n = (self.visible().len() + 1).min(MAX_SPLIT);
        let (h, w) = pane_size(area, n);
        let cwd = self.active_path();

        let mut s = Session::new(agent_id, &title, self.active_space);
        match PtySession::spawn_in(agent_id, h, w, &cwd) {
            Ok(pty) => {
                s.pty = Some(pty);
                s.size = (h, w);
                s.pending = prompt.map(String::from);
                s.started = Some(Instant::now());
                s.seen = (0, Instant::now());
                self.sessions.push(s);
                self.focus = self.sessions.len() - 1;
                self.mode = Mode::Panes;
            }
            Err(e) => self.status = format!("{title} 기동 실패 — {e}"),
        }
    }

    /// 시작 화면의 질문으로 **모든 에이전트**를 한 번에 띄운다.
    pub fn start_round(&mut self, question: &str, area: Rect) {
        let ids: Vec<String> = self.agents.iter().map(|(id, _)| id.clone()).collect();
        for id in ids {
            self.spawn_session(&id, area, Some(question));
        }
        let vis = self.visible();
        self.focus = vis.first().copied().unwrap_or(0);
        if self.status.is_empty() {
            self.status = HINT.into();
        }
    }

    /// 세션을 닫는다. 자식 프로세스도 정리하고 목록에서 뺀다.
    pub fn close_session(&mut self, i: usize) {
        if i >= self.sessions.len() {
            return;
        }
        if let Some(p) = self.sessions[i].pty.as_mut() {
            p.kill();
        }
        self.sessions.remove(i);
        // 뒤 인덱스가 한 칸씩 당겨졌다. 포커스를 보정한다.
        if self.focus > i {
            self.focus -= 1;
        }
        let vis = self.visible();
        if !vis.contains(&self.focus) {
            self.focus = vis.first().copied().unwrap_or(0);
        }
        if vis.is_empty() {
            self.mode = Mode::Idle;
            self.input.clear();
        }
    }

    /// 보이는 세션 안에서 포커스를 옮긴다.
    pub fn cycle_focus(&mut self, delta: i32) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let cur = vis.iter().position(|&i| i == self.focus).unwrap_or(0) as i32;
        let n = vis.len() as i32;
        self.focus = vis[(((cur + delta) % n + n) % n) as usize];
    }

    /// 보이는 세션 중 n 번째로 포커스.
    pub fn focus_nth(&mut self, n: usize) {
        if let Some(&i) = self.visible().get(n) {
            self.focus = i;
        }
    }

    pub fn focused(&mut self) -> Option<&mut PtySession> {
        self.sessions.get_mut(self.focus).and_then(|s| s.pty.as_mut())
    }

    /// 살아 있는 세션의 출력을 먹이고, 준비된 세션에 프롬프트를 넣는다.
    pub fn tick(&mut self) {
        for s in self.sessions.iter_mut() {
            let before = s.pty.as_ref().map(|p| p.rx_bytes).unwrap_or(0);
            if let Some(p) = s.pty.as_mut() {
                p.pump();
            }
            let after = s.pty.as_ref().map(|p| p.rx_bytes).unwrap_or(0);
            s.refresh_subs(after != before);
            if s.pending.is_some() && s.ready_to_inject() {
                let text = s.pending.take().unwrap_or_default();
                if let Some(p) = s.pty.as_mut() {
                    let _ = p.write(text.as_bytes());
                    // 개행은 살짝 뒤에. 붙여 보내면 입력창이 아직 다 안 그려진
                    // 에이전트에서 첫 글자가 잘린다.
                    std::thread::sleep(Duration::from_millis(120));
                    let _ = p.write(&[13]);
                }
            }
        }
    }

    /// 화면 크기가 바뀌면 각 PTY 도 자기 칸 크기로 맞춘다.
    pub fn sync_sizes(&mut self, area: Rect) {
        let n = if self.tabbed() { 1 } else { self.visible().len() };
        let (h, w) = pane_size(area, n);
        for i in self.visible() {
            let s = &mut self.sessions[i];
            if s.size != (h, w) {
                if let Some(p) = s.pty.as_mut() {
                    let _ = p.resize(h, w);
                    s.size = (h, w);
                }
            }
        }
    }

    // ---------- 마우스 ----------

    pub fn hit_test(&self, x: u16, y: u16) -> Option<Hit> {
        if let Some(r) = &self.add_space_hit {
            if contains(r, x, y) {
                return Some(Hit::AddSpace);
            }
        }
        if let Some(r) = &self.add_session_hit {
            if contains(r, x, y) {
                return Some(Hit::AddSession);
            }
        }
        // 닫기 버튼이 패널 영역 안에 있으므로 먼저 본다.
        for (i, r) in &self.close_hit {
            if contains(r, x, y) {
                return Some(Hit::Close(*i));
            }
        }
        for (i, r) in &self.space_hit {
            if contains(r, x, y) {
                return Some(Hit::Space(*i));
            }
        }
        for (i, r) in &self.agent_hit {
            if contains(r, x, y) {
                return Some(Hit::Focus(*i));
            }
        }
        for (i, r) in &self.pane_hit {
            if contains(r, x, y) {
                return Some(Hit::Focus(*i));
            }
        }
        None
    }

    /// 휠 스크롤. 커서가 놓인 칸만 움직인다.
    pub fn scroll(&mut self, x: u16, y: u16, delta: i32) {
        let target = if contains(&self.spaces_area, x, y) {
            Some((&mut self.space_scroll, self.spaces.len() + 1))
        } else if contains(&self.agents_area, x, y) {
            let n = self.visible().len();
            Some((&mut self.agent_scroll, n))
        } else {
            None
        };
        let Some((off, total)) = target else { return };
        let max = total.saturating_sub(1) as u16;
        *off = ((*off as i32 + delta).clamp(0, max as i32)) as u16;
    }

    // ---------- 렌더 ----------

    pub fn draw(&mut self, f: &mut Frame) {
        match self.mode {
            Mode::Idle => self.draw_idle(f),
            Mode::Panes => self.draw_panes(f),
            Mode::Picker => {
                self.draw_panes(f);
                self.draw_picker(f);
            }
            Mode::NewSpace => {
                self.draw_panes(f);
                self.draw_new_space(f);
            }
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
        f.render_widget(
            Paragraph::new(self.active_path().to_string_lossy().into_owned())
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
        put_cursor(f, inner, &self.input);
    }

    fn draw_panes(&mut self, f: &mut Frame) {
        let area = f.area();
        let vert = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(3), Constraint::Length(1)])
            .split(area);

        // 최상단은 첫 질문이 아니라 **현재 공간의 경로**다.
        // 질문은 각 에이전트 화면 안에 이미 남아 있고, 여기서 늘 필요한 정보는
        // 「지금 어디서 돌고 있는가」다.
        f.render_widget(
            Paragraph::new(self.active_path().to_string_lossy().into_owned())
                .style(Style::default().add_modifier(Modifier::BOLD))
                .block(Block::default().borders(Borders::BOTTOM)),
            vert[0],
        );

        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(crate::sidebar::WIDTH), Constraint::Min(20)])
            .split(vert[1]);
        crate::sidebar::draw(f, main[0], self);

        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(3)])
            .split(main[1]);
        self.draw_tabbar(f, right[0]);
        self.draw_bodies(f, right[1]);

        let hint = if self.prefix {
            "Ctrl+] 눌림 — n 새 질문 · q 종료 · 1..9 포커스".to_string()
        } else {
            self.status.clone()
        };
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            vert[2],
        );
    }

    /// 탭 줄. 세션 칩들과 끝의 `+`.
    ///
    /// 탭 모드가 아니어도 그린다 — `+` 가 늘 같은 자리에 있어야 찾기 쉽다.
    fn draw_tabbar(&mut self, f: &mut Frame, area: Rect) {
        self.add_session_hit = None;
        let vis = self.visible();
        let tabbed = vis.len() > MAX_SPLIT;

        let mut spans: Vec<Span> = Vec::new();
        let mut x = area.x;
        for &i in &vis {
            let s = &self.sessions[i];
            let focused = i == self.focus;
            let label = format!(" {} ", s.title);
            let w = UnicodeWidthStr::width(label.as_str()) as u16;
            let style = if focused {
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
            // 탭 모드에서는 이 칩이 유일한 전환 수단이다.
            if tabbed && x + w <= area.right() {
                self.pane_hit.push((i, Rect { x, y: area.y, width: w, height: 1 }));
            }
            x += w + 1;
        }
        // 새 세션 버튼
        if x + 3 <= area.right() {
            spans.push(Span::styled(
                "[+]",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
            self.add_session_hit = Some(Rect { x, y: area.y, width: 3, height: 1 });
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_bodies(&mut self, f: &mut Frame, area: Rect) {
        let vis = self.visible();
        self.close_hit.clear();
        if vis.is_empty() {
            f.render_widget(
                Paragraph::new("세션이 없다. [+] 로 에이전트를 띄운다.")
                    .style(Style::default().fg(Color::DarkGray)),
                area,
            );
            return;
        }

        // 넷 이상이면 포커스된 하나만 크게. 셋 이하면 나란히.
        let shown: Vec<usize> = if vis.len() > MAX_SPLIT {
            vec![if vis.contains(&self.focus) { self.focus } else { vis[0] }]
        } else {
            self.pane_hit.clear();
            vis.clone()
        };
        let n = shown.len().max(1);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, n as u32); n])
            .split(area);

        for (slot, &i) in shown.iter().enumerate() {
            let s = &self.sessions[i];
            let a = cols[slot];
            let focused = i == self.focus;
            let border = if focused { Color::Cyan } else { Color::DarkGray };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(if focused { BorderType::Thick } else { BorderType::Plain })
                .border_style(Style::default().fg(border))
                .title(format!(" {} ", s.title));
            let inner = block.inner(a);
            f.render_widget(block, a);

            if a.width >= 6 {
                let btn = Rect { x: a.right().saturating_sub(4), y: a.y, width: 3, height: 1 };
                f.render_widget(
                    Paragraph::new("[x]")
                        .style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray })),
                    btn,
                );
                self.close_hit.push((i, btn));
            }
            if vis.len() <= MAX_SPLIT {
                self.pane_hit.push((i, a));
            }

            if let Some(p) = s.pty.as_ref() {
                let screen = p.screen();
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
    }

    /// `+` 를 눌렀을 때의 에이전트 선택 상자.
    fn draw_picker(&self, f: &mut Frame) {
        let area = f.area();
        let h = self.agents.len() as u16 + 2;
        let r = center(area, 40, h);
        f.render_widget(Clear, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .title(" 어떤 에이전트를 띄울까 ");
        let inner = block.inner(r);
        f.render_widget(block, r);
        let lines: Vec<Line> = self
            .agents
            .iter()
            .enumerate()
            .map(|(i, (_, t))| {
                Line::from(vec![
                    Span::styled(format!(" {} ", i + 1), Style::default().fg(Color::Green)),
                    Span::raw(t.clone()),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// 새 공간 경로 입력 상자.
    fn draw_new_space(&self, f: &mut Frame) {
        let area = f.area();
        let r = center(area, area.width.min(70), 3);
        f.render_widget(Clear, r);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green))
            .title(" 새 공간 경로 (Enter 추가 · Esc 취소) ");
        let inner = block.inner(r);
        f.render_widget(block, r);
        f.render_widget(Paragraph::new(self.input.as_str()), inner);
        put_cursor(f, inner, &self.input);
    }

    // 사이드바가 자기 영역과 클릭 대상을 등록한다.
    pub(crate) fn set_spaces_area(&mut self, r: Rect) {
        self.spaces_area = r;
        self.space_hit.clear();
        self.add_space_hit = None;
    }
    pub(crate) fn set_agents_area(&mut self, r: Rect) {
        self.agents_area = r;
        self.agent_hit.clear();
    }
    pub(crate) fn push_space_hit(&mut self, i: usize, r: Rect) {
        self.space_hit.push((i, r));
    }
    pub(crate) fn push_agent_hit(&mut self, i: usize, r: Rect) {
        self.agent_hit.push((i, r));
    }
    pub(crate) fn set_add_space_hit(&mut self, r: Rect) {
        self.add_space_hit = Some(r);
    }
}

pub const HINT: &str =
    "클릭 이동 · [x] 닫기 · [+] 세션 추가 · Alt+←/→ · Ctrl+] 다음 n 새 질문, q 종료";

fn contains(r: &Rect, x: u16, y: u16) -> bool {
    r.width > 0 && r.height > 0 && x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn center(area: Rect, w: u16, h: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

/// 입력 끝에 커서를 둔다.
///
/// 문자 수가 아니라 **표시 폭**으로 센다. 한글은 한 글자가 두 칸을 차지하므로
/// chars().count() 로 재면 커서가 글자 수만큼만 가서 어긋난다.
fn put_cursor(f: &mut Frame, inner: Rect, text: &str) {
    let len = UnicodeWidthStr::width(text) as u16;
    let width = inner.width.max(1);
    f.set_cursor_position((
        (inner.x + len % width).min(inner.right().saturating_sub(1)),
        (inner.y + len / width).min(inner.bottom().saturating_sub(1)),
    ));
}

/// 패널 하나에 줄 PTY 크기. 테두리와 상하단 줄을 뺀 값이다.
fn pane_size(area: Rect, n: usize) -> (u16, u16) {
    let n = n.max(1) as u16;
    let usable = area.width.saturating_sub(crate::sidebar::WIDTH);
    let w = (usable / n).saturating_sub(2).max(20);
    // 상단 경로 2줄 + 탭 1줄 + 상태 1줄 + 테두리 2줄
    let h = area.height.saturating_sub(6).max(6);
    (h, w)
}
