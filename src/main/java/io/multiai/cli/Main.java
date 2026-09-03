package io.multiai.cli;

import io.multiai.cli.app.ChatApplication;
import io.multiai.cli.process.*;
import io.multiai.cli.provider.*;
import io.multiai.cli.room.*;
import io.multiai.cli.ui.ConsoleRenderer;

import java.io.IOException;
import java.nio.file.*;
import java.util.*;

/**
 * 진입점. SPEC §7.1.
 *
 * 사용법: multi-ai [--workspace <path>] [--room <id>] [--home <path>]
 *
 * --workspace 는 대상 워크스페이스를 지정한다(D18). 생략하면 현재 디렉터리다.
 */
public final class Main {

    /** SPEC D13 — agy 기본 모델. config.properties 로 덮어쓸 수 있다. */
    private static final String DEFAULT_AGY_MODEL = "gemini-3.1-pro-high";

    public static void main(String[] args) {
        try {
            System.exit(realMain(args));
        } catch (Exception e) {
            System.err.println("치명적 오류: " + e);
            System.exit(2);
        }
    }

    private static int realMain(String[] args) throws IOException {
        Map<String, String> opt = parseOptions(args);
        ConsoleRenderer ui = new ConsoleRenderer();

        Path home = opt.containsKey("home")
                ? Path.of(opt.get("home")) : RoomRepository.defaultHome();
        RoomRepository repo = new RoomRepository(home);
        repo.ensureLayout();

        // D18 — 워크스페이스는 절대 경로로 정규화해 방에 고정한다.
        Path workspace = Path.of(opt.getOrDefault("workspace", System.getProperty("user.dir")))
                .toAbsolutePath().normalize();
        if (!Files.isDirectory(workspace)) {
            ui.error("워크스페이스가 디렉터리가 아니다: " + workspace);
            return 1;
        }

        Properties cfg = repo.loadConfig();
        CommandResolver resolver = new WindowsCommandResolver(overridesFrom(cfg));
        ProcessLauncher launcher = new WindowsProcessLauncher(repo.tempDir());

        List<AiProvider> providers = buildProviders(resolver, launcher, cfg, ui);
        if (providers.isEmpty()) {
            ui.error("사용 가능한 CLI 가 하나도 없다. claude / codex / agy 설치와 PATH 를 확인하라.");
            return 1;
        }

        ChatRoom room = opt.containsKey("room")
                ? repo.open(opt.get("room"))
                : repo.create(null, workspace);

        try (ChatApplication app = new ChatApplication(repo, providers, launcher, ui, room)) {
            Runtime.getRuntime().addShutdownHook(new Thread(launcher::shutdown, "cleanup"));
            app.run();
        }
        return 0;
    }

    /** 해석에 실패한 공급자는 제외하고 나머지로 진행한다 (§4.3-6 부분 실패 허용). */
    private static List<AiProvider> buildProviders(CommandResolver resolver, ProcessLauncher launcher,
                                                   Properties cfg, ConsoleRenderer ui) {
        List<AiProvider> out = new ArrayList<>();
        resolver.resolve("claude").ifPresentOrElse(
                c -> out.add(new ClaudeCliAdapter(c, launcher)),
                () -> ui.error("claude 를 찾지 못했다 — 이 참여자는 비활성이다."));
        String codexModel = cfg.getProperty("codex.model");
        resolver.resolve("codex").ifPresentOrElse(
                c -> out.add(new CodexCliAdapter(c, launcher, codexModel)),
                () -> ui.error("codex 를 찾지 못했다 — codex.cmd/.ps1 폴백은 쓰지 않는다(§6.3). 비활성."));
        String model = cfg.getProperty("agy.model", DEFAULT_AGY_MODEL);
        resolver.resolve("agy").ifPresentOrElse(
                c -> out.add(new AgyCliAdapter(c, launcher, model)),
                () -> ui.error("agy 를 찾지 못했다 — 이 참여자는 비활성이다."));
        return out;
    }

    private static Map<String, Path> overridesFrom(Properties cfg) {
        Map<String, Path> m = new HashMap<>();
        for (String name : List.of("claude", "codex", "agy")) {
            String v = cfg.getProperty(name + ".path");
            if (v != null && !v.isBlank()) m.put(name, Path.of(v.trim()));
        }
        return m;
    }

    private static Map<String, String> parseOptions(String[] args) {
        Map<String, String> m = new HashMap<>();
        for (int i = 0; i < args.length; i++) {
            if (args[i].startsWith("--") && i + 1 < args.length) {
                m.put(args[i].substring(2), args[++i]);
            }
        }
        return m;
    }
}
