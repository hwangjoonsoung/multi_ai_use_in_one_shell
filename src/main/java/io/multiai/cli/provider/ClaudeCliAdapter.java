package io.multiai.cli.provider;

import io.multiai.cli.process.*;

import java.nio.file.Path;
import java.time.Duration;
import java.util.List;

/**
 * Claude Code CLI 어댑터. SPEC §7.2.
 *
 * 읽기 전용:  -p --add-dir <ws> --input-format text --output-format text
 *             --restricted --permission-mode plan --permission-prompts none --tools ""
 * 쓰기 허용:  --permission-mode acceptEdits (그 외 동일)
 *
 * --restricted 는 필수다(D17). 없으면 Claude 만 사용자 CLAUDE.md·MCP 설정을
 * 상속해 세 공급자의 출발 조건이 달라진다.
 * 프롬프트는 stdin 으로 주입한다(§5.3 실측).
 */
public final class ClaudeCliAdapter extends AbstractCliProvider {

    public ClaudeCliAdapter(ResolvedCommand c, ProcessLauncher l) { super(c, l); }

    @Override public String id() { return "claude"; }
    @Override public String displayName() { return "Claude"; }
    @Override public boolean acceptsStdin() { return true; }

    @Override
    protected List<String> arguments(String prompt, Path ws, boolean write,
                                     Path tempDir, Duration timeout) {
        return List.of(
                "-p",
                "--add-dir", ws.toString(),
                "--input-format", "text",
                "--output-format", "text",
                "--restricted",
                "--permission-mode", write ? "acceptEdits" : "plan",
                "--permission-prompts", "none",
                "--tools", write ? "default" : "");
    }

    @Override
    protected String extractText(RunOutcome out, Path tempDir) {
        return out.stdout().strip();
    }
}
