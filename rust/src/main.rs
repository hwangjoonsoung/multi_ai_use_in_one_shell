//! multi_ai_cli — Claude / Codex / Gemini(agy) 를 한 화면에서 다루는 에이전트 멀티플렉서.
//!
//! R1  PTY 하나를 화면에 붙인다 (통과)
//! R2  참여자마다 자기 칸. 포커스된 칸으로만 키가 간다  ← 현재
//!
//! 사용법
//!   multi_ai_cli               시작 화면에서 질문을 입력한다
//!   multi_ai_cli --solo <에이전트>   한 에이전트만 전체 화면으로 (R1 확인용)
//!   multi_ai_cli --selftest    PTY+VT 파이프라인 자동 점검

mod app;
mod pty;
mod vtscreen;

use anyhow::Result;
use app::{App, Mode};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};

/// 기본 참여자. R3 에서 설치 탐색 결과로 대체한다.
const AGENTS: &[(&str, &str)] = &[
    ("claude", "Claude"),
    ("codex", "Codex"),
    ("agy", "Gemini via agy"),
];

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--selftest") => return selftest(),
        Some("--which") => {
            // 각 에이전트를 어떻게 띄울지 보여준다. 기동 실패를 진단할 때 쓴다.
            for (id, title) in AGENTS {
                match pty::resolve_agent(id) {
                    Some((exe, args)) => {
                        println!("  {title:<16} {}", exe.display());
                        if !args.is_empty() {
                            println!("  {:<16} 선행 인자 {args:?}", "");
                        }
                    }
                    None => println!("  {title:<16} 찾지 못함 — 이 참여자는 비활성"),
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
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let result = body(&mut term);

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    result
}

// ---------- R2 본 흐름 ----------

fn run(term: &mut Term) -> Result<()> {
    let mut app = App::new(AGENTS);

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
                // Windows 는 누를 때와 뗄 때를 모두 보낸다. 그대로 넘기면
                // 한 번 친 키가 두 번 입력된다.
                if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    continue;
                }
                match app.mode {
                    Mode::Idle => on_key_idle(&mut app, k, term)?,
                    Mode::Panes => on_key_panes(&mut app, k)?,
                }
            }
            Event::Resize(_, _) => {
                let a = term.size()?;
                app.sync_sizes(ratatui::layout::Rect::new(0, 0, a.width, a.height));
            }
            _ => {}
        }
    }
}

/// 시작 화면 — 여기서는 우리가 직접 줄 편집을 한다.
fn on_key_idle(app: &mut App, k: KeyEvent, term: &mut Term) -> Result<()> {
    match k.code {
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Enter => {
            let q = app.input.trim().to_string();
            if !q.is_empty() {
                let a = term.size()?;
                app.start_round(&q, ratatui::layout::Rect::new(0, 0, a.width, a.height));
                app.input.clear();
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Esc => app.input.clear(),
        KeyCode::Char(c) => app.input.push(c),
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
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < app.panes.len() {
                    app.focus = i;
                }
            }
            KeyCode::Char('n') => {
                app.mode = Mode::Idle;
                app.input.clear();
            }
            KeyCode::Char('q') => app.quit = true,
            // 프리픽스를 한 번 더 누르면 프리픽스 자체를 자식에게 보낸다.
            KeyCode::Char(']') => {
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
        let n = app.panes.len();
        match k.code {
            KeyCode::Left => {
                if n > 0 {
                    app.focus = (app.focus + n - 1) % n;
                }
                return Ok(());
            }
            KeyCode::Right => {
                if n > 0 {
                    app.focus = (app.focus + 1) % n;
                }
                return Ok(());
            }
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < n {
                    app.focus = i;
                }
                return Ok(());
            }
            _ => {}
        }
    }

    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char(']') {
        app.prefix = true;
        return Ok(());
    }

    if let Some(bytes) = encode_key(&k) {
        if let Some(s) = app.focused() {
            let _ = s.write(&bytes);
        }
    }
    Ok(())
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

// ---------- 셀프테스트 ----------

/// TTY 없이 PTY + VT 파이프라인을 검증한다.
fn selftest() -> Result<()> {
    println!("== PTY + VT 셀프테스트 ==");

    let argv: Vec<String> = std::env::args().skip(2).collect();
    let (prog, args): (String, Vec<String>) = if argv.is_empty() {
        (
            "powershell".to_string(),
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                "$e=[char]27; Write-Host \"${e}[31m빨강${e}[0m OK\"".into(),
            ],
        )
    } else {
        (argv[0].clone(), argv[1..].to_vec())
    };
    let argrefs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    println!("  자식: {prog} {argrefs:?}");

    let mut s = pty::PtySession::spawn_raw(&prog, &argrefs, 24, 80)?;

    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        s.pump();
        if s.finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    s.pump();

    let screen = s.screen();
    let text = screen_text(&screen);
    println!("  자식 종료: {} (코드 {:?})", s.finished(), s.exit_code);
    println!("  수신 바이트: {}", s.rx_bytes);
    println!("  화면 첫 줄: {:?}", text.lines().next().unwrap_or(""));

    let mut ok = true;
    if !text.contains("빨강") {
        println!("  [FAIL] 한글이 화면 버퍼에 없다");
        ok = false;
    }
    if !text.contains("OK") {
        println!("  [FAIL] 평문이 화면 버퍼에 없다");
        ok = false;
    }
    let red = find_cell(&screen, '빨').map(|c| c.fgcolor());
    println!("  '빨' 전경색: {red:?}");
    if !matches!(red, Some(vt100::Color::Idx(1))) {
        println!("  [FAIL] 색 속성이 보존되지 않았다");
        ok = false;
    }
    let wide = find_cell(&screen, '빨').map(|c| c.is_wide());
    println!("  '빨' 와이드 셀: {wide:?}");
    if wide != Some(true) {
        println!("  [FAIL] 한글이 와이드로 처리되지 않았다");
        ok = false;
    }

    println!();
    println!(
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
