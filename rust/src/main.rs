//! multi_ai_cli — Claude / Codex / Gemini(agy) 를 한 화면에서 다루는 에이전트 멀티플렉서.
//!
//! R1  PTY 하나를 화면에 붙인다 (통과)
//! R2  참여자마다 자기 칸. 포커스된 칸으로만 키가 간다  ← 현재
//!
//! 사용법
//!   multi_ai_cli               쓸 수 있는 참여자를 전부 띄우고 바로 패널로 들어간다
//!   multi_ai_cli --solo <에이전트>   한 에이전트만 전체 화면으로 (R1 확인용)
//!   multi_ai_cli --converge [@수렴자] <안건>   구조화 교차검증 → REPORT.md
//!   multi_ai_cli --rooms       저장된 방 목록
//!   multi_ai_cli --show <ID>   방 기록 보기
//!   multi_ai_cli --which       각 에이전트를 어떻게 띄우는지 확인
//!   multi_ai_cli --quote <에이전트> [경로]  인용 전달이 무엇을 꺼내는지 확인
//!   multi_ai_cli --trust       현재 디렉터리를 각 에이전트에 신뢰 등록
//!   multi_ai_cli --selftest    PTY+VT 파이프라인 자동 점검
//!   multi_ai_cli --probe       셸과 «cd 따라가기» 가 이 환경에서 되는지 점검

mod app;
mod modal;
mod model;
mod subagents;
mod converge;
mod room;
mod sidebar;
mod pty;
mod relay;
mod trust;
mod vtscreen;

use anyhow::Result;
use app::{App, Hit, Mode};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    event::{
        DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, MouseButton,
        MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};

/// 기본 참여자. R3 에서 설치 탐색 결과로 대체한다.
const AGENTS: &[(&str, &str)] = &[
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("agy", "Gemini"),
];

type Term = Terminal<CrosstermBackend<io::Stdout>>;

/// println! 대신 쓴다.
///
/// 출력을 `head` 같은 곳으로 파이프하면 상대가 먼저 닫아 쓰기가 실패하는데,
/// println! 은 그때 패닉한다. 크래시처럼 보이지만 정상적인 상황이므로 조용히 넘긴다.
macro_rules! outln {
    () => { { use std::io::Write; let _ = writeln!(std::io::stdout()); } };
    ($($arg:tt)*) => { {
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    } };
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--selftest") => return selftest(),
        Some("--probe") => return probe(),
        Some("--complete") => {
            // 경로 탭 완성을 화면 없이 확인한다. `~` 확장도 여기서 드러난다.
            let mut app = App::new(AGENTS);
            for raw in args.iter().skip(1) {
                app.input = raw.clone();
                app.status.clear();
                app.complete_path();
                outln!("  {raw:<28} -> {}", app.input);
                if !app.status.is_empty() {
                    outln!("  {:<28}    {}", "", app.status);
                }
            }
            return Ok(());
        }
        Some("--subs") => {
            // 서브에이전트 명부를 화면에서 읽어내는지 확인한다.
            //
            // 프로세스 트리로는 못 본다는 것이 실측으로 확인됐다 — 서브에이전트
            // 둘을 끝까지 돌려도 자손 프로세스는 늘지 않았다. 에이전트가 자기
            // 화면 아래에 그리는 명부가 유일한 관측 지점이다.
            let agent = args.get(1).cloned().unwrap_or_else(|| "claude".into());
            let prompt = args.get(2).cloned();
            let secs: u64 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(60);
            let mut s = pty::PtySession::spawn(&agent, 40, 120)?;
            let mut sent = false;
            for i in 0..secs {
                std::thread::sleep(Duration::from_secs(1));
                s.pump();
                if !sent && i >= 8 {
                    if let Some(p) = &prompt {
                        let _ = s.write(p.as_bytes());
                        std::thread::sleep(Duration::from_millis(150));
                        let _ = s.write(&[13]);
                        outln!("-- 프롬프트 주입 --");
                    }
                    sent = true;
                }
                let subs = subagents::scan(&s.screen());
                if !subs.is_empty() {
                    outln!("{i:3}s  서브에이전트 {}개", subs.len());
                    for b in &subs {
                        outln!("      {} {} [{}]", b.kind, b.desc, if b.running { "도는 중" } else { "끝" });
                    }
                }
            }
            return Ok(());
        }
        Some("--quote") => {
            // 인용 전달이 무엇을 꺼내는지 화면 없이 확인한다.
            //
            // JSONL 은 비공개 내부 규약이라 CLI 가 올라가면 조용히 깨진다.
            // 그때 "왜 화면에서만 가져오지" 를 추측하지 않으려고 남긴다.
            let agent = args.get(1).cloned().unwrap_or_else(|| "claude".into());
            let cwd = match args.get(2) {
                Some(p) => app::expand_tilde(p),
                None => std::env::current_dir()?,
            };
            outln!("  대상: {agent}  경로: {}", cwd.display());
            match relay::last_answer(&agent, &cwd) {
                Some(q) => {
                    outln!(
                        "  출처: {}  {}자{}",
                        q.origin.label(),
                        q.text.chars().count(),
                        if q.truncated { " (앞부분 생략)" } else { "" }
                    );
                    outln!("  ── 앞 400자 ──");
                    let head: String = q.text.chars().take(400).collect();
                    for l in head.lines() {
                        outln!("  {l}");
                    }
                }
                None => outln!("  세션 기록에서 찾지 못했다 — 실제로는 화면 버퍼로 떨어진다"),
            }
            return Ok(());
        }
        Some("--rooms") => {
            let rooms = room::list();
            if rooms.is_empty() {
                outln!("저장된 방이 없다.");
            }
            for (id, dir) in rooms {
                let n = room::read_transcript(&dir.join("transcript.md"), room::next_id_of(&dir)).len();
                outln!("  {id:<16} 메시지 {n}건  {}", dir.display());
            }
            return Ok(());
        }
        Some("--show") => {
            let Some(id) = args.get(1) else {
                eprintln!("사용법: multi_ai_cli --show <방 ID>   (--rooms 로 목록 확인)");
                return Ok(());
            };
            let dir = room::rooms_dir().join(id);
            let msgs = room::read_transcript(&dir.join("transcript.md"), room::next_id_of(&dir));
            if msgs.is_empty() {
                outln!("기록이 없거나 방을 찾지 못했다: {id}");
            }
            for m in msgs {
                // 복구분은 그렇다고 밝힌다. 정상 복원인 척하지 않는다.
                let mark = if m.suspect { "  [복원 의심]" } else { "" };
                outln!("── [{}] {}{}", m.id, m.sender, mark);
                outln!("{}", m.body);
                outln!("");
            }
            return Ok(());
        }
        Some("--converge") => {
            let subject = args[1..].join(" ");
            if subject.trim().is_empty() {
                eprintln!("사용법: multi_ai_cli --converge <안건>");
                eprintln!("        [@수렴자] 를 앞에 붙이면 그 참여자는 검토에서 빠진다");
                return Ok(());
            }
            return run_converge(&subject);
        }
        Some("--trust") => {
            // 보안 결정이라 자동으로 하지 않는다. 이 명령을 직접 실행할 때만 기록한다.
            let ws = std::env::current_dir()?;
            outln!("워크스페이스를 신뢰 목록에 등록한다: {}", ws.display());
            outln!("(대화상자에서 '예' 를 누르는 것과 같은 일이다)");
            outln!("");
            for r in trust::trust_workspace(&ws) {
                outln!("  {:<8} {}", r.agent, r.outcome);
            }
            return Ok(());
        }
        Some("--which") => {
            // 각 에이전트를 어떻게 띄울지 보여준다. 기동 실패를 진단할 때 쓴다.
            for (id, title) in AGENTS {
                match pty::resolve_agent(id) {
                    Some((exe, args)) => {
                        outln!("  {title:<16} {}", exe.display());
                        if !args.is_empty() {
                            outln!("  {:<16} 선행 인자 {args:?}", "");
                        }
                    }
                    None => outln!("  {title:<16} 찾지 못함 — 이 참여자는 비활성"),
                }
            }
            return Ok(());
        }
        Some("--solo") => {
            let agent = args.get(1).cloned().unwrap_or_else(|| "claude".into());
            return with_terminal(|t| solo(t, &agent));
        }
        _ => {}
    }
    with_terminal(run)
}

/// 터미널을 raw·대체 화면으로 바꾸고 끝나면 반드시 되돌린다.
fn with_terminal<F>(body: F) -> Result<()>
where
    F: FnOnce(&mut Term) -> Result<()>,
{
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;

    // 바깥 터미널에 **키를 구분해서 달라**고 청한다.
    //
    // 이게 없으면 Shift+Enter 가 그냥 CR 로 와서 Enter 와 구분되지 않는다.
    // 지원하는 터미널에서만 켠다 — 안 되는 터미널에 밀어 넣으면 켜졌다고
    // 착각한 채 응답을 기다린다. macOS 기본 터미널은 지원하지 않으므로
    // 그쪽에서는 Option+Enter 가 대신이다(ESC CR 로 오고 우리가 알아본다).
    let enhanced = matches!(crossterm::terminal::supports_keyboard_enhancement(), Ok(true));
    if enhanced {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let result = body(&mut term);

    if enhanced {
        let _ = execute!(term.backend_mut(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    execute!(term.backend_mut(), DisableMouseCapture, LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

// ---------- R2 본 흐름 ----------

fn run(term: &mut Term) -> Result<()> {
    let mut app = App::new(AGENTS);

    // 질문을 먼저 묻지 않는다. 크기를 알자마자 참여자를 띄우고 패널로 들어간다.
    // 크기가 필요해서 App::new 가 아니라 여기서 한다.
    let a = term.size()?;
    app.bootstrap(ratatui::layout::Rect::new(0, 0, a.width, a.height));

    loop {
        app.tick();
        term.draw(|f| app.draw(f))?;
        if app.quit {
            return Ok(());
        }
        let area = term.size()?;
        app.sync_sizes(ratatui::layout::Rect::new(0, 0, area.width, area.height));

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) => {
                keylog(&k);
                // Windows 는 누를 때와 뗄 때를 모두 보낸다. 그대로 넘기면
                // 한 번 친 키가 두 번 입력된다.
                if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    continue;
                }
                match app.mode {
                    Mode::Panes => on_key_panes(&mut app, k)?,
                    Mode::Picker => on_key_picker(&mut app, k, term)?,
                    Mode::NewSpace => on_key_new_space(&mut app, k)?,
                    Mode::Broadcast => on_key_broadcast(&mut app, k)?,
                    Mode::Relay => on_key_relay(&mut app, k)?,
                    Mode::RenameTab => on_key_rename(&mut app, k)?,
                }
            }
            Event::Mouse(m) => on_mouse(&mut app, m),
            Event::Resize(_, _) => {
                let a = term.size()?;
                app.sync_sizes(ratatui::layout::Rect::new(0, 0, a.width, a.height));
            }
            _ => {}
        }
    }
}

/// `+` 선택 상자 — 어떤 에이전트를 띄울지 고른다.
fn on_key_picker(app: &mut App, k: KeyEvent, term: &mut Term) -> Result<()> {
    match k.code {
        KeyCode::Esc => app.mode = Mode::Panes,
        KeyCode::Char(c @ '1'..='9') => {
            let i = c as usize - '1' as usize;
            let a = term.size()?;
            app.mode = Mode::Panes;
            app.pick(i, ratatui::layout::Rect::new(0, 0, a.width, a.height));
        }
        _ => {}
    }
    Ok(())
}

/// 새 공간 경로 입력.
fn on_key_new_space(app: &mut App, k: KeyEvent) -> Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.input.clear();
            app.mode = Mode::Panes;
        }
        KeyCode::Enter => {
            let path = app.input.trim().to_string();
            match app.add_space(&path) {
                Ok(()) => {
                    app.input.clear();
                    app.status = app::HINT.into();
                }
                // 경로가 틀렸으면 입력을 지우지 않는다. 고쳐 쓰게 둔다.
                Err(e) => app.status = e,
            }
        }
        // 셸처럼 Tab 으로 경로를 채운다. 공간은 디렉터리라 후보도 디렉터리뿐이다.
        KeyCode::Tab => app.complete_path(),
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => app.input.push(c),
        _ => {}
    }
    Ok(())
}

/// 모두에게 묻기 상자.
fn on_key_broadcast(app: &mut App, k: KeyEvent) -> Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.input.clear();
            app.mode = Mode::Panes;
        }
        KeyCode::Enter => {
            let q = app.input.trim().to_string();
            if !q.is_empty() {
                app.broadcast(&q);
            }
            app.input.clear();
            app.mode = Mode::Panes;
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => app.input.push(c),
        _ => {}
    }
    Ok(())
}

/// 탭 이름 고치기 — 한 줄 편집.
fn on_key_rename(app: &mut App, k: KeyEvent) -> Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.input.clear();
            app.cancel_rename();
        }
        KeyCode::Enter => {
            let name = app.input.trim().to_string();
            app.commit_rename(&name);
            app.input.clear();
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => app.input.push(c),
        _ => {}
    }
    Ok(())
}

/// 인용 전달 상자 — 한 문장만 받는다.
///
/// 인용문은 상자를 열 때 이미 꺼내 뒀다. 여기서 받는 건 «넌 어떻게 생각해?»
/// 한 줄이고, 그게 인용문 뒤에 붙어 나머지 칸으로 간다.
fn on_key_relay(app: &mut App, k: KeyEvent) -> Result<()> {
    match k.code {
        KeyCode::Esc => {
            app.input.clear();
            app.cancel_relay();
        }
        KeyCode::Enter => {
            let q = app.input.trim().to_string();
            // 문장이 비어도 보낸다. 인용만 넘기고 싶을 수 있다.
            // 대상이 하나도 없으면 relay 가 이유를 남기고 상자를 지킨다.
            if app.relay(&q) {
                app.input.clear();
                app.mode = Mode::Panes;
            }
        }
        // 문장 칸과 대상 칩 사이를 오간다.
        KeyCode::Tab => app.cycle_relay_focus(1),
        KeyCode::BackTab => app.cycle_relay_focus(-1),
        KeyCode::Right if app.relay_focus > 0 => app.cycle_relay_focus(1),
        KeyCode::Left if app.relay_focus > 0 => app.cycle_relay_focus(-1),
        KeyCode::Down => app.cycle_relay_focus(1),
        KeyCode::Up => app.cycle_relay_focus(-1),
        // Space 는 칩 위에서만 켜기/끄기다. 문장 칸에서는 띄어쓰기다.
        KeyCode::Char(' ') if app.relay_focus > 0 => app.toggle_relay_target(),
        KeyCode::Backspace => {
            app.relay_focus = 0;
            app.input.pop();
        }
        // 칩에 있을 때 글자를 치면 문장 칸으로 돌아온다. 친 글자는 살린다.
        KeyCode::Char(c) => {
            app.relay_focus = 0;
            app.input.push(c);
        }
        _ => {}
    }
    Ok(())
}

/// 패널 화면 — 키는 포커스된 자식에게 그대로 간다.
///
/// 우리 조작은 프리픽스(Ctrl+]) 뒤에 온다. 그러지 않으면 에이전트가 쓰는 키를
/// 우리가 가로채게 되고, 그 순간 "직접 쓰는 것과 같다"는 전제가 깨진다.
fn on_key_panes(app: &mut App, k: KeyEvent) -> Result<()> {
    if app.prefix {
        app.prefix = false;
        match k.code {
            KeyCode::Char(c @ '1'..='9') => app.focus_nth(c as usize - '1' as usize),
            // 새 질문. 예전엔 시작 화면으로 돌아갔지만 그 화면이 없어졌고,
            // 하려던 일(전원에게 묻기)은 Ctrl+A 와 같다.
            KeyCode::Char('n') => {
                app.input.clear();
                app.mode = Mode::Broadcast;
            }
            KeyCode::Char('q') => app.quit = true,
            // t 터미널 추가 — [+] 클릭과 같다.
            KeyCode::Char('t') => {
                let a = app.last_area();
                app.spawn_terminal(a);
            }
            // p 에이전트 선택 상자. 닫은 참여자를 되살릴 때 쓴다.
            KeyCode::Char('p') => app.mode = Mode::Picker,
            // f 이 칸의 마지막 답변을 나머지 칸으로 넘긴다.
            KeyCode::Char('f') => app.prepare_relay(),
            // 프리픽스 뒤의 a 는 Ctrl+A 를 **자식에게** 보낸다.
            // Ctrl+A 자체는 우리가 «모두에게 묻기»로 가져갔기 때문이다.
            KeyCode::Char('a') => {
                if let Some(s) = app.focused() {
                    let _ = s.write(&[0x01]);
                }
            }
            // 프리픽스를 한 번 더 누르면 프리픽스 자체를 자식에게 보낸다.
            c if is_prefix_key(c) => {
                if let Some(s) = app.focused() {
                    let _ = s.write(&[0x1d]);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    // Alt+화살표 / Alt+숫자로 바로 이동한다. 프리픽스보다 빠르고,
    // 에이전트 TUI 들이 Alt+방향키를 거의 쓰지 않아 충돌이 적다.
    if k.modifiers.contains(KeyModifiers::ALT) {
        match k.code {
            KeyCode::Left => {
                app.cycle_focus(-1);
                return Ok(());
            }
            KeyCode::Right => {
                app.cycle_focus(1);
                return Ok(());
            }
            KeyCode::Char(c @ '1'..='9') => {
                app.focus_nth(c as usize - '1' as usize);
                return Ok(());
            }
            _ => {}
        }
    }

    if k.modifiers.contains(KeyModifiers::CONTROL) {
        match k.code {
            c if is_prefix_key(c) => {
                app.prefix = true;
                return Ok(());
            }
            // 모두에게 한 번에 묻는다.
            //
            // 자식들도 Ctrl+A 를 쓸 수 있다(줄 맨 앞으로). 그 기능이 필요하면
            // 프리픽스를 거쳐 Ctrl+] 다음 a 로 보낼 수 있게 열어 뒀다.
            KeyCode::Char('a') => {
                app.input.clear();
                app.mode = Mode::Broadcast;
                return Ok(());
            }
            _ => {}
        }
    }

    if let Some(bytes) = encode_key(&k) {
        // 글자를 친 것만 «직접 쓰기»로 센다. 화살표·Enter 는 대화상자에 답하는
        // 조작이라 대기 중인 질문을 취소시키면 안 된다.
        let typed = matches!(k.code, KeyCode::Char(_)) && !k.modifiers.contains(KeyModifiers::CONTROL);
        let focus = app.focus;
        if let Some(s) = app.focused() {
            let _ = s.write(&bytes);
        }
        if typed {
            if let Some(s) = app.sessions.get_mut(focus) {
                s.user_typed = true;
            }
        }
    }
    Ok(())
}

/// 클릭으로 패널을 고르거나 닫는다.
///
/// 마우스는 우리가 먹는다. 자식에게 넘기면 에이전트가 자기 좌표계로 해석해
/// 엉뚱한 곳을 누른 것이 된다 — 패널마다 원점이 다르기 때문이다.
fn on_mouse(app: &mut App, m: MouseEvent) {
    // 휠은 사이드바 스크롤에 쓴다. 커서가 놓인 칸만 움직인다.
    match m.kind {
        MouseEventKind::ScrollUp => {
            app.scroll(m.column, m.row, -1);
            return;
        }
        MouseEventKind::ScrollDown => {
            app.scroll(m.column, m.row, 1);
            return;
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return,
    }
    match app.hit_test(m.column, m.row) {
        Some(Hit::Close(i)) => app.close_session(i),
        Some(Hit::ShowAll) => app.show_all = true,
        // 탭 칩 — 한 번이면 포커스, 짧은 사이에 두 번이면 이름 고치기.
        Some(Hit::Tab(i)) => app.tab_click(i),
        Some(Hit::Focus(i)) => {
            // 특정 에이전트를 고르면 전체 보기에서 빠져나온다.
            app.show_all = false;
            app.focus = i;
        }
        Some(Hit::Space(i)) => app.select_space(i),
        Some(Hit::AddSpace) => {
            app.input = app.active_path().to_string_lossy().into_owned();
            app.mode = Mode::NewSpace;
        }
        // [+] 는 무엇을 띄울지 묻는다. 기동 때 아무것도 자동으로 뜨지 않으므로
        // 에이전트를 붙이는 길이 여기에 있어야 한다. 터미널만 빠르게 붙이려면
        // Ctrl+] t 가 상자를 건너뛴다.
        Some(Hit::AddSession) => app.mode = Mode::Picker,
        None => {}
    }
}

/// 한 에이전트만 전체 화면으로. R1 검증용으로 남겨둔다.
fn solo(term: &mut Term, agent: &str) -> Result<()> {
    let size = term.size()?;
    let mut session = pty::PtySession::spawn(agent, size.height, size.width)?;
    loop {
        session.pump();
        term.draw(|f| {
            let area = f.area();
            let screen = session.screen();
            f.render_widget(vtscreen::VtScreen::new(&screen), area);
            if !screen.hide_cursor() {
                let (r, c) = screen.cursor_position();
                f.set_cursor_position((area.x + c, area.y + r));
            }
        })?;
        if session.finished() {
            return Ok(());
        }
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(k) => {
                    if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char(']') {
                        return Ok(());
                    }
                    if let Some(b) = encode_key(&k) {
                        session.write(&b)?;
                    }
                }
                Event::Resize(c, r) => session.resize(r, c)?,
                _ => {}
            }
        }
    }
}

/// crossterm 키 이벤트를 터미널이 보내는 바이트열로 바꾼다.
/// `MAI_KEYLOG=<파일>` 이 있으면 받은 키를 그대로 적는다.
///
/// 키 이름은 crossterm 이 터미널·플랫폼마다 다르게 붙인다. 안 먹는 단축키를
/// 두고 추측하는 대신 **무엇이 실제로 들어왔는지** 본다. MAI_DUMP 가 PTY
/// 바이트에 하는 일을 키 쪽에서 하는 것이다.
fn keylog(k: &KeyEvent) {
    let Ok(path) = std::env::var("MAI_KEYLOG") else { return };
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{:?} mods={:?} kind={:?}", k.code, k.modifiers, k.kind);
    }
}

/// Ctrl 과 함께 눌렸을 때 프리픽스로 볼 키인가.
///
/// 터미널이 보내는 바이트는 어디서나 0x1D 하나인데 **crossterm 이 붙이는 이름이
/// 플랫폼마다 다르다.** 유닉스 파서는 0x1C..=0x1F 를 `'4'..'7'` 로 옮기므로
/// Ctrl+] 가 `Char('5')` 로 들어온다(crossterm 0.29 parse.rs:110). Windows 는
/// 콘솔 API 에서 가상 키를 직접 받아 `Char(']')` 로 온다.
///
/// 한쪽만 보던 탓에 macOS 에서는 프리픽스가 통째로 죽어 있었다 — Ctrl+] 다음의
/// q(종료)·1..9(포커스)·a 가 전부 자식에게 흘러들어갔다(실측).
fn is_prefix_key(c: KeyCode) -> bool {
    match c {
        KeyCode::Char(']') => true,
        // 유닉스에서만 받는다. Windows 의 Char('5') 는 진짜 Ctrl+5 다.
        KeyCode::Char('5') => !cfg!(windows),
        _ => false,
    }
}

fn encode_key(k: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let b: Vec<u8> = match k.code {
        KeyCode::Char(c) if ctrl => {
            let up = c.to_ascii_uppercase();
            if up.is_ascii_uppercase() {
                vec![up as u8 - b'A' + 1]
            } else {
                return None;
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        // Shift+Enter / Alt+Enter 는 **줄바꿈**이다. 전송이 아니다.
    //
    // 터미널은 예로부터 Enter 와 Shift+Enter 를 구분하지 않고 둘 다 CR 만
    // 보냈다. 그래서 에이전트들은 «ESC CR» 을 줄바꿈 신호로 받기로 하고 있다
    // (실측: Claude 입력창에서 ESC CR, CSI 13;2u, 역슬래시+CR 셋 다 줄바꿈).
    // 우리가 그 변환을 맡는다.
    KeyCode::Enter if k.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
        b"\x1b\r".to_vec()
    }
    KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        _ => return None,
    };
    Some(b)
}

// ---------- 구조화 수렴 (헤드리스) ----------

/// `/converge` 의 CLI 진입점.
///
/// PTY 를 쓰지 않는다. 대화형에서는 스키마 강제가 안 되기 때문이다.
/// 한 바이너리에 두 실행 모드를 두는 이유가 이것이다.
fn run_converge(raw: &str) -> Result<()> {
    let cfg = room::Config::load();
    let ws = std::env::current_dir()?;

    // 앞에 @이름 이 오면 그 참여자를 수렴자로 보고 검토에서 뺀다.
    let (consolidator, subject) = match raw.strip_prefix('@') {
        Some(rest) => match rest.split_once(char::is_whitespace) {
            Some((who, s)) => (Some(who.to_lowercase()), s.trim().to_string()),
            None => (None, raw.to_string()),
        },
        None => (None, raw.to_string()),
    };

    let reviewers: Vec<(String, String)> = AGENTS
        .iter()
        .filter(|(id, _)| consolidator.as_deref() != Some(*id))
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect();

    outln!("안건: {subject}");
    outln!("검토자: {}", reviewers.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", "));
    match &consolidator {
        Some(c) => outln!("수렴자: {c} (검토에서 제외)"),
        None => outln!("수렴자: 규칙 기반 (모델 호출 없음)"),
    }
    outln!("");

    let mut r = room::Room::create(&ws)?;
    let round = r.start_round();
    let out_dir = r.run_dir(round)?.join("converge");

    let session = converge::Session {
        reviewers,
        workspace: ws,
        out_dir,
        agy_model: Some(&cfg.agy_model),
    };
    let res = session.run(
        &subject,
        &converge::Progress {
            stage: &|m| outln!("  · {m}"),
            done: &|r| outln!("  [{}] {} · 지적 {}건", r.reviewer_name, r.verdict.label(), r.issues.len()),
        },
    )?;

    if let Some(reason) = res.aborted {
        eprintln!();
        eprintln!("  ! {reason}");
        return Ok(());
    }

    let last = res.round2.as_ref().or(res.round1.as_ref());
    if let Some(o) = last {
        outln!("");
        outln!("== 판정 요약 ==");
        for rv in &o.reviews {
            outln!("  {:<18} {}", rv.reviewer_name, rv.verdict.label());
        }
        let mut failed = res.round1.as_ref().map(|x| x.failed.clone()).unwrap_or_default();
        if let Some(x) = &res.round2 {
            failed.extend(x.failed.clone());
        }
        failed.dedup();
        if !failed.is_empty() {
            outln!("  ! PARTIAL — 응답 없음: {}", failed.join(", "));
        }
        outln!("");
        outln!(
            "  합의 {} · 이견 {} · 단독 지적 {} · 미해결 {}",
            o.count(converge::engine::Bucket::Agreed),
            o.count(converge::engine::Bucket::Disputed),
            o.count(converge::engine::Bucket::Solo),
            o.open_questions.len()
        );
        if !o.open_questions.is_empty() {
            outln!("");
            outln!("== 사용자 결정 필요 ==");
            for q in &o.open_questions {
                outln!("  - {q}");
            }
        }
    }
    // 기록은 남기되 터미널에는 요약만 낸다. 전문은 보고서에 있다.
    let _ = r.append("user", "OK", 0, &subject);
    if let Some(rep) = &res.report {
        let _ = r.append("consolidator", "OK", 0, &format!("보고서: {}", rep.display()));
        outln!("");
        outln!("  · 보고서: {}", rep.display());
    }
    outln!("  · 기록: {}", r.transcript().display());
    Ok(())
}

// ---------- 환경 점검 ----------

/// 셸을 띄워 «cd 하면 공간이 따라가는가» 를 화면 없이 판정한다.
///
/// 이 기능은 세 고리가 다 성립해야 돈다 — 셸을 찾고, 자식 pid 를 얻고,
/// 그 프로세스의 cwd 를 읽는 것. 하나라도 끊기면 증상이 «아무 일도 안 일어남»
/// 이라 화면만 봐서는 어디가 문제인지 모른다.
///
/// 특히 Windows 의 PowerShell 은 `Set-Location` 이 셸 안의 위치만 바꾸고 Win32
/// 프로세스 cwd 는 그대로 두는 경우가 있다. 그러면 우리가 읽는 값이 안 바뀐다.
/// 그 판정을 하려고 만든 것이다.
fn probe() -> Result<()> {
    outln!("== 환경 점검 ==");
    outln!("  플랫폼: {}", std::env::consts::OS);

    let Some((exe, args)) = pty::resolve_shell() else {
        outln!("  [FAIL] 셸을 찾지 못했다");
        anyhow::bail!("셸 없음");
    };
    outln!("  셸: {} {args:?}", exe.display());

    let cwd = std::env::current_dir()?;
    outln!("  기동 경로: {}", cwd.display());

    let mut s = pty::PtySession::spawn_shell_in(24, 80, &cwd)?;
    let Some(pid) = s.pid() else {
        outln!("  [FAIL] 자식 pid 를 얻지 못했다");
        anyhow::bail!("pid 없음");
    };
    outln!("  자식 pid: {pid}");

    // 셸이 자리를 잡을 때까지 기다린다. 프롬프트가 나오면 준비된 것이다.
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        s.pump();
    }

    let mut sys = sysinfo::System::new();
    let before = pty::process_cwd(&mut sys, pid);
    match &before {
        Some(p) => outln!("  읽어낸 경로: {}", p.display()),
        None => {
            outln!("  [FAIL] 프로세스 경로를 읽지 못했다");
            outln!("         → 공간이 cd 를 따라갈 수 없다. 나머지 기능은 그대로 동작한다.");
            anyhow::bail!("cwd 를 읽지 못했다");
        }
    }

    // 한 칸 위로 옮겨 보고 값이 따라오는지 본다. `cd ..` 는 어느 셸에서나 같다.
    outln!("");
    outln!("  «cd ..» 를 보낸다");
    s.write(b"cd ..\r")?;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        s.pump();
    }
    let after = pty::process_cwd(&mut sys, pid);
    match &after {
        Some(p) => outln!("  cd .. 뒤:    {}", p.display()),
        None => outln!("  cd .. 뒤:    (읽지 못함)"),
    }

    outln!("");
    let ok = match (&before, &after) {
        (Some(b), Some(a)) if a != b => {
            outln!("  RESULT: cd 를 따라간다 — 공간 경로가 갱신된다");
            true
        }
        (Some(_), Some(_)) => {
            outln!("  RESULT: 셸이 cd 해도 프로세스 경로가 그대로다");
            outln!("          → 공간 경로가 안 따라간다. PowerShell 에서 흔한 일이다.");
            outln!("          → cmd.exe 를 쓰거나(COMSPEC), 그대로 두고 [+] 로 새 공간을 지정한다.");
            false
        }
        _ => {
            outln!("  RESULT: 판정 불가");
            false
        }
    };
    if ok { Ok(()) } else { anyhow::bail!("cd 를 따라가지 못한다") }
}

// ---------- 셀프테스트 ----------

/// TTY 없이 PTY + VT 파이프라인을 검증한다.
fn selftest() -> Result<()> {
    outln!("== PTY + VT 셀프테스트 ==");

    let argv: Vec<String> = std::env::args().skip(2).collect();
    let (prog, args): (String, Vec<String>) = if argv.is_empty() {
        // 자식은 OS 마다 다르다. 화면에 색이 붙은 한글 한 줄만 나오면 된다.
        if cfg!(windows) {
            (
                "powershell".to_string(),
                vec![
                    "-NoProfile".into(),
                    "-Command".into(),
                    "$e=[char]27; Write-Host \"${e}[31m빨강${e}[0m OK\"".into(),
                ],
            )
        } else {
            (
                "/bin/sh".to_string(),
                vec![
                    "-c".into(),
                    // printf 는 \033 을 풀어 준다. echo -e 는 셸마다 다르게 군다.
                    "printf '\\033[31m빨강\\033[0m OK\\n'".into(),
                ],
            )
        }
    } else {
        (argv[0].clone(), argv[1..].to_vec())
    };
    let argrefs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    outln!("  자식: {prog} {argrefs:?}");

    let mut s = pty::PtySession::spawn_raw(&prog, &argrefs, 24, 80)?;

    // 자식이 끝나도 PTY 버퍼에 아직 안 읽은 출력이 남아 있다. 종료를 보자마자
    // 단정하면 절반만 읽고 실패한다(실측 — 91/110 바이트에서 실패). 그래서
    // **출력이 잠잠해질 때까지** 더 읽는다.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut quiet_since = None;
    while std::time::Instant::now() < deadline {
        let before = s.rx_bytes;
        s.pump();
        if s.rx_bytes != before {
            quiet_since = None;
        } else if s.finished() {
            let t = *quiet_since.get_or_insert(std::time::Instant::now());
            if t.elapsed() >= Duration::from_millis(300) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    s.pump();

    let screen = s.screen();
    let text = screen_text(&screen);
    outln!("  자식 종료: {} (코드 {:?})", s.finished(), s.exit_code);
    outln!("  수신 바이트: {}", s.rx_bytes);
    outln!("  화면 첫 줄: {:?}", text.lines().next().unwrap_or(""));

    let mut ok = true;
    if !text.contains("빨강") {
        outln!("  [FAIL] 한글이 화면 버퍼에 없다");
        ok = false;
    }
    if !text.contains("OK") {
        outln!("  [FAIL] 평문이 화면 버퍼에 없다");
        ok = false;
    }
    let red = find_cell(&screen, '빨').map(|c| c.fgcolor());
    outln!("  '빨' 전경색: {red:?}");
    if !matches!(red, Some(vt100::Color::Idx(1))) {
        outln!("  [FAIL] 색 속성이 보존되지 않았다");
        ok = false;
    }
    let wide = find_cell(&screen, '빨').map(|c| c.is_wide());
    outln!("  '빨' 와이드 셀: {wide:?}");
    if wide != Some(true) {
        outln!("  [FAIL] 한글이 와이드로 처리되지 않았다");
        ok = false;
    }

    outln!("");
    outln!(
        "{}",
        if ok {
            "  RESULT: PTY + VT 파이프라인 정상"
        } else {
            "  RESULT: 실패"
        }
    );
    if ok {
        Ok(())
    } else {
        anyhow::bail!("셀프테스트 실패")
    }
}

fn screen_text(screen: &vt100::Screen) -> String {
    let (rows, cols) = screen.size();
    let mut out = String::new();
    for r in 0..rows {
        let mut c = 0;
        while c < cols {
            if let Some(cell) = screen.cell(r, c) {
                let s = cell.contents();
                out.push_str(if s.is_empty() { " " } else { &s });
                // 와이드 문자는 두 칸을 차지한다. 뒤 칸은 이어지는 자리라 건너뛴다.
                c += if cell.is_wide() { 2 } else { 1 };
            } else {
                c += 1;
            }
        }
        out.push('\n');
    }
    out
}

fn find_cell<'a>(screen: &'a vt100::Screen, ch: char) -> Option<&'a vt100::Cell> {
    let (rows, cols) = screen.size();
    for r in 0..rows {
        for c in 0..cols {
            if let Some(cell) = screen.cell(r, c) {
                if cell.contents().starts_with(ch) {
                    return Some(cell);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(&[("claude", "Claude"), ("codex", "Codex")])
    }
    fn key(c: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(c, m)
    }

    /// 프리픽스 키의 **이름**이 플랫폼마다 다르다. 유닉스 crossterm 은 0x1D 를
    /// `Char('5')` 로, Windows 는 `Char(']')` 로 준다. 한쪽만 보면 그 플랫폼에서
    /// 프리픽스가 통째로 죽는다 — 실제로 macOS 에서 그랬다.
    #[test]
    fn 프리픽스는_두_이름을_모두_받는다() {
        for name in [KeyCode::Char(']'), KeyCode::Char('5')] {
            let mut a = app();
            on_key_panes(&mut a, key(name, KeyModifiers::CONTROL)).unwrap();
            assert!(a.prefix, "{name:?} 를 프리픽스로 못 받았다");
        }
    }

    #[test]
    fn 프리픽스_다음_q_는_종료다() {
        let mut a = app();
        on_key_panes(&mut a, key(KeyCode::Char('5'), KeyModifiers::CONTROL)).unwrap();
        on_key_panes(&mut a, key(KeyCode::Char('q'), KeyModifiers::empty())).unwrap();
        assert!(a.quit, "Ctrl+] 다음 q 가 종료로 이어지지 않았다");
        assert!(!a.prefix, "프리픽스가 한 번 쓰고 풀려야 한다");
    }

    /// `[+]` 와 Ctrl+] t 는 터미널을 붙인다. 에이전트 선택 상자가 아니다.
    #[test]
    fn 프리픽스_t_는_터미널을_붙인다() {
        let mut a = app();
        let before = a.sessions.len();
        on_key_panes(&mut a, key(KeyCode::Char('5'), KeyModifiers::CONTROL)).unwrap();
        on_key_panes(&mut a, key(KeyCode::Char('t'), KeyModifiers::empty())).unwrap();
        assert_eq!(a.sessions.len(), before + 1, "터미널 칸이 안 생겼다");
        let s = a.sessions.last().expect("방금 붙인 칸");
        assert!(s.is_terminal, "터미널로 표시돼야 한다");
        assert!(matches!(a.mode, Mode::Panes), "선택 상자로 새면 안 된다");
    }

    /// 닫은 참여자를 되살리는 길은 남아 있어야 한다.
    #[test]
    fn 프리픽스_p_는_에이전트_선택_상자다() {
        let mut a = app();
        on_key_panes(&mut a, key(KeyCode::Char('5'), KeyModifiers::CONTROL)).unwrap();
        on_key_panes(&mut a, key(KeyCode::Char('p'), KeyModifiers::empty())).unwrap();
        assert!(matches!(a.mode, Mode::Picker));
    }

    #[test]
    fn 셸을_해석한다() {
        let (exe, _) = pty::resolve_shell().expect("셸이 하나는 있어야 한다");
        assert!(exe.is_file(), "{exe:?} 가 실행 파일이 아니다");
        assert!(!pty::shell_name().is_empty());
    }

    fn app_with_tabs() -> App {
        let mut a = app();
        a.sessions.push(model::Session::terminal("zsh", 0));
        a.sessions.push(model::Session::terminal("zsh 2", 0));
        a
    }

    /// crossterm 은 «두 번 누름» 을 알려주지 않는다. 우리가 시각으로 가른다.
    #[test]
    fn 탭을_빠르게_두_번_누르면_이름_고치기다() {
        let mut a = app_with_tabs();
        a.tab_click(1);
        assert!(matches!(a.mode, Mode::Panes), "한 번은 포커스만");
        assert_eq!(a.focus, 1);
        a.tab_click(1);
        assert!(matches!(a.mode, Mode::RenameTab), "두 번이면 이름 고치기");
        assert_eq!(a.input, "zsh 2", "지금 이름이 채워져 있어야 고쳐 쓴다");
    }

    /// 다른 탭을 잇달아 누른 것은 두 번 누름이 아니다.
    #[test]
    fn 서로_다른_탭은_두_번_누름이_아니다() {
        let mut a = app_with_tabs();
        a.tab_click(0);
        a.tab_click(1);
        assert!(matches!(a.mode, Mode::Panes));
        assert_eq!(a.focus, 1);
    }

    /// 사이가 뜨면 두 번 누름이 아니다.
    #[test]
    fn 천천히_두_번_누르면_포커스만_옮긴다() {
        let mut a = app_with_tabs();
        a.tab_click(1);
        std::thread::sleep(Duration::from_millis(450));
        a.tab_click(1);
        assert!(matches!(a.mode, Mode::Panes));
    }

    #[test]
    fn 이름을_고쳐_쓴다() {
        let mut a = app_with_tabs();
        a.begin_rename(0);
        a.commit_rename("계획 담당");
        assert_eq!(a.sessions[0].title, "계획 담당");
        assert!(matches!(a.mode, Mode::Panes));
    }

    /// 빈 이름을 받으면 지운다. 이름이 없으면 탭을 고를 수가 없다.
    #[test]
    fn 빈_이름은_무시한다() {
        let mut a = app_with_tabs();
        a.begin_rename(0);
        a.commit_rename("   ");
        assert_eq!(a.sessions[0].title, "zsh");
    }

    /// `[+]` 상자의 0번은 터미널이고 그다음이 에이전트다.
    #[test]
    fn 선택_상자는_터미널을_앞세운다() {
        let a = app();
        let items = a.picker_items();
        assert!(items[0].starts_with("터미널"), "{items:?}");
        assert_eq!(items.len(), a.agents.len() + 1);
        assert_eq!(items[1], "Claude");
    }

    /// Enter 는 전송, Shift/Alt+Enter 는 줄바꿈(ESC CR)이다.
    ///
    /// 터미널은 예로부터 둘을 구분하지 않고 CR 만 보냈다. 구분해 주는
    /// 터미널에서는 우리가 «ESC CR» 로 바꿔 에이전트에게 넘긴다.
    #[test]
    fn shift_enter_는_줄바꿈이다() {
        let plain = encode_key(&key(KeyCode::Enter, KeyModifiers::empty())).unwrap();
        assert_eq!(plain, b"\r", "맨 Enter 는 그대로 전송이다");

        for m in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
            let v = encode_key(&key(KeyCode::Enter, m)).unwrap();
            assert_eq!(v, b"\x1b\r", "{m:?} + Enter 는 줄바꿈이어야 한다");
        }
    }

    /// Ctrl+A 는 우리가 «모두에게 묻기»로 가져갔다.
    #[test]
    fn ctrl_a_는_모두에게_묻기다() {
        let mut a = app();
        on_key_panes(&mut a, key(KeyCode::Char('a'), KeyModifiers::CONTROL)).unwrap();
        assert!(matches!(a.mode, Mode::Broadcast));
    }
}
