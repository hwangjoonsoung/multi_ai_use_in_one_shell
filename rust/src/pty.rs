//! PTY 세션 하나. 자식 에이전트를 대화형 그대로 띄우고 출력을 VT 파서에 먹인다.
//!
//! 헤드리스(`claude -p`)와 다른 점: 에이전트가 자기 TUI 를 그린다. 우리는 그 화면을
//! 받아 그릴 뿐이다.

use anyhow::{anyhow, Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, TryRecvError},
        Arc, Mutex,
    },
};

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    parser: Arc<Mutex<vt100::Parser>>,
    done: bool,
    /// 진단용 — PTY 에서 실제로 받은 바이트 수
    pub rx_bytes: usize,
    /// 진단용 — 자식 종료 코드
    pub exit_code: Option<u32>,
}

impl PtySession {
    /// 에이전트를 PTY 안에서 기동한다. 크기는 화면과 같아야 한다.
    pub fn spawn(agent: &str, rows: u16, cols: u16) -> Result<Self> {
        let exe = resolve(agent)
            .ok_or_else(|| anyhow!("실행 파일을 찾지 못했다: {agent}"))?;

        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("PTY 를 열지 못했다")?;

        let mut cmd = CommandBuilder::new(exe);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        // Claude Code 세션 식별 변수를 물려주지 않는다. 중첩 실행 시 혼동을 막는다.
        for k in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_SESSION_ID"] {
            cmd.env_remove(k);
        }

        let child = pair.slave.spawn_command(cmd).context("에이전트를 띄우지 못했다")?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("PTY 리더 복제 실패")?;
        let writer = pair.master.take_writer().context("PTY 라이터 획득 실패")?;

        // 읽기는 블로킹이라 별도 스레드에 둔다. 안 그러면 렌더 루프가 멈춘다.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master: pair.master,
            child,
            writer,
            rx,
            parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2000))),
            done: false,
            rx_bytes: 0,
            exit_code: None,
        })
    }

    /// 받은 출력을 전부 파서에 먹인다. 논블로킹이다.
    pub fn pump(&mut self) {
        let mut pending_replies: Vec<u8> = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(chunk) => {
                    self.rx_bytes += chunk.len();
                    if std::env::var("MAI_DUMP").is_ok() {
                        eprintln!("[rx {}] {:?}", chunk.len(), String::from_utf8_lossy(&chunk));
                    }
                    if let Ok(mut p) = self.parser.lock() {
                        p.process(&chunk);
                    }
                    // 자식이 던진 질의에 답한다. 안 하면 상대가 응답을 기다리며 멈춘다.
                    pending_replies.extend(self.answer_queries(&chunk));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.done = true;
                    break;
                }
            }
        }
        if !pending_replies.is_empty() {
            let _ = self.write(&pending_replies);
        }
        if let Ok(Some(st)) = self.child.try_wait() {
            self.exit_code = Some(st.exit_code());
            self.done = true;
        }
    }

    /// 터미널 질의에 대한 응답을 만든다.
    ///
    /// **에뮬레이터의 필수 책무다.** ConPTY 는 기동 직후 `ESC[6n`(커서 위치)을 보내고
    /// 응답을 기다린다. 답하지 않으면 자식 출력이 한 바이트도 나오지 않는다
    /// (R1 에서 실측으로 확인한 원인). vt100 크레이트는 이걸 대신해 주지 않는다.
    fn answer_queries(&self, chunk: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < chunk.len() {
            if chunk[i] != 0x1b || chunk[i + 1] != b'[' {
                i += 1;
                continue;
            }
            // ESC [ ... <최종 바이트> 형태를 훑는다.
            let start = i + 2;
            let mut j = start;
            while j < chunk.len() && !(0x40..=0x7e).contains(&chunk[j]) {
                j += 1;
            }
            if j >= chunk.len() {
                break;
            }
            let params = &chunk[start..j];
            let final_byte = chunk[j];
            match (params, final_byte) {
                // DSR — 커서 위치 보고. 1-기준으로 답한다.
                (b"6", b'n') | (b"?6", b'n') => {
                    let (r, c) = self
                        .parser
                        .lock()
                        .map(|p| p.screen().cursor_position())
                        .unwrap_or((0, 0));
                    out.extend_from_slice(
                        format!("\x1b[{};{}R", r + 1, c + 1).as_bytes(),
                    );
                }
                // DSR — 상태 보고. "이상 없음".
                (b"5", b'n') => out.extend_from_slice(b"\x1b[0n"),
                // DA1 — 장치 속성. VT100 상당으로 답한다.
                (b"", b'c') | (b"0", b'c') => out.extend_from_slice(b"\x1b[?1;2c"),
                // DA2 — 보조 장치 속성.
                (b">", b'c') | (b">0", b'c') => out.extend_from_slice(b"\x1b[>0;10;1c"),
                _ => {}
            }
            i = j + 1;
        }
        out
    }

    pub fn screen(&self) -> vt100::Screen {
        self.parser.lock().unwrap().screen().clone()
    }

    pub fn finished(&self) -> bool {
        self.done
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// 화면이 바뀌면 PTY 와 파서 양쪽을 같이 맞춘다. 한쪽만 바꾸면 어긋난다.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
        Ok(())
    }
}

/// PATH 에서 실행 파일을 찾는다. Windows 는 확장자를 붙여 본다.
///
/// 셸 래퍼(.cmd/.ps1)는 쓰지 않는다 — 셸이 인자를 재해석해 프롬프트가 깨진다
/// (Java 판에서 실측으로 확인한 사항. SPEC §6.3).
fn resolve(name: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(name);
    if direct.is_file() {
        return Some(direct);
    }
    let exts: &[&str] = if cfg!(windows) { &[".exe", ""] } else { &[""] };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let c = dir.join(format!("{name}{ext}"));
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

impl PtySession {
    /// 인자를 직접 지정해 띄운다. 셀프테스트와 헤드리스 호출에 쓴다.
    pub fn spawn_raw(program: &str, args: &[&str], rows: u16, cols: u16) -> Result<Self> {
        let exe = resolve(program)
            .ok_or_else(|| anyhow!("실행 파일을 찾지 못했다: {program}"))?;
        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("PTY 를 열지 못했다")?;

        let mut cmd = CommandBuilder::new(exe);
        for a in args {
            cmd.arg(a);
        }
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }


        let child = pair.slave.spawn_command(cmd).context("자식을 띄우지 못했다")?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("PTY 리더 복제 실패")?;
        let writer = pair.master.take_writer().context("PTY 라이터 획득 실패")?;

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master: pair.master,
            child,
            writer,
            rx,
            parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2000))),
            done: false,
            rx_bytes: 0,
            exit_code: None,
        })
    }
}
