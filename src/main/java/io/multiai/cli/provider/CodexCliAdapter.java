package io.multiai.cli.provider;

import io.multiai.cli.process.*;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.time.Duration;
import java.util.List;
import java.util.UUID;

/**
 * Codex CLI 어댑터. SPEC §7.2.
 *
 * 읽기 전용: codex exec - --skip-git-repo-check -C <ws> -s read-only
 *            -c model_reasoning_effort="high" -o <temp>/<run-id>.md
 * 쓰기 허용: -s workspace-write (그 외 동일)
 *
 * 끝의 "-" 가 stdin 에서 프롬프트를 읽으라는 지시다 (codex exec --help 실측).
 * -o 임시 파일은 워크스페이스가 아니라 %USERPROFILE%\.multi-ai-cli\temp\ 에 만든다 —
 * read-only 샌드박스와의 충돌을 피하고 워크스페이스를 오염시키지 않는다(§7.6).
 * 파일은 try-finally 로 정리하고, 없거나 비면 stdout 으로 폴백한다.
 */
public final class CodexCliAdapter extends AbstractCliProvider {

    private volatile Path lastOutFile;

    public CodexCliAdapter(ResolvedCommand c, ProcessLauncher l) { super(c, l); }

    @Override public String id() { return "codex"; }
    @Override public String displayName() { return "Codex"; }
    @Override public boolean acceptsStdin() { return true; }

    @Override
    protected List<String> arguments(String prompt, Path ws, boolean write,
                                     Path tempDir, Duration timeout) {
        Path out = tempDir.resolve(UUID.randomUUID() + ".md");
        lastOutFile = out;
        return List.of(
                "exec", "-",
                "--skip-git-repo-check",
                "-C", ws.toString(),
                "-s", write ? "workspace-write" : "read-only",
                "-c", "model_reasoning_effort=\"high\"",
                "-o", out.toString());
    }

    @Override
    protected String extractText(RunOutcome out, Path tempDir) {
        Path f = lastOutFile;
        try {
            if (f != null && Files.isRegularFile(f)) {
                String s = Files.readString(f, StandardCharsets.UTF_8).strip();
                if (!s.isBlank()) return s;
            }
        } catch (IOException ignored) {
            // 파일이 불완전하면 스트림으로 폴백한다.
        } finally {
            deleteQuietly(f);
        }
        return out.stdout().strip();
    }

    private static void deleteQuietly(Path f) {
        if (f == null) return;
        try {
            Files.deleteIfExists(f);
        } catch (IOException ignored) {
            // 정리 실패가 라운드를 중단시키지 않는다.
        }
    }
}
