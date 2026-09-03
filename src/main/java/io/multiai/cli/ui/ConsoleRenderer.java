package io.multiai.cli.ui;

import io.multiai.cli.provider.ProviderResult;

import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.List;

/**
 * 콘솔 출력. SPEC §5.1 · 요구사항 4.
 *
 * 응답 완료 순서대로 출력하되 한 참여자의 블록 안에 다른 참여자의 출력이 섞이지
 * 않게 한다. 실행 상태·소요 시간·성공 실패를 참여자별로 표시한다.
 *
 * 출력 스트림은 UTF-8 로 고정한다. Windows 콘솔 코드페이지는 run.ps1 에서
 * chcp 65001 로 맞춘다 — 스트림 인코딩과 콘솔 코드페이지는 별개다(§6.3, 교차검토 R11).
 */
public final class ConsoleRenderer {

    private final PrintStream out;

    public ConsoleRenderer() {
        this.out = new PrintStream(new java.io.FileOutputStream(java.io.FileDescriptor.out),
                true, StandardCharsets.UTF_8);
    }

    public void print(String s) { out.println(s); }
    public void blank() { out.println(); }

    public void prompt(String roomName) {
        out.print("multi-ai(" + roomName + ")> ");
        out.flush();
    }

    /** 라운드 시작 시 대상 참여자를 실행 중으로 표시한다. */
    public void running(List<String> displayNames) {
        StringBuilder sb = new StringBuilder();
        for (String n : displayNames) sb.append("[").append(n).append(" · 실행 중] ");
        out.println(sb.toString().stripTrailing());
        out.println();
    }

    /** 발화자 블록. 완료 순서대로 호출된다. */
    public void result(ProviderResult r) {
        out.println("[" + r.displayName() + "] " + badge(r) + "  " + secs(r.elapsed()));
        out.println();
        if (!r.text().isBlank()) {
            out.println(r.text().stripTrailing());
        } else {
            out.println("(응답 없음)");
        }
        if (!r.ok()) {
            // 프로세스 오류를 AI 응답으로 가장하지 않는다 (SPEC §7.7).
            out.println();
            out.println("  ! 실행 실패: " + r.failureNote());
            if (r.stderrFile() != null) out.println("  ! stderr: " + r.stderrFile());
        }
        out.println();
    }

    public void error(String msg) {
        out.println("  ! " + msg);
    }

    public void notice(String msg) {
        out.println("  · " + msg);
    }

    private static String badge(ProviderResult r) {
        return switch (r.status()) {
            case OK -> "성공";
            case UNPARSED -> "성공(원문)";
            case FAILED -> "실패";
            case TIMEOUT -> "타임아웃";
            case CANCELLED -> "취소됨";
            case UNAVAILABLE -> "사용 불가";
        };
    }

    private static String secs(Duration d) {
        return String.format("%.1fs", d.toMillis() / 1000.0);
    }
}
