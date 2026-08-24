import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

function functionBody(startMarker: string, endMarker: string) {
  const start = source.indexOf(startMarker);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.ok(start >= 0, `missing ${startMarker}`);
  assert.ok(end > start, `missing ${endMarker} after ${startMarker}`);
  return source.slice(start, end);
}

function previewBranch(startMarker: string, endMarker: string, tauriMarker: string) {
  const body = functionBody(startMarker, endMarker);
  const start = body.indexOf("if (!isTauri) {");
  const end = body.indexOf(tauriMarker, start);
  assert.ok(start >= 0, `missing preview branch in ${startMarker}`);
  assert.ok(end > start, `missing Tauri boundary in ${startMarker}`);
  return body.slice(start, end);
}

test("preview access transactions clear stale errors on success", () => {
  for (const [name, body] of [
    ["API key save", previewBranch("async function applyDraftKey", "async function testDraftApiKey", "if (!tryBeginMutation())")],
    ["API key test", previewBranch("async function testDraftApiKey", "async function autoMapDraftApiKey", "setTestingKey(true)")],
    ["API key auto-map", previewBranch("async function autoMapDraftApiKey", "async function activateKey", "setAutoMappingKey(true)")],
    ["API key activation", previewBranch("async function activateKey", "function updateRoleSubscription", "if (!tryBeginMutation())")],
    ["aggregate route-table save", previewBranch("async function saveRoleMappings", "function loadAggregateSchemeDraft", "if (!tryBeginMutation())")],
    ["aggregate scheme confirmation", previewBranch("async function confirmPendingAggregateScheme", "function cancelPendingAggregateScheme", "if (!tryBeginMutation())")],
    ["API key deletion", previewBranch("async function deleteKey", "async function primaryAction", "if (!tryBeginMutation())")],
  ] as const) {
    assert.match(body, /setError\(""\);/, `${name} can leave an old error visible after success`);
  }
});

test("the repair restart action names its persistent DrvFS repair side effect", () => {
  assert.match(source, /runAction\("restart_services"\)[\s\S]*?title="备份配置、收窄持久 DrvFS 写授权/);
  assert.match(source, />修复并重启<\/button>/);
});
