//! 서브에이전트 조회.
//!
//! 에이전트가 하위 작업을 돌리면 **자식 프로세스**로 뜬다. Claude Code 는
//! `CLAUDE_CODE_CHILD_SESSION` 을 단 자식 세션을 만들고, 우리는 그 프로세스를
//! 관측할 수 있다.
//!
//! **이름까지는 알 수 없다.** 어떤 서브에이전트인지는 프로세스 목록에 안 나온다.
//! 그래서 개수와 PID 만 보여준다 — 아는 것까지만 말한다.
//!
//! 프로세스 열거는 비싸다. 초당 한 번만 갱신한다.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

pub struct Tree {
    sys: System,
    last: Instant,
    /// 부모 PID -> 자식 PID 목록
    children: HashMap<u32, Vec<u32>>,
}

impl Tree {
    pub fn new() -> Self {
        Self {
            sys: System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::new()),
            ),
            last: Instant::now() - Duration::from_secs(10),
            children: HashMap::new(),
        }
    }

    /// 필요하면 갱신한다. 매 프레임 호출해도 초당 한 번만 실제로 훑는다.
    pub fn refresh_if_stale(&mut self) {
        if self.last.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        self.children.clear();
        for (pid, p) in self.sys.processes() {
            if let Some(parent) = p.parent() {
                self.children
                    .entry(parent.as_u32())
                    .or_default()
                    .push(pid.as_u32());
            }
        }
        self.last = Instant::now();
    }

    /// 어떤 프로세스의 자손 전부. 깊이 우선으로 훑되 순환은 막는다.
    pub fn descendants(&self, root: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        let mut seen = std::collections::HashSet::new();
        while let Some(p) = stack.pop() {
            let Some(kids) = self.children.get(&p) else { continue };
            for &k in kids {
                if seen.insert(k) {
                    out.push(k);
                    stack.push(k);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// 프로세스 이름. 없으면 빈 문자열.
    pub fn name(&self, pid: u32) -> String {
        self.sys
            .process(Pid::from_u32(pid))
            .map(|p| p.name().to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}
