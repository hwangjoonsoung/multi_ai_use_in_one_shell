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
use unicode_width::UnicodeWidthStr;

pub struct Pane {
    pub id: String,
    pub title: String,
    pub session: Option<PtySession>,
    /// 마지막으로 맞춰둔 크기. 달라졌을 때만 resize 한다.
    size: (u16, u16),
    /// 아직 넣지 못한 프롬프트. 이 칸이 준비되면 넣는다.
    pending: Option<String>,
    /// 기동 시각. 최대 대기 시간을 재는 기준.
    started: Option<Instant>,
    /// 마지막으로 관측한 수신 바이트 수와 그 시각. 출력이 멎으면 준비된 것으로 본다.
    seen: (usize, Instant),
}

impl Pane {
    fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            session: None,
            size: (0, 0),
            pending: None,
            started: None,
            seen: (0, Instant::now()),
        }
    }

    fn status(&self) -> &'static str {
        self.state().label()
    }

    /// 지금 상태. 최근 출력 여부로 «출력 중»과 «멎음»을 가른다.
    pub fn state(&self) -> PaneState {
        match &self.session {
            None => PaneState::Idle,
            Some(s) if s.finished() => PaneState::Exited,
            Some(_) if self.pending.is_some() => PaneState::Starting,
            Some(_) => {
                if self.seen.1.elapsed() < Duration::from_millis(800) {
                    PaneState::Working
                } else {
                    PaneState::Quiet
                }
            }
        }
    }

    /// 프롬프트를 넣어도 되는 상태인가.
    ///
    /// 에이전트마다 기동 속도가 크게 다르다(agy 는 모델 조회 때문에 느리다).
    /// 고정 지연으로는 빠른 쪽엔 낭비, 느린 쪽엔 입력이 삼켜진다. 그래서
    /// **출력이 한 번 나온 뒤 잠잠해지면** 준비된 것으로 본다.
    fn ready_to_inject(&mut self) -> bool {
        let Some(s) = self.session.as_ref() else { return false };
        let Some(started) = self.started else { return false };
        let rx = s.rx_bytes;
        if rx != self.seen.0 {
            self.seen = (rx, Instant::now());
            return false;
        }
        // 아무것도 못 받았으면 아직 뜨는 중이다.
        if rx == 0 {
            return started.elapsed() >= Duration::from_secs(20);
        }
        // 출력이 멎고 600ms 지났거나, 20초를 넘겼으면 넣는다.
        self.seen.1.elapsed() >= Duration::from_millis(600)
            || started.elapsed() >= Duration::from_secs(20)
    }
}

/// 패널 상태. **관측 가능한 사실만** 표현한다.
///
/// "무엇을 하는 중인지"는 우리가 알 수 없다. 프로세스가 살아 있는지, 최근에
/// 출력이 있었는지만 안다. 그 이상을 추측해 표시하면 사용자를 오도한다.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaneState {
    /// 아직 안 띄웠다
    Idle,
    /// 띄웠고 프롬프트 주입을 기다리는 중
    Starting,
    /// 최근에 출력이 있었다
    Working,
    /// 살아 있지만 잠잠하다
    Quiet,
    /// 프로세스가 끝났다
    Exited,
}

impl PaneState {
    pub fn label(self) -> &'static str {
        match self {
            PaneState::Idle => "대기",
            PaneState::Starting => "기동 중",
            PaneState::Working => "출력 중",
            PaneState::Quiet => "멎음",
            PaneState::Exited => "종료",
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
    /// 사이드바에 보일 작업 공간 정보
    pub workspace_name: String,
    pub workspace_path: String,
    pub branch: String,
}

impl App {
    pub fn new(agents: &[(&str, &str)]) -> Self {
        let mut me = Self {
            panes: agents.iter().map(|(id, t)| Pane::new(id, t)).collect(),
            focus: 0,
            mode: Mode::Idle,
            input: String::new(),
            question: String::new(),
            status: String::new(),
            prefix: false,
            quit: false,
            workspace_name: String::new(),
            workspace_path: String::new(),
            branch: String::new(),
        };
        me.detect_workspace();
        me
    }

    /// 현재 디렉터리 이름과 git 브랜치를 읽는다.
    ///
    /// 브랜치는 .git/HEAD 를 직접 읽는다. git 프로세스를 띄우면 매 프레임
    /// 비용이 들고, 우리가 필요한 건 한 줄뿐이다.
    fn detect_workspace(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        self.workspace_name = cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "-".into());
        self.workspace_path = cwd.to_string_lossy().into_owned();
        self.branch = read_branch(&cwd).unwrap_or_default();
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
                    p.pending = Some(question.to_string());
                    p.started = Some(Instant::now());
                    p.seen = (0, Instant::now());
                }
                Err(e) => failed.push(format!("{}: {}", p.title, e)),
            }
        }
        self.status = if failed.is_empty() {
            "Alt+←/→ 패널 이동 · Alt+1/2/3 직접 선택 · Ctrl+] 다음 n 새 질문, q 종료".into()
        } else {
            format!("기동 실패 — {}", failed.join(", "))
        };
    }

    /// 살아 있는 세션의 출력을 먹이고, 준비된 칸에 프롬프트를 넣는다.
    pub fn tick(&mut self) {
        for p in self.panes.iter_mut() {
            if let Some(s) = p.session.as_mut() {
                s.pump();
            }
            if p.pending.is_some() && p.ready_to_inject() {
                let text = p.pending.take().unwrap_or_default();
                if let Some(s) = p.session.as_mut() {
                    let _ = s.write(text.as_bytes());
                    // 개행은 살짝 뒤에 보낸다. 붙여 보내면 입력창이 아직 다 안
                    // 그려진 에이전트에서 첫 글자가 잘린다.
                    std::thread::sleep(Duration::from_millis(120));
                    let _ = s.write(&[13]);
                }
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
        //
        // 문자 수가 아니라 **표시 폭**으로 세야 한다. 한글은 한 글자가 두 칸을
        // 차지하므로 chars().count() 로 재면 커서가 글자 수만큼만 가서 어긋난다.
        let len = UnicodeWidthStr::width(self.input.as_str()) as u16;
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

        // 왼쪽 사이드바 + 오른쪽 패널들
        let main = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(crate::sidebar::WIDTH),
                Constraint::Min(20),
            ])
            .split(vert[1]);
        crate::sidebar::draw(f, main[0], self);

        let n = self.panes.len().max(1);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, n as u32); n])
            .split(main[1]);

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
            "Ctrl+] 눌림 — n 새 질문 · q 종료 · 1/2/3 포커스".to_string()
        } else {
            self.status.clone()
        };
        f.render_widget(
            Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
            vert[2],
        );
    }
}

/// .git/HEAD 에서 현재 브랜치 이름을 읽는다.
fn read_branch(dir: &std::path::Path) -> Option<String> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let head = d.join(".git").join("HEAD");
        if head.is_file() {
            let text = std::fs::read_to_string(head).ok()?;
            let t = text.trim();
            return Some(match t.strip_prefix("ref: refs/heads/") {
                Some(name) => name.to_string(),
                // 분리된 HEAD 면 커밋 해시 앞부분만 보여준다.
                None => format!("({})", &t[..t.len().min(7)]),
            });
        }
        cur = d.parent();
    }
    None
}

/// 패널 하나에 줄 PTY 크기. 테두리와 상하단 줄을 뺀 값이다.
fn pane_size(area: Rect, n: usize) -> (u16, u16) {
    let n = n.max(1) as u16;
    let usable = area.width.saturating_sub(crate::sidebar::WIDTH);
    let w = (usable / n).saturating_sub(2).max(20);
    let h = area.height.saturating_sub(5).max(6);
    (h, w)
}
