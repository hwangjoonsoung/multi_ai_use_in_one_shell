package io.multiai.cli.app;

import io.multiai.cli.orchestration.ParallelRoundExecutor;
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

    private ChatRoom room;

    public ChatApplication(RoomRepository repo, List<AiProvider> providers,
                           ProcessLauncher launcher, ConsoleRenderer ui, ChatRoom room) {
        this.repo = repo;
        this.providers = providers;
        this.launcher = launcher;
        this.ui = ui;
        this.room = room;
        Set<String> ids = new LinkedHashSet<>();
        for (AiProvider p : providers) ids.add(p.id());
        this.parser = new CommandParser(ids);
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
                    handleChat(c);
                }
            }
        }
        repo.saveMeta(room);
        ui.notice("기록 저장: " + room.transcript());
    }

    // ---------- 채팅 ----------

    private void handleChat(CommandParser.Chat c) throws IOException {
        List<AiProvider> targets = resolveTargets(c.targets());
        if (targets.isEmpty()) {
            ui.error("호출 가능한 참여자가 없다. /status 로 확인하라.");
            return;
        }

        int round = room.startRound();
        room.addUser(c.text());
        Path runDir = repo.runDir(room, round);

        // 문맥은 사용자 메시지를 저장한 뒤 조립한다 — 이번 요청도 포함되어야 한다.
        String prompt;
        try {
            prompt = PromptContextBuilder.build(room, c.text(), "참여자");
        } catch (PromptContextBuilder.RequestTooLargeException e) {
            // 잘라 보내지 않는다 (SPEC §7.2 전송 규약).
            ui.error(e.getMessage());
            ui.notice("요청을 나눠서 보내라.");
            return;
        }

        ui.running(targets.stream().map(AiProvider::displayName).toList());
        List<ProviderResult> results = executor.run(targets, prompt, room.workspace(),
                false, runDir, repo.tempDir(), TIMEOUT, ui::result);

        for (ProviderResult r : results) {
            room.add(r.providerId(), r.status().name(), r.elapsed().toMillis(),
                    r.text().isBlank() ? "(응답 없음)" : r.text());
        }
        repo.saveMeta(room);

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
            case "help" -> banner();
            default -> ui.error("알 수 없는 명령: /" + s.name() + "  (/help 로 목록 확인)");
        }
    }

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
            ui.print("  " + pad(r.id(), 18) + pad(r.name(), 24)
                    + "라운드 " + r.round() + cur);
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
            if (dmg > 0) ui.error("복원 중 손상·의심 " + dmg + "건. 해당 메시지는 문맥에서 제외되거나 의심 표시된다.");
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
        ui.print("  /status /rooms /open <ID> /new [이름] /cancel [참여자] /exit");
        ui.blank();
    }

    private static String pad(String s, int n) {
        return s.length() >= n ? s + " " : s + " ".repeat(n - s.length());
    }

    @Override
    public void close() {
        executor.close();
        launcher.shutdown();
    }
}
