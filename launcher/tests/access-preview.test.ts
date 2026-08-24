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

test("preview access transactions clear stale errors on success", () => {
  const activateKey = functionBody("async function activateKey", "function updateRoleSubscription");
  const saveAggregate = functionBody("async function saveRoleMappings", "function loadAggregateSchemeDraft");
  const confirmAggregate = functionBody("async function confirmPendingAggregateScheme", "function cancelPendingAggregateScheme");

  for (const [name, body] of [
    ["API key activation", activateKey],
    ["aggregate route-table save", saveAggregate],
    ["aggregate scheme confirmation", confirmAggregate],
  ] as const) {
    assert.match(
      body,
      /if \(!isTauri\) \{[\s\S]*?setError\(""\);/,
      `${name} can leave an old error visible after success`,
    );
  }
});

test("the repair restart action names its persistent DrvFS repair side effect", () => {
  assert.match(source, /runAction\("restart_services"\)[\s\S]*?title="备份配置、收窄持久 DrvFS 写授权/);
  assert.match(source, />修复并重启<\/button>/);
});
