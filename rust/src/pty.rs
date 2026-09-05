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
    /// 에이전트를 현재 디렉터리에서 기동한다.
    pub fn spawn(agent: &str, rows: u16, cols: u16) -> Result<Self> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Self::spawn_in(agent, rows, cols, &cwd)
    }

    /// 에이전트를 **주어진 경로에서** 기동한다. 크기는 화면과 같아야 한다.
    ///
    /// 공간(space)마다 경로가 다르므로 작업 디렉터리를 인자로 받는다.
    /// 에이전트는 cwd 를 기준으로 파일을 찾으므로 이게 곧 「어느 프로젝트인가」다.
    pub fn spawn_in(agent: &str, rows: u16, cols: u16, cwd: &std::path::Path) -> Result<Self> {
        let (exe, prefix) = resolve_agent(agent)
            .ok_or_else(|| anyhow!("실행 파일을 찾지 못했다: {agent}"))?;

        let pty = portable_pty::native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("PTY 를 열지 못했다")?;

        let mut cmd = CommandBuilder::new(exe);
        for a in &prefix {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
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

    /// 자식 프로세스 ID. 서브에이전트(자손 프로세스) 조회의 기준점이다.
    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
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

impl PtySession {
    /// 자식을 종료한다. 종료 확인까지 짧게 기다린다.
    ///
    /// 이걸 안 하면 우리가 빠져나온 뒤에도 에이전트 프로세스가 남는다.
    /// Java 판에서 /cancel 을 만들며 겪었던 것과 같은 부류의 문제다.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 에이전트를 어떻게 띄울지 해석한다.
///
/// 반환값은 (실행 파일, 선행 인자) 다. codex 처럼 실행 파일이 PATH 에
/// `.exe` 로 없고 node 스크립트로 존재하는 경우가 있다.
///
/// **셸 래퍼(.cmd/.ps1)는 쓰지 않는다.** 셸이 인자를 재해석해 프롬프트가 깨진다
/// (Java 판에서 실측 확인. SPEC §6.3).
pub fn resolve_agent(agent: &str) -> Option<(PathBuf, Vec<String>)> {
    // 사용자가 경로를 직접 준 경우
    let direct = PathBuf::from(agent);
    if direct.is_file() {
        return Some((direct, vec![]));
    }
    match agent {
        "codex" => resolve_codex(),
        // agy 는 macOS 에서 ~/.local/bin 에 설치되는 경우가 많다 (SPEC §8.4 K6).
        "agy" if !cfg!(windows) => {
            let local = home_dir().join(".local/bin/agy");
            if local.is_file() {
                return Some((local, vec![]));
            }
            resolve("agy").map(|p| (p, vec![]))
        }
        other => resolve(other).map(|p| (p, vec![])),
    }
}

fn home_dir() -> PathBuf {
    crate::app::home_var()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// codex 는 PATH 에 codex.exe 가 없다. 셸 shim(codex/.cmd/.ps1)만 있다.
///
///   1순위  벤더 네이티브 codex.exe
///   2순위  node.exe + codex.js  (shim 이 하는 일을 셸 없이 재현)
///   실패   지원 불가로 보고한다
fn resolve_codex() -> Option<(PathBuf, Vec<String>)> {
    let native = if cfg!(windows) { "codex.exe" } else { "codex" };
    for root in npm_roots() {
        let base = root.join("node_modules/@openai/codex/node_modules");
        if base.is_dir() {
            if let Some(exe) = find_file(&base, native, 8, Some("vendor")) {
                return Some((exe, vec![]));
            }
        }
    }
    let node = resolve("node")?;
    for root in npm_roots() {
        let js = root.join("node_modules/@openai/codex/bin/codex.js");
        if js.is_file() {
            return Some((node, vec![js.to_string_lossy().into_owned()]));
        }
    }
    None
}

fn npm_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        out.push(PathBuf::from(appdata).join("npm"));
    }
    if !cfg!(windows) {
        // Homebrew(Intel/Apple Silicon)와 사용자 npm prefix
        out.push(PathBuf::from("/opt/homebrew/lib"));
        out.push(PathBuf::from("/usr/local/lib"));
        out.push(home_dir().join(".npm-global/lib"));
    }
    // 셸 shim 이 있는 디렉터리도 후보다.
    if let Some(p) = resolve("codex.cmd").and_then(|p| p.parent().map(PathBuf::from)) {
        out.push(p);
    }
    out
}

/// 깊이 제한 재귀 탐색. 경로에 `must_contain` 이 들어간 것만 채택한다.
fn find_file(dir: &std::path::Path, name: &str, depth: usize, must_contain: Option<&str>) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            dirs.push(path);
        } else if path.file_name().and_then(|s| s.to_str()) == Some(name) {
            let ok = match must_contain {
                Some(part) => path.to_string_lossy().replace(chr_backslash(), "/").contains(part),
                None => true,
            };
            if ok {
                return Some(path);
            }
        }
    }
    for d in dirs {
        if let Some(hit) = find_file(&d, name, depth - 1, must_contain) {
            return Some(hit);
        }
    }
    None
}

fn chr_backslash() -> char {
    92 as char
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
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    if !cfg!(windows) {
        // GUI 로 띄우면 PATH 가 빈약하다. 흔한 위치를 보조로 덧붙인다.
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(home_dir().join(".local/bin"));
    }
    for dir in dirs {
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
