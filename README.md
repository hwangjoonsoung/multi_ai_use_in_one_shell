# multi_ai_cli

Claude · Codex · Gemini(agy) 세 AI가 한 채팅방에 참여자로 들어와, 같은 안건에 독립적으로 답한 뒤 서로의 답을 읽고 쟁점을 좁혀가는 독립 CLI 애플리케이션.

**상태**: **Phase 1~4 구현 완료** · SPEC §7.10 완료 기준 전 항목 충족 · Windows 실측 검증 완료

## 실행

```powershell
.\scripts\doctor.ps1                 # CLI 설치·인증·모델 점검
.\scripts\compile.ps1                # javac (외부 라이브러리 없음, D12)
.\scripts
un.ps1                    # 현재 디렉터리를 워크스페이스로 실행
.\scripts
un.ps1 -Workspace C:\proj  # 대상 워크스페이스 지정 (D18)
.\scripts
un.ps1 -Room 20260903-141530   # 기존 방 재개
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
| 0 | **[USAGE.md](USAGE.md)** | **사용법.** 설치 점검부터 명령어·문제 해결까지 | 약 8,000자 |
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
