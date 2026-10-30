// @vitest-environment jsdom
import { beforeEach, expect, it } from "vitest";
import type { QuotaSnapshot, QuotaSnapshotView } from "@dogcodes/protocol";
import { withLastReading } from "./quotaFallback";
import { welcomeStatus } from "./AgentWelcome";

const reading = (collectedAtUnixMs: number, extra: Partial<QuotaSnapshot> = {}): QuotaSnapshot => ({
  windows: [{ id: "five_hour", windowMinutes: 300, usedPercent: 30, resetsAtUnix: null }],
  collectedAtUnixMs, staleAfterUnixMs: collectedAtUnixMs + 60_000,
  source: "native_pull", confidence: "provider_reported", parserVersion: "test", ...extra,
});
const view = (quota: QuotaSnapshot | null, diagnostic: string | null = null): QuotaSnapshotView =>
  ({ providerId: "claude-code", installationId: "ins_claude", quota, diagnostic });

beforeEach(() => localStorage.clear());

it("does not restore a provider account's old reading after credentials rotate", () => {
  for (const providerId of ["opencode", "pi", "deepseek-harness"]) {
    withLastReading({ ...view(reading(1_000, { source: "provider_api" })), providerId });
    expect(withLastReading({ ...view(null, "quota_rate_limited"), providerId }).quota).toBeNull();
  }
});

it("keeps the last successful reading while reads are rate limited, across restarts", () => {
  const good = view(reading(1_000, { context: { usedTokens: 1, windowTokens: 2 } as QuotaSnapshot["context"] }));
  expect(withLastReading(good)).toBe(good);
  // Nothing but localStorage carries the reading to the next call, as after a restart.
  const limited = withLastReading(view(null, "claude_quota_rate_limited"));
  expect(limited.diagnostic).toBe("claude_quota_rate_limited");
  expect(limited.quota?.windows[0].usedPercent).toBe(30);
  expect(limited.quota?.collectedAtUnixMs).toBe(1_000);
  // Conversation context is not carried over from the account reading.
  expect(limited.quota?.context).toBeNull();
});

it("only stands in for rate limiting, never for login or other failures", () => {
  withLastReading(view(reading(1_000)));
  expect(withLastReading(view(null, "claude_quota_login_required")).quota).toBeNull();
  expect(withLastReading(view(null, "quota_network_error")).quota).toBeNull();
  expect(withLastReading(view(null, "quota_rate_limited")).quota?.collectedAtUnixMs).toBe(1_000);
});

it("does not remember demo readings", () => {
  withLastReading(view(reading(1_000, { confidence: "demo" })));
  expect(withLastReading(view(null, "claude_quota_rate_limited")).quota).toBeNull();
});

it("labels a remembered reading on the welcome page with when it was taken", () => {
  withLastReading(view(reading(Date.UTC(2026, 9, 16, 1, 30))));
  const limited = withLastReading(view(null, "claude_quota_rate_limited"));
  const installation = { installationId: "ins_claude", providerId: "claude-code", status: "ready", version: "2.1.0" } as never;
  const status = welcomeStatus({ installation, view: limited });
  expect(status.label).toBe("额度查询受限");
  expect(status.detail).toContain("的读数");
});

it("keeps the last reading when Claude Code's token awaits renewal, without asking to log in again", () => {
  withLastReading(view(reading(1_000)));
  const expired = withLastReading(view(null, "claude_quota_token_expired"));
  expect(expired.quota?.collectedAtUnixMs).toBe(1_000);
  const installation = { installationId: "ins_claude", providerId: "claude-code", status: "ready", version: "2.1.0" } as never;
  const status = welcomeStatus({ installation, view: expired });
  expect(status.label).toBe("令牌待续期");
  expect(status.detail).toContain("自动续期");
  expect(status.detail).toContain("无需重新登录");
});
