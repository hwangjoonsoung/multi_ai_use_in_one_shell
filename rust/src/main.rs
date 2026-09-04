//! R1 — PTY 하나에 에이전트를 띄워 화면에 붙인다.
//!
//! 목적은 하나다: **터미널에서 그 에이전트를 직접 쓰는 것과 구분이 안 되는가.**
//! 로고·상태바·`/` 자동완성·권한 프롬프트가 다 보이고 조작돼야 한다.
//! 여기서 막히면 이후 계획이 의미가 없다 (REBUILD.md §7.1).
//!
//! 사용법:  multi_ai_cli [에이전트]        기본값 claude

mod pty;
mod vtscreen;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io, time::Duration};

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "claude".to_string());
    if arg == "--selftest" {
        return selftest();
    }
    let agent = arg;

    // 터미널 크기를 알아야 PTY 를 같은 크기로 연다. 크기가 어긋나면
    // 에이전트가 자기 화면을 잘못 그린다.
    let (cols, rows) = crossterm::terminal::size()?;
    let mut session = pty::PtySession::spawn(&agent, rows, cols)?;

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;

    let result = run(&mut term, &mut session);

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;

    match result {
        Ok(()) => {
            println!("종료: {agent} 세션이 끝났습니다.");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn run(term: &mut Term, session: &mut pty::PtySession) -> Result<()> {
    loop {
        // 자식이 뱉은 출력을 파서에 먹인다.
        session.pump();

        term.draw(|f| {
            let area = f.area();
            let screen = session.screen();
            f.render_widget(vtscreen::VtScreen::new(&screen), area);
            // 자식이 커서를 보이게 두었으면 우리도 같은 자리에 놓는다.
            if !screen.hide_cursor() {
                let (r, c) = screen.cursor_position();
                f.set_cursor_position((area.x + c, area.y + r));
            }
        })?;

        if session.finished() {
            return Ok(());
        }

        // 키를 자식에게 그대로 넘긴다. 여기서 권한 승인·/ 자동완성·esc 가 동작한다.
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(k) => {
                    // Windows 에서 crossterm 은 누를 때(Press)와 뗄 때(Release)를
                    // 모두 보낸다. 그대로 넘기면 한 번 친 키가 두 번 입력된다.
                    // Repeat 은 길게 눌렀을 때이므로 함께 통과시킨다.
                    if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    // Ctrl+] 는 우리 탈출구. 자식에게 넘기지 않는다.
                    if k.modifiers.contains(KeyModifiers::CONTROL)
                        && k.code == KeyCode::Char(']')
                    {
                        return Ok(());
                    }
                    if let Some(bytes) = encode_key(&k) {
                        session.write(&bytes)?;
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
            // Ctrl+A..Z → 0x01..0x1A
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

/// TTY 없이 PTY + VT 파이프라인을 검증한다.
///
/// R1 의 성패는 "PTY 로 띄운 자식의 출력이 화면 버퍼로 정확히 들어오는가" 다.
/// 대화형 렌더는 사람이 봐야 알지만, 아래 세 가지는 자동으로 확인할 수 있다.
///   1. Windows ConPTY 로 자식이 뜨는가
///   2. 출력이 vt100 파서를 거쳐 셀 격자에 들어오는가
///   3. 색·굵기 같은 속성과 한글 와이드 셀이 보존되는가
fn selftest() -> Result<()> {
    println!("== R1 셀프테스트 ==");

    // 1) ANSI 색과 한글을 함께 내보내는 자식을 PTY 에 띄운다.
    // 인자로 프로그램을 바꿔 시험할 수 있게 한다. 기본은 cmd 로 가장 단순하게.
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

    // 자식이 끝날 때까지 최대 20초 기다리며 출력을 먹인다.
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

    // 2) 속성 확인 — 빨강 글자의 전경색이 인덱스 1 이어야 한다.
    let red = find_cell(&screen, '빨').map(|c| c.fgcolor());
    println!("  '빨' 전경색: {:?}", red);
    if !matches!(red, Some(vt100::Color::Idx(1))) {
        println!("  [FAIL] 색 속성이 보존되지 않았다");
        ok = false;
    }

    // 3) 와이드 셀 확인 — 한글은 두 칸을 차지해야 한다.
    let wide = find_cell(&screen, '빨').map(|c| c.is_wide());
    println!("  '빨' 와이드 셀: {:?}", wide);
    if wide != Some(true) {
        println!("  [FAIL] 한글이 와이드로 처리되지 않았다");
        ok = false;
    }

    println!();
    println!("{}", if ok { "  RESULT: PTY + VT 파이프라인 정상" } else { "  RESULT: 실패" });
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
