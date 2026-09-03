package io.multiai.cli.app;

import io.multiai.cli.orchestration.ParallelRoundExecutor;
import io.multiai.cli.orchestration.PromptPresetRepository;
import io.multiai.cli.orchestration.WorkspaceStatus;
import io.multiai.cli.converge.*;
import io.multiai.cli.process.*;
import io.multiai.cli.provider.*;
import io.multiai.cli.room.*;
import io.multiai.cli.ui.ConsoleRenderer;

import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.time.Duration;
import java.util.*;

/**
 * 채팅방 루프. SPEC §5 · §7.4.
 *
 * 사용자 입력 → 명령/멘션 파싱 → 대상 결정 → 공통 문맥 생성 → 대상별 프롬프트
 * → 병렬 실행 → 결과 정규화 → 완료 순서대로 출력 → 라운드 결과를 방에 저장.
 */
public final class ChatApplication implements AutoCloseable {

    /** SPEC D16 — Java 가 전 공급자 공통 600초를 강제한다. */
    private static final Duration TIMEOUT = Duration.ofSeconds(600);

    private final RoomRepository repo;
    private final List<AiProvider> providers;
    private final ProcessLauncher launcher;
    private final ConsoleRenderer ui;
    private final CommandParser parser;
    private final ParallelRoundExecutor executor = new ParallelRoundExecutor();
    private final PromptPresetRepository presets;

    private ChatRoom room;

    public ChatApplication(RoomRepository repo, List<AiProvider> providers,
                           ProcessLauncher launcher, ConsoleRenderer ui, ChatRoom room) {
        this.repo = repo;
        this.providers = providers;
        this.launcher = launcher;
        this.ui = ui;
        this.room = room;
        this.parser = new CommandParser(providerIds());
        this.presets = new PromptPresetRepository(repo.home().resolve("presets.properties"));
        try {
            presets.load();
        } catch (IOException e) {
            ui.error("프리셋을 읽지 못했다: " + e.getMessage());
        }
    }

    private Set<String> providerIds() {
        Set<String> ids = new LinkedHashSet<>();
        for (AiProvider p : providers) ids.add(p.id());
        return ids;
    }

    public void run() throws IOException {
        banner();
        try (BufferedReader in = new BufferedReader(
                new InputStreamReader(System.in, StandardCharsets.UTF_8))) {
            while (true) {
                ui.prompt(room.name());
                String line = in.readLine();
                if (line == null) break;
                if (line.isBlank()) continue;

                CommandParser.Input input = parser.parse(line);
                if (input instanceof CommandParser.Slash s) {
                    if (s.name().equals("exit") || s.name().equals("quit")) break;
                    handleSlash(s);
                } else if (input instanceof CommandParser.Chat c) {
                    if (c.text().isBlank()) {
                        ui.error("보낼 내용이 없다.");
                        continue;
                    }
                    execute(resolveTargets(c.targets()), c.text(), false);
                }
            }
        }
        repo.saveMeta(room);
        ui.notice("기록 저장: " + room.transcript());
    }

    // ---------- 라운드 실행 ----------

    /** 한 라운드를 실행하고 저장한다. 일반 채팅과 /run·/preset run 이 공유한다. */
    private void execute(List<AiProvider> targets, String text, boolean write) throws IOException {
        if (targets.isEmpty()) {
            ui.error("호출 가능한 참여자가 없다. /status 로 확인하라.");
            return;
        }
        int round = room.startRound();
        room.addUser(text);
        Path runDir = repo.runDir(room, round);

        // 문맥은 사용자 메시지를 저장한 뒤 조립한다 — 이번 요청도 포함되어야 한다.
        String prompt;
        try {
            prompt = PromptContextBuilder.build(room, text, "참여자");
        } catch (PromptContextBuilder.RequestTooLargeException e) {
            // 잘라 보내지 않는다 (SPEC §7.2 전송 규약).
            ui.error(e.getMessage());
            ui.notice("요청을 나눠서 보내라.");
            return;
        }

        if (write) {
            ui.notice("쓰기 프로필로 1회 실행한다 — 대상 " + targets.get(0).displayName()
                    + ", 워크스페이스 " + room.workspace());
        }
        ui.running(targets.stream().map(AiProvider::displayName).toList());
        List<ProviderResult> results = executor.run(targets, prompt, room.workspace(),
                write, runDir, repo.tempDir(), TIMEOUT, ui::result);

        for (ProviderResult r : results) {
            room.add(r.providerId(), r.status().name(), r.elapsed().toMillis(),
                    r.text().isBlank() ? "(응답 없음)" : r.text());
        }
        repo.saveMeta(room);

        if (write) reportWorkspaceChanges();

        if (results.stream().noneMatch(ProviderResult::ok)) {
            ui.error("모든 참여자가 실패했다. /status 로 설치·인증 상태를 확인하라.");
        }
    }

    /** 멘션이 없으면 전 참여자. 있으면 지목된 참여자만. */
    private List<AiProvider> resolveTargets(List<String> mentions) {
        if (mentions.isEmpty()) return providers;
        List<AiProvider> out = new ArrayList<>();
        for (AiProvider p : providers) if (mentions.contains(p.id())) out.add(p);
        return out;
    }

    // ---------- 슬래시 명령 ----------

    private void handleSlash(CommandParser.Slash s) throws IOException {
        switch (s.name()) {
            case "status" -> status();
            case "new" -> newRoom(String.join(" ", s.args()));
            case "rooms" -> rooms();
            case "open" -> open(s.args());
            case "cancel" -> cancel(s.args());
            case "run" -> runCommand(s);
            case "preset" -> preset(s);
            case "converge" -> converge(s);
            case "help" -> banner();
            default -> ui.error("알 수 없는 명령: /" + s.name() + "  (/help 로 목록 확인)");
        }
    }

    /**
     * /run [멘션] [--write] [프롬프트]. SPEC §7.5 「쓰기 실행」.
     * 쓰기 프로필은 이 호출 한 번에만 적용되고 세션에 남지 않는다.
     */
    private void runCommand(CommandParser.Slash s) throws IOException {
        RunCommand rc;
        try {
            rc = RunCommand.parse(s.args(), s.raw(), providerIds());
        } catch (RunCommand.ParseException e) {
            ui.error(e.getMessage());
            return;
        }
        execute(resolveTargets(rc.targets()), rc.prompt(), rc.write());
    }

    /**
     * /preset save [이름] [프롬프트] | run [이름] | list | rm [이름]. SPEC §7.5.
     * 프리셋 실행은 쓰기 권한을 승격하지 않는다 (§7.10 완료 기준).
     */
    private void preset(CommandParser.Slash s) throws IOException {
        List<String> a = s.args();
        String sub = a.isEmpty() ? "list" : a.get(0).toLowerCase(Locale.ROOT);
        switch (sub) {
            case "list" -> presetList();
            case "save" -> presetSave(s, a);
            case "run" -> presetRun(a);
            case "rm" -> presetRemove(a);
            default -> ui.error("사용법: /preset [list|save|run|rm] ...");
        }
    }

    private void presetList() {
        if (presets.all().isEmpty()) {
            ui.notice("저장된 프리셋이 없다.  /preset save <이름> [@참여자...] <프롬프트>");
            return;
        }
        ui.blank();
        for (PromptPresetRepository.Preset x : presets.all()) {
            String tg = x.targets().isEmpty() ? "전체" : String.join(",", x.targets());
            ui.print("  " + pad(x.name(), 16) + pad("[" + tg + "]", 20) + preview(x.prompt()));
        }
        ui.blank();
    }

    private void presetSave(CommandParser.Slash s, List<String> a) throws IOException {
        if (a.size() < 2) {
            ui.error("사용법: /preset save <이름> [@참여자...] <프롬프트>");
            return;
        }
        String name = a.get(1);
        String rest = stripTokens(s.raw(), List.of(a.get(0), name));
        List<String> targets = new ArrayList<>();
        while (rest.startsWith("@")) {
            int sp = rest.indexOf(' ');
            String id = (sp < 0 ? rest : rest.substring(0, sp))
                    .substring(1).toLowerCase(Locale.ROOT);
            if (!providerIds().contains(id)) break;
            if (!targets.contains(id)) targets.add(id);
            rest = sp < 0 ? "" : rest.substring(sp + 1).stripLeading();
        }
        if (rest.isBlank()) {
            ui.error("프롬프트가 비어 있다.");
            return;
        }
        presets.save(new PromptPresetRepository.Preset(name, targets, rest));
        ui.notice("프리셋 저장: " + name + (targets.isEmpty() ? " [전체]" : " " + targets));
    }

    private void presetRun(List<String> a) throws IOException {
        if (a.size() < 2) {
            ui.error("사용법: /preset run <이름>");
            return;
        }
        Optional<PromptPresetRepository.Preset> found = presets.get(a.get(1));
        if (found.isEmpty()) {
            ui.error("프리셋을 찾을 수 없다: " + a.get(1));
            return;
        }
        // 프리셋은 권한을 저장하지 않는다. 언제나 읽기 전용으로 실행한다.
        PromptPresetRepository.Preset x = found.get();
        execute(resolveTargets(x.targets()), x.prompt(), false);
    }

    private void presetRemove(List<String> a) throws IOException {
        if (a.size() < 2) {
            ui.error("사용법: /preset rm <이름>");
            return;
        }
        ui.notice(presets.remove(a.get(1))
                ? "삭제됨: " + a.get(1)
                : "프리셋을 찾을 수 없다: " + a.get(1));
    }

    // ---------- Phase 3: 구조화 수렴 ----------

    /**
     * /converge [@수렴자] 안건. SPEC §7.5-1 · §7.9.
     *
     * 수렴자로 지명된 참여자는 검토 대상에서 제외된다 — 자기 답을 자기가 분류하면
     * 자기 채점이 된다. 지명이 없으면 규칙 기반 분류만 수행한다.
     */
    private void converge(CommandParser.Slash s) throws IOException {
        List<String> a = s.args();
        String consolidator = null;
        int skip = 0;
        if (!a.isEmpty() && a.get(0).startsWith("@")) {
            String id = a.get(0).substring(1).toLowerCase(Locale.ROOT);
            if (!providerIds().contains(id)) {
                ui.error("알 수 없는 수렴자: @" + id);
                return;
            }
            consolidator = id;
            skip = 1;
        }
        String subject = stripTokens(s.raw(), a.subList(0, skip));
        if (subject.isBlank()) {
            ui.error("사용법: /converge [@수렴자] <안건>");
            ui.notice("수렴자를 지명하면 그 참여자는 검토에서 빠진다 (§7.5-1).");
            return;
        }

        final String cons = consolidator;
        List<AiProvider> reviewers = providers.stream()
                .filter(p -> cons == null || !p.id().equals(cons)).toList();
        if (reviewers.size() < 2) {
            ui.error("검토자가 2명 미만이다. 수렴에는 최소 2명이 필요하다.");
            return;
        }

        int round = room.startRound();
        room.addUser("[converge] " + subject);
        Path outDir = repo.runDir(room, round).resolve("converge");

        ui.blank();
        ui.notice("검토자: " + reviewers.stream().map(AiProvider::displayName).toList());
        ui.notice(cons == null ? "수렴자: 규칙 기반 (모델 호출 없음)"
                : "수렴자: " + cons + " (검토에서 제외됨)");

        ConvergeSession session = new ConvergeSession(executor, repo.tempDir(), TIMEOUT);
        ConvergeSession.Result res = session.run(reviewers, subject, room.workspace(), outDir,
                new ConvergeSession.Progress() {
                    @Override public void stage(String m) { ui.notice(m); }
                    @Override public void reviewerDone(StructuredReview r) {
                        ui.print("  [" + r.reviewerName() + "] " + r.verdict()
                                + " · 지적 " + r.issues().size() + "건");
                    }
                });

        if (res.aborted()) {
            ui.error(res.abortReason());
            repo.saveMeta(room);
            return;
        }
        printConvergeSummary(res);
        room.add("consolidator", "OK", 0, "수렴 보고서: " + res.report());
        repo.saveMeta(room);
    }

    /** 터미널에는 판정 요약과 미해결만 낸다. 전문은 경로만 안내한다 (§7.5-1). */
    private void printConvergeSummary(ConvergeSession.Result res) {
        ConsolidationEngine.Outcome last = res.round2() != null ? res.round2() : res.round1();
        ui.blank();
        ui.print("== 판정 요약 ==");
        for (StructuredReview r : last.reviews()) {
            ui.print("  " + pad(r.reviewerName(), 18)
                    + (r.valid() ? r.verdict().toString() : "응답 없음"));
        }
        if (res.round1().partial()) {
            ui.error("PARTIAL — 응답 없음: " + String.join(", ", res.round1().failedReviewers()));
        }

        long agree = last.findings().stream()
                .filter(f -> f.bucket() == ConsolidationEngine.Bucket.합의).count();
        long dis = last.findings().stream()
                .filter(f -> f.bucket() == ConsolidationEngine.Bucket.이견).count();
        long solo = last.findings().stream()
                .filter(f -> f.bucket() == ConsolidationEngine.Bucket.단독지적).count();
        ui.blank();
        ui.print("  합의 " + agree + " · 이견 " + dis + " · 단독 지적 " + solo
                + " · 미해결 " + last.openQuestions().size());

        if (!last.openQuestions().isEmpty()) {
            ui.blank();
            ui.print("== 사용자 결정 필요 ==");
            last.openQuestions().forEach(q -> ui.print("  - " + q));
        }
        ui.blank();
        ui.notice("보고서: " + res.report());
        ui.blank();
    }

    /** SPEC §7.5 — 쓰기 실행 후 변경 목록만 보여준다. 커밋·푸시는 하지 않는다. */
    private void reportWorkspaceChanges() {
        WorkspaceStatus.Report rep = WorkspaceStatus.collect(room.workspace());
        ui.blank();
        ui.print("== 워크스페이스 변경 (" + rep.vcs() + ") ==");
        if (!rep.note().isBlank()) ui.notice(rep.note());
        if (rep.hasChanges()) {
            rep.lines().forEach(l -> ui.print("  " + l));
            ui.blank();
            ui.notice("커밋·푸시는 수행하지 않는다. 필요하면 직접 실행하라.");
        } else if (rep.vcs() != WorkspaceStatus.Vcs.NONE) {
            ui.notice("변경 없음");
        }
        ui.blank();
    }

    // ---------- 방 관리 ----------

    private void status() {
        ui.blank();
        ui.print("== 참여자 ==");
        for (AiProvider p : providers) {
            ResolvedCommand c = p.command();
            String tier = c.isPrimary() ? "" : "  [강등 tier " + c.tier() + "]";
            ui.print("  " + pad(p.displayName(), 18) + c.executable() + tier);
            if (!c.note().isBlank()) ui.print("      " + c.note());
        }
        ui.blank();
        ui.print("== 방 ==");
        ui.print("  id        " + room.id());
        ui.print("  이름      " + room.name());
        ui.print("  워크스페이스 " + room.workspace());
        ui.print("  라운드    " + room.round() + " / 메시지 " + room.messages().size());
        long dmg = room.damagedCount();
        if (dmg > 0) ui.error("복원 시 손상·의심 메시지 " + dmg + "건 (SUSPECT/CORRUPT)");
        ui.print("  저장 위치 " + repo.home());
        ui.print("  프리셋    " + presets.all().size() + "건");
        ui.blank();
    }

    private void newRoom(String name) throws IOException {
        repo.saveMeta(room);
        room = repo.create(name, room.workspace());
        ui.notice("새 방: " + room.name() + " (" + room.id() + ")");
    }

    private void rooms() throws IOException {
        List<RoomRepository.RoomSummary> list = repo.list();
        if (list.isEmpty()) {
            ui.notice("저장된 방이 없다.");
            return;
        }
        ui.blank();
        for (RoomRepository.RoomSummary r : list) {
            String cur = r.id().equals(room.id()) ? " *" : "";
            ui.print("  " + pad(r.id(), 18) + pad(r.name(), 24) + "라운드 " + r.round() + cur);
        }
        ui.blank();
    }

    private void open(List<String> args) throws IOException {
        if (args.isEmpty()) {
            ui.error("사용법: /open <방 ID>   (/rooms 로 목록 확인)");
            return;
        }
        try {
            repo.saveMeta(room);
            ChatRoom opened = repo.open(args.get(0));
            // SPEC D18-5 — 저장된 워크스페이스가 없어졌으면 조용히 대체하지 않는다.
            if (!Files.isDirectory(opened.workspace())) {
                ui.error("저장된 워크스페이스가 존재하지 않는다: " + opened.workspace());
                ui.notice("이 방은 열지 않는다. 경로를 복구하거나 새 방을 만들어라.");
                return;
            }
            room = opened;
            ui.notice("방 열기: " + room.name() + "  메시지 " + room.messages().size() + "건");
            long dmg = room.damagedCount();
            if (dmg > 0) {
                ui.error("복원 중 손상·의심 " + dmg + "건. 해당 메시지는 문맥에서 제외되거나 의심 표시된다.");
            }
        } catch (NoSuchFileException e) {
            ui.error("방을 찾을 수 없다: " + args.get(0));
        }
    }

    /** SPEC §6.3 — 지정 참여자의 프로세스 트리만 종료한다. best-effort 다. */
    private void cancel(List<String> args) {
        if (args.isEmpty()) {
            for (AiProvider p : providers) launcher.cancel(p.id());
            ui.notice("전체 취소 요청. 생존 프로세스가 있으면 오류로 표시된다.");
            return;
        }
        String id = args.get(0).replaceFirst("^@", "").toLowerCase(Locale.ROOT);
        launcher.cancel(id);
        ui.notice("취소 요청: " + id);
    }

    private void banner() {
        ui.blank();
        ui.print("multi_ai_cli — Claude · Codex · Gemini(agy) 한 방에서");
        ui.print("  일반 문장          전 참여자 동시 호출");
        ui.print("  @claude/@codex/@gemini <질문>   지목 호출");
        ui.print("  /run @<참여자> [--write] <프롬프트>   지정 권한으로 1회 실행");
        ui.print("  /preset [list|save|run|rm] ...   프롬프트 프리셋 (권한 승격 없음)");
        ui.print("  /converge [@수렴자] <안건>   구조화 교차검증 → REPORT.md");
        ui.print("  /status /rooms /open <ID> /new [이름] /cancel [참여자] /exit");
        ui.blank();
    }

    // ---------- 헬퍼 ----------

    private static String pad(String s, int n) {
        return s.length() >= n ? s + " " : s + " ".repeat(n - s.length());
    }

    private static String preview(String s) {
        String one = s.replace('\n', ' ').strip();
        return one.length() <= 40 ? one : one.substring(0, 40) + "...";
    }

    /** raw 앞쪽에서 지정 토큰들을 순서대로 걷어낸다. */
    private static String stripTokens(String raw, List<String> tokens) {
        String rest = raw;
        for (String t : tokens) {
            int idx = rest.indexOf(t);
            if (idx < 0) break;
            rest = rest.substring(idx + t.length()).stripLeading();
        }
        return rest.strip();
    }

    @Override
    public void close() {
        executor.close();
        launcher.shutdown();
    }
}
