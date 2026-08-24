import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  buildBridgeEgressRepairPrompt,
  type BridgeEgressReport,
} from "../src/bridgeEgress.ts";

const report: BridgeEgressReport = {
  operation: "bridge_egress",
  ok: false,
  code: "work.bridge_egress.proxy_dead",
  conclusion: "Bridge outbound proxy refused the WSL TCP connection.",
  billableRequestSent: false,
  model: "claude-haiku-4-5-20251001",
  outboundProxyConfigured: true,
  outboundProxyUrl: "http://proxy-user:proxy-password@127.0.0.1:10808/private?token=query-secret#fragment",
  upstreamBaseUrl: "https://api-user:api-password@api.deepseek.com/anthropic?key=upstream-secret#fragment",
  health: { state: "passed", code: "work.bridge_egress.health_ok", httpStatus: 200, durationMs: 15 },
  proxy: { state: "failed", code: "work.bridge_egress.proxy_dead", durationMs: 1 },
  models: { state: "skipped", code: "work.bridge_egress.models_skipped", durationMs: 0 },
  request: { state: "skipped", code: "work.bridge_egress.request_skipped", durationMs: 0 },
  direct: { state: "skipped", code: "work.bridge_egress.direct_skipped", durationMs: 0 },
  suggestedAction: "Clear outbound_proxy_url after approval.",
  warnings: [],
};

test("repair Prompt carries all five layers and the non-gating repair boundary", () => {
  const prompt = buildBridgeEgressRepairPrompt(report);

  assert.match(prompt, /work\.bridge_egress\.proxy_dead/);
  assert.match(prompt, /127\.0\.0\.1:10808/);
  assert.match(prompt, /1\. Bridge \/health/);
  assert.match(prompt, /2\. outbound proxy TCP/);
  assert.match(prompt, /3\. Bridge \/v1\/models/);
  assert.match(prompt, /4\. 最小 \/v1\/messages/);
  assert.match(prompt, /5\. 不经 outbound proxy 的直连对照/);
  assert.match(prompt, /本次是否已发真实请求：否/);
  assert.match(prompt, /仍失败也不得阻塞‘打开 Claude Science’/);
  assert.match(prompt, /csa-smoke\.ps1 -Only \"bridge,egress\"/);
  assert.match(prompt, /未经我明确批准，不修改 outbound_proxy_url/);
  assert.match(prompt, /不修改系统代理、VPN、DNS、hosts、证书、端口 443/);
});

test("repair Prompt strips URL credentials, query strings, and fragments", () => {
  const prompt = buildBridgeEgressRepairPrompt(report);

  assert.match(prompt, /http:\/\/127\.0\.0\.1:10808/);
  assert.match(prompt, /https:\/\/api\.deepseek\.com/);
  assert.equal(prompt.includes("/anthropic"), false, "Prompt leaked an upstream path");
  for (const secret of [
    "proxy-user",
    "proxy-password",
    "query-secret",
    "api-user",
    "api-password",
    "upstream-secret",
    "#fragment",
  ]) {
    assert.equal(prompt.includes(secret), false, `Prompt leaked ${secret}`);
  }
});

test("UI requires a second explicit confirmation and keeps the probe off ALLOW", () => {
  const source = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");
  const openStart = source.indexOf("function openBridgeEgressAssistant()");
  const confirmStart = source.indexOf("async function confirmBridgeEgressCheck()");
  const copyStart = source.indexOf("async function copyBridgeEgressPrompt()");
  assert.ok(openStart >= 0 && confirmStart > openStart && copyStart > confirmStart);

  const openHandler = source.slice(openStart, confirmStart);
  const confirmHandler = source.slice(confirmStart, copyStart);
  assert.equal(openHandler.includes("invoke<"), false, "first click must only open consent UI");
  assert.match(confirmHandler, /run_bridge_egress_check/);
  assert.match(confirmHandler, /confirmBillable: true/);
  assert.equal(source.match(/confirmBillable: true/g)?.length, 1);
  assert.match(source, /同意并开始体检（1 次真实请求）/);
  assert.match(source, /会向当前模型发送一次真实请求/);
  assert.match(source, /max_tokens=1/);
  assert.match(source, /BRIDGE_EGRESS_TIMEOUT_MS = 75_000/);
  assert.match(source, /ref={bridgeEgressDialogRef}/);
  assert.match(source, /event\.key === "Escape"/);
  assert.match(source, /previouslyFocused\?\.focus\(\)/);
  assert.match(styles, /@media \(max-width: 900px\)[\s\S]*bridge-egress-layers[\s\S]*repeat\(2/);

  for (const forbidden of [
    "refreshAllow",
    "setAllowStatus",
    "commitAllowStatus",
    "setStatus",
    "updateAllowActionBusy",
    "setBusy",
  ]) {
    assert.equal(confirmHandler.includes(forbidden), false, `WORK handler touched ${forbidden}`);
  }
});
