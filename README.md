# multi_ai_cli

Claude · Codex · Gemini(agy) 세 AI가 한 채팅방에 참여자로 들어와, 같은 안건에 독립적으로 답한 뒤 서로의 답을 읽고 쟁점을 좁혀가는 독립 CLI 애플리케이션.

**상태**: **Rust + PTY 로 재작성 결정** (2026-09-04) · 계획은 [REBUILD.md](REBUILD.md) · Java 구현은 R4 까지 참조용으로 유지

## 실행

현행 구현은 **Rust 판**(`rust/`)이다. 아래 Java 판 명령은 참조용으로 남겨 둔다.

### 설치 — 한 단어로 띄우기 (권장)

```bash
./scripts/install.sh          # ~/.local/bin/mai 심링크
```

그 뒤로는 **어느 디렉터리에서든** `mai` 한 단어면 된다. 빌드가 필요하면 알아서 하고 바로 띄운다.

```bash
cd ~/any/project
mai                # 여기를 공간으로 실행
mai ~/other        # 다른 디렉터리를 공간으로
mai --which        # 참여자를 어떻게 띄우는지
mai --doctor       # 설치·인증·툴체인 점검
mai --rebuild      # 강제로 다시 빌드하고 실행
```

`mai` 는 **cwd 를 바꾸지 않는다.** 앱이 자기 작업 디렉터리를 첫 공간으로 잡으므로
어디서 쳤는지가 곧 어느 프로젝트인지다. 빌드만 서브셸에서 저장소로 들어간다.

심링크 대신 alias 를 쓰려면 `./scripts/install.sh --alias`, 되돌리려면 `--uninstall`.
심링크를 기본으로 삼은 이유는 alias 가 대화형 셸에서만 살아 있어 스크립트나 다른
도구가 부를 때는 없는 것이 되기 때문이다.

### macOS / Linux — Rust 판 (스크립트 직접 호출)

```bash
./scripts/doctor.sh                        # CLI 설치·인증·모델 + Rust 툴체인 점검
./scripts/rust-build.sh                    # cargo build (--release 도 그대로 넘어간다)
./scripts/rust-run.sh                      # 현재 디렉터리를 워크스페이스로 실행
./scripts/rust-run.sh --workspace ~/proj   # 대상 워크스페이스 지정 (D18)
./scripts/rust-run.sh --which              # 참여자를 어떻게 띄우는지 확인
./scripts/rust-run.sh --trust              # 현재 디렉터리를 각 에이전트에 신뢰 등록
./scripts/rust-run.sh --selftest           # PTY+VT 파이프라인 점검
```

Rust 툴체인이 없으면 먼저 깐다. `rust-build.sh` 는 `~/.cargo/env` 를 알아서 읽는다.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

세 CLI 중 없는 것은 **비활성 참여자로 빠지고 나머지로 진행한다.** `agy` 가 없어도 앱은 뜬다.

#### 화면과 키 (Rust TUI)

띄우면 **곧바로 패널 화면**이다. 질문을 먼저 묻는 시작 화면은 없다. 실행한 디렉터리가 첫 공간이 되고,
쓸 수 있는 참여자가 전부 자기 칸에 뜬다. 각 칸에는 에이전트 자신의 첫 화면이 그대로 나오므로,
평소 그 CLI 를 쓰던 것과 똑같이 그 칸에 바로 입력하면 된다.

```
┌ spaces ┐ Claude  Codex  Gemini  zsh  [+]   ← 탭 줄. [+] 로 터미널 추가
│+ 새 공간│┌ Claude ─[x]┐┌ Codex ─[x]┐┌ Gemini ─[x]┐
│▸ 현재경로│└────────────┘└───────────┘└────────────┘
└────────┘
┌ agents ┐   ● 색 점은 관측된 상태. 아래는 서브에이전트 명부
```

| 키 | 동작 |
|---|---|
| 그냥 타이핑 | **포커스된 칸의 에이전트에게 그대로** 간다 |
| `Ctrl+A` | 보이는 모든 에이전트에게 같은 질문을 한 번에 |
| `Alt+←/→` · `Alt+1..9` · 클릭 | 칸 이동 |
| 휠 | 커서가 놓인 것만 굴린다 — 사이드바 / 터미널 칸. **에이전트 칸은 아래 참고** |
| `Ctrl+]` 다음 `f` | **이 칸의 마지막 답변을 나머지 칸에 넘긴다.** 한 문장만 덧붙이면 된다 |
| `[+]` · `Ctrl+]` 다음 `t` | **터미널 칸 추가.** 그 공간의 경로에서 `$SHELL` 을 띄운다 |
| `Ctrl+]` 다음 `p` | 에이전트 선택 상자. 닫은 참여자를 되살릴 때 |
| `Ctrl+]` 다음 `q` | 종료 |
| `Ctrl+]` 다음 `n` | 모두에게 묻기 (`Ctrl+A` 와 같다) |
| `Ctrl+]` 다음 `a` / `]` | `Ctrl+A` / `Ctrl+]` 자체를 **자식에게** 보낸다 |

`[+]` 는 **터미널**을 붙인다. 에이전트는 기동 때 이미 다 떠 있어서, 그 자리에서 정작 아쉬운 건
같은 경로에서 명령을 쳐 볼 셸이기 때문이다. 에이전트를 다시 띄우는 길은 `Ctrl+]` 다음 `p` 다.

칸이 넷 이상이면 나란히 두지 않고 탭으로 바뀐다. `[전체]` 탭으로 다시 한 화면에 모을 수 있다.

#### 스크롤이 되는 칸, 안 되는 칸

휠은 커서가 놓인 곳에만 간다. 사이드바는 목록을 굴리고, **터미널 칸은 우리 스크롤백을 굴린다.**

**에이전트 칸은 스크롤되지 않는다.** 우리가 안 보내서가 아니라 에이전트가 안 받기 때문이다.
프로토콜대로 좌표를 칸 기준으로 바꿔 넘기고 있고, 받지 않는 자식이면 우리 스크롤백으로 떨어진다.
그런데 바닐라 PTY 에 직접 붙여 재 보면 이렇다.

| | 마우스 추적 | 휠 | PageUp/Down | 화살표 |
|---|:---:|:---:|:---:|:---:|
| Claude | 켬 (`?1000h`~`?1006h`) | 무시 | 무시 | 동작 |
| Codex | **안 켬** (`?1004` 뿐) | 무시 | — | 동작 |

게다가 둘 다 대체 화면(`ESC[?1049h`)에서 돌고, vt100 은 대체 화면에 스크롤백을 두지 않는다
(`Grid::new(size, 0)`). 지나간 출력이 우리 버퍼에도 남지 않는다는 뜻이다. 그래서 에이전트 칸의
지난 내용을 보려면 **답 넘기기와 같은 곳 — 세션 기록 —** 을 봐야 한다. `mai --quote claude` 가
그 입구다.

#### 답 넘기기 (`Ctrl+]` 다음 `f`)

AI-1 이 계획을 내놓았을 때 AI-2·AI-3 에게 「넌 어떻게 생각해?」를 한 제스처로 던지는 기능이다.

```
Claude 칸에 포커스 → Ctrl+] f
╭ Claude 의 답 8,412자 (세션 기록) ──────────────────────────╮
│받을 칸  [ ] Codex   [x] Gemini                             │
│한 마디  넌 어떻게 생각해?                                  │
│Tab 이동 · Space 켜기/끄기 · Enter 보내기 · Esc 취소        │
╰────────────────────────────────────────────────────────────╯
```

받는 칸에는 인용문 + 그 한 문장이 들어간다. 출처 칸은 후보에 없다 — 자기 답을 자기에게
다시 주는 꼴이기 때문이다. 처음엔 나머지가 전부 켜져 있고, `Tab`·`Space` 로 좁힌다.
대상 지정에 `Alt+숫자` 같은 조합키를 쓰지 않은 것은, macOS 터미널이 기본 설정에서
Option 을 Meta 로 보내지 않아 그 키가 아예 안 오는 환경이 있기 때문이다.

**세션은 공유할 수 없다.** 세 CLI 는 서로 다른 서버·포맷·인증을 쓰고, API 키 경로는
`INTENT.md §3.1` 이 막아 놨다. 그래서 세션이 아니라 **답변만** 옮긴다. 꺼내는 곳은 둘이다.

| 순위 | 어디서 | 한계 |
|:---:|---|---|
| 1 | 에이전트가 남긴 세션 기록 JSONL | 없음. 전문이 그대로 온다 |
| 2 | 우리 화면 버퍼 | 칸에 보이는 만큼. 대체 화면은 스크롤백이 없다 |

확인된 위치는 `~/.claude/projects/<경로>/…jsonl` 과 `~/.codex/sessions/…/rollout-*.jsonl` 이다.
`agy` 는 아직 못 찾아 화면으로 떨어진다. **이 포맷들은 비공개 내부 규약이라 CLI 업데이트로
깨질 수 있다.** 그래서 실패를 오류로 다루지 않고 조용히 2순위로 내려가며, 어디서 몇 자를
가져왔는지 상자 제목에 늘 표시한다.

무엇이 나올지 화면 없이 확인하려면:

```bash
mai --quote claude          # 지금 디렉터리의 claude 세션에서 마지막 답변
mai --quote codex ~/proj
```

`MAI_KEYLOG=/tmp/keys.log` 를 주면 받은 키를 그대로 적는다. 단축키가 안 먹을 때 쓴다 —
키 이름은 터미널·플랫폼마다 다르게 들어온다.

### Windows — Java 판 (참조용)

```powershell
.\scripts\doctor.ps1                 # CLI 설치·인증·모델 점검
.\scripts\compile.ps1                # javac (외부 라이브러리 없음, D12)
.\scripts\run.ps1                    # 현재 디렉터리를 워크스페이스로 실행
.\scripts\run.ps1 -Workspace C:\proj  # 대상 워크스페이스 지정 (D18)
.\scripts\run.ps1 -Room 20260903-141530   # 기존 방 재개
```

방 안에서 쓰는 명령:

| 입력 | 동작 |
|---|---|
| 일반 문장 | 전 참여자 동시 호출 |
| `@claude` / `@codex` / `@gemini <질문>` | 지목 호출 |
| `/status [auth]` | 참여자 경로·방 상태. `auth` 는 버전·인증·모델까지 확인 |
| `/rooms` · `/open <ID>` · `/new [이름]` | 방 목록·재개·생성 |
| `/run @<참여자> [--write] <프롬프트>` | 지정 권한으로 **1회** 실행. `--write` 는 참여자 1명만 |
| `/preset [list\|save\|run\|rm]` | 프롬프트 프리셋. 실행해도 권한은 승격되지 않는다 |
| `/converge [@수렴자] <안건>` | **구조화 교차검증.** 독립 검토 → 분류 → 조건부 2라운드 → `REPORT.md` |
| `/cancel [참여자]` | 실행 중 프로세스 종료 (best-effort) |
| `/exit` | 저장 후 종료 |

## 문서

읽는 순서가 정해져 있다.

| # | 파일 | 내용 | 분량 |
|:---:|---|---|---:|
| ★ | **[REBUILD.md](REBUILD.md)** | **Rust + PTY 재작성 계획.** 현재 진행 기준 | 약 9,000자 |
| 0 | [USAGE.md](USAGE.md) | Java 판 사용법. R5 에서 재작성 예정 | 약 8,000자 |
| 1 | **[INTENT.md](INTENT.md)** | **무엇을 왜 만드는가.** 목적·동기·금지사항(N1~N8) | 약 4,600자 |
| 2 | **[SPEC.md](SPEC.md)** | 어떻게 만드는가. 요구사항·환경 실측·설계 스펙·미해결 쟁점 | 약 30,000자 |
| 3 | [BACKGROUND.md](BACKGROUND.md) | 왜 다른 방식을 안 썼는가. 기존 솔루션 조사 | 약 8,000자 |
| — | [ARCHIVE.md](ARCHIVE.md) | **폐기안. 실행 금지 · 동결** | 약 11,800자 |
| ★ | **[REVIEW_REPORT.md](REVIEW_REPORT.md)** | **1차 교차검토 수렴 보고서.** 확정 사항과 사용자 결정 대기 항목 | 약 9,000자 |

**쓰기만 할 거면 [USAGE.md](USAGE.md) 하나면 된다.** 설계를 이해하려면 `INTENT.md` → `SPEC.md` 순으로 읽는다. `BACKGROUND.md`는 "왜 그건 안 썼나"라는 의문이 생겼을 때만 편다.

> ⚠ **`ARCHIVE.md`는 읽지 않는다.** 폐기됐는데도 복붙 가능한 셸 명령과 완성된 스키마를 갖추고 있어 현행 스펙보다 실행 가능해 보인다. AI에게 검토를 맡길 때 이 파일은 제공하지 않는다.

## AI에게 검토를 맡길 때

`INTENT.md` + `SPEC.md` 두 개만 준다 (약 34,600자). 결과는 `codex_review.md` / `agy_review.md` 로 각각 떨어진다. 자세한 내용은 [`prompts/README.md`](prompts/README.md).

```powershell
$P = Get-Content prompts/review-request.md -Raw

# Codex
codex exec --skip-git-repo-check -C . -s workspace-write `
  -c model_reasoning_effort="high" $P

# agy (Gemini)
agy -p $P --add-dir . --model gemini-3.1-pro-high `
  --mode accept-edits --sandbox --disable-slash-commands `
  --print-timeout 10m
```

**에이전트가 리포트 파일을 직접 만든다.** 그래서 쓰기 권한이 필요하다. 대신 프롬프트에서 범위를 좁혔다 — 리포트 1개만 생성, 기존 문서 수정·삭제 금지, 커밋·푸시 금지.

받은 뒤에는 두 리포트를 눈으로 비교하지 말고 수렴 절차를 태운다. 결과물 `REVIEW_REPORT.md` 가 실제 검토·수정 대상이다.

**수렴은 CLI 명령이 아니라 Claude 대화창에 말로 요청한다.**

> consolidate.md 대로 수렴해줘

`consolidate.md` 를 codex·agy에게 보내면 안 된다. 자기 답안을 자기가 채점하는 꼴이 된다.

| 산출물 | 내용 |
|---|---|
| `codex_review.md` / `agy_review.md` | 개별 검토 응답 (근거로 보존) |
| `codex_review_r2.md` / `agy_review_r2.md` | 2라운드 반론 (이견이 남을 때만) |
| **`REVIEW_REPORT.md`** | **수렴 보고서 — 합의·이견·미해결 분류와 결정 요청** |

> 이 흐름 전체가 `SPEC.md` §7.9 **Phase 3(구조화 수렴)** 의 수동 예행연습이다. `REVIEW_REPORT.md` 는 Phase 3이 자동 생성해야 할 출력 형식의 참조 구현이 된다.

## config.properties

`%USERPROFILE%\.multi-ai-cli\config.properties` 로 기본값을 덮어쓴다.

```properties
agy.model=gemini-3.1-pro-high   # D13 기본값
codex.model=                    # 계정이 기본 모델을 거부할 때 지정
claude.path=                    # 자동 탐색 실패 시 실행 파일 경로
codex.path=
agy.path=
```

## 충돌 시 우선순위

```text
INTENT.md > SPEC.md §7 > SPEC.md §4~§6·§8·§9 > SPEC.md §0~§1 > BACKGROUND.md > ARCHIVE.md(참고 불가)
```

## 이력

- 2026-09-03 — codex 초안 작성, agy 실측 검증(K1~K6), Claude 교차 확인(F1~F10)
- 2026-09-03 — 단일 파일 48,058자를 4개 문서로 분리, `INTENT.md` 신설 (F11·F12)

원본 단일 파일은 `_deprecated/multi-ai-cli-orchestration.md.orig` 에 보관돼 있다. 내용은 위 4개 문서에 전부 옮겨졌으므로 확인 후 삭제해도 된다.
