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
  candidates: [
    {
      address: "http://candidate-user:candidate-password@127.0.0.1:12334/private?token=candidate-secret#candidate-fragment",
      source: "windows_system_proxy",
      processName: "Hiddify",
      tcp: { state: "passed", code: "work.bridge_egress.candidate.tcp_ok", durationMs: 2 },
      upstream: { state: "passed", code: "work.bridge_egress.candidate.upstream_ok", httpStatus: 401, durationMs: 120 },
      recommended: true,
      reason: "Windows 系统代理一致，且当前上游可达。",
    },
    {
      address: "",
      source: "direct",
      tcp: { state: "passed", code: "work.bridge_egress.candidate.tcp_ok", durationMs: 0 },
      upstream: { state: "passed", code: "work.bridge_egress.candidate.upstream_ok", httpStatus: 401, durationMs: 170 },
      recommended: false,
      reason: "直连覆盖面较窄。",
    },
  ],
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
  assert.match(prompt, /出口候选/);
  assert.match(prompt, /Windows 系统代理一致，且当前上游可达/);
  assert.match(prompt, /http:\/\/127\.0\.0\.1:12334/);
  assert.ok(prompt.indexOf("http://127.0.0.1:12334") < prompt.indexOf("直连（空值）"));
  assert.match(prompt, /POST body 只含一个键：\{"outbound_proxy_url":"http:\/\/127\.0\.0\.1:12334"\}/);
  assert.match(prompt, /config\.json\.bak-<yyyymmdd-HHMMSS>/);
  assert.match(prompt, /未经我明确批准，不修改 outbound_proxy_url/);
  assert.match(prompt, /不修改系统代理、VPN、DNS、hosts、证书、端口 443/);
});

test("repair Prompt warns when direct is the recommended candidate", () => {
  const directReport: BridgeEgressReport = {
    ...report,
    candidates: report.candidates?.map((candidate) => ({
      ...candidate,
      recommended: candidate.source === "direct",
    })),
  };

  const prompt = buildBridgeEgressRepairPrompt(directReport);
  assert.match(prompt, /直连可能到不了 OpenAI \/ Anthropic/);
  assert.match(prompt, /只有用户确认自己使用的上游全部是国内服务/);
  assert.match(prompt, /POST body 只含一个键：\{"outbound_proxy_url":""\}/);
});

test("repair Prompt keeps upstream reachability ahead of the direct-last tie breaker", () => {
  const mixedReport: BridgeEgressReport = {
    ...report,
    candidates: report.candidates?.map((candidate) => candidate.source === "direct"
      ? { ...candidate, recommended: true }
      : {
        ...candidate,
        recommended: false,
        upstream: {
          state: "failed",
          code: "work.bridge_egress.candidate.upstream_unreachable",
          durationMs: 120,
        },
      }),
  };

  const prompt = buildBridgeEgressRepairPrompt(mixedReport);
  assert.ok(prompt.indexOf("直连（空值）") < prompt.indexOf("http://127.0.0.1:12334"));
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
    "candidate-user",
    "candidate-password",
    "candidate-secret",
    "candidate-fragment",
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
  const applyStart = source.indexOf("async function applyBridgeEgressFix()");
  const bridgeDetailStart = source.indexOf("const bridgeDetail =", applyStart);
  assert.ok(openStart >= 0 && confirmStart > openStart && copyStart > confirmStart && applyStart > copyStart);
  assert.ok(bridgeDetailStart > applyStart);

  const openHandler = source.slice(openStart, confirmStart);
  const confirmHandler = source.slice(confirmStart, copyStart);
  const applyHandler = source.slice(applyStart, bridgeDetailStart);
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
  assert.match(applyHandler, /invoke<BridgeEgressApplyResult>\("apply_bridge_egress_fix"/);
  assert.match(applyHandler, /candidateUrl: recommendedBridgeEgressCandidate\.address/);
  assert.match(source, /应用此修复（会修改 Bridge 出口配置）/);
  assert.equal(source.match(/应用此修复（会修改 Bridge 出口配置）/g)?.length, 1);
  assert.match(source, /应用后的验证会发送 1 次真实请求/);
  assert.match(source, /点击下方按钮即表示同意本次写入与这 1 次验证请求/);
  assert.match(source, /disabled=\{bridgeEgressApplyBusy\}/);
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
    assert.equal(applyHandler.includes(forbidden), false, `WORK apply handler touched ${forbidden}`);
  }
});
