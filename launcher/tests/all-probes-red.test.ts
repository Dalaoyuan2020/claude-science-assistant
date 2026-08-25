import assert from "node:assert/strict";
import test from "node:test";

import {
  classifyNonGatingFailure,
  createLaneState,
  createProbeCircuit,
  laneReducer,
  nonGatingFailurePresentation,
  nonGatingFailureIsMuted,
  primaryButtonView,
  runNonGatingProbe,
  type AllowStatus,
} from "../src/laneContract.ts";

test("all_probes_red_button_still_open / bridge_egress_never_gates", () => {
  const allow: AllowStatus = {
    wslInstalled: true,
    distro: "Ubuntu-24.04",
    linuxUser: "tester",
    runtimePresent: true,
    claudeRunning: true,
    claudePid: 42,
    listenerPresent: true,
    pid8765: 42,
    pid8766: 42,
    daemonState: "managed_ready",
    listenerProbeOk: true,
    controlSocketPresent: true,
    windowsBridgeProbe: "checked",
    canOpen: true,
    canStart: false,
  };
  const initialGrade = { state: "running", warnings: [] as string[] };
  const initialWork = { state: "idle" };
  const before = createLaneState(allow, initialGrade, initialWork);
  const beforeAllowSnapshot = structuredClone(before.allow);
  const beforeButton = primaryButtonView(before.allow, true, false);

  const allRedGrade = {
    state: "error",
    bridgeIdentity: "mismatch",
    proxyState: "conflict",
    sandboxForwarders: "0/3",
    daemonProcessState: "D",
    daemonWaitChannel: "p9_client_rpc",
    diskBlocked: true,
    broadDrvfsGrant: true,
    egressHttpStatus: 502,
    warnings: [
      "grade.bridge.identity_mismatch",
      "grade.proxy.conflict",
      "grade.sandbox.topology_failed",
      "grade.drvfs.p9_blocked",
      "grade.disk.blocked",
    ],
  };
  const allRedWork = {
    state: "error",
    canary: "failed",
    bridgeEgress: "proxy_dead",
    networkQuality: "timeout",
    runtimeUpdate: "timeout",
    warnings: [
      "work.sandbox_canary.failed",
      "work.bridge_egress.proxy_dead",
      "work.network_quality.timeout",
      "work.runtime_update.timeout",
    ],
  };

  const afterGrade = laneReducer(before, { lane: "grade", value: allRedGrade });
  const afterAllRed = laneReducer(afterGrade, { lane: "work", value: allRedWork });
  const afterButton = primaryButtonView(afterAllRed.allow, true, false);

  assert.strictEqual(afterGrade.allow, before.allow, "GRADE must preserve the ALLOW object");
  assert.strictEqual(afterAllRed.allow, before.allow, "WORK must preserve the ALLOW object");
  assert.deepEqual(afterAllRed.allow, beforeAllowSnapshot, "ALLOW PID/port fields must not be overwritten");
  assert.deepEqual(afterButton, beforeButton, "all-red probes must not change the primary button");
  assert.deepEqual(afterButton, {
    action: "open",
    label: "打开 Claude Science",
    disabled: false,
  });
  assert.match(
    nonGatingFailurePresentation("work.bridge_egress.proxy_dead", afterAllRed.allow),
    /仍可打开 Claude Science/,
  );
});

test("non_gating_probe_timeout_opens_circuit_and_ignores_late_result", async () => {
  const circuit = createProbeCircuit();
  let taskCalls = 0;
  const first = await runNonGatingProbe({
    lane: "work",
    key: "network_quality",
    timeoutMs: 5,
    cooldownMs: 1_000,
    circuit,
    task: async () => {
      taskCalls += 1;
      await new Promise((resolve) => setTimeout(resolve, 25));
      return "late-success";
    },
  });

  assert.equal(first.ok, false);
  if (first.ok) assert.fail("timeout probe unexpectedly succeeded");
  assert.equal(first.code, "work.network_quality.timeout");
  assert.equal(first.timedOut, true);
  assert.equal(nonGatingFailureIsMuted(first.code, first.timedOut, first.skipped), true);
  const openUntilAfterTimeout = circuit.openUntil;

  await new Promise((resolve) => setTimeout(resolve, 35));
  assert.equal(circuit.openUntil, openUntilAfterTimeout, "late completion must not close the circuit");

  const second = await runNonGatingProbe({
    lane: "work",
    key: "network_quality",
    timeoutMs: 5,
    cooldownMs: 1_000,
    circuit,
    task: async () => {
      taskCalls += 1;
      return "must-not-run";
    },
  });
  assert.equal(second.ok, false);
  if (second.ok) assert.fail("open circuit unexpectedly ran the probe");
  assert.equal(second.code, "work.network_quality.circuit_open");
  assert.equal(second.skipped, true);
  assert.equal(taskCalls, 1);
});

test("newer_probe_supersedes_older_call_without_leaving_its_busy_state_pending", async () => {
  const circuit = createProbeCircuit();
  let releaseOlder: ((value: string) => void) | undefined;
  const older = runNonGatingProbe({
    lane: "grade",
    key: "status",
    timeoutMs: 100,
    circuit,
    task: () => new Promise<string>((resolve) => {
      releaseOlder = resolve;
    }),
  });
  await Promise.resolve();

  const newer = runNonGatingProbe({
    lane: "grade",
    key: "status",
    timeoutMs: 100,
    circuit,
    task: async () => "newer-result",
  });
  const newerResult = await newer;
  assert.equal(newerResult.ok, true);

  assert.ok(releaseOlder, "older task should have started");
  releaseOlder("older-result");
  const olderResult = await Promise.race([
    older,
    new Promise<never>((_, reject) => setTimeout(() => reject(new Error("older probe stayed pending")), 50)),
  ]);
  assert.equal(olderResult.ok, false);
  if (olderResult.ok) assert.fail("older result unexpectedly won after supersession");
  assert.equal(olderResult.code, "grade.status.superseded");
  assert.equal(olderResult.skipped, true);
});

test("semantic_timeouts_are_gray_but_confirmed_proxy_failures_are_not", () => {
  assert.equal(nonGatingFailureIsMuted("work.network_quality.timeout"), true);
  assert.equal(nonGatingFailureIsMuted("work.network_quality.daemon_mount_io_busy"), true);
  assert.equal(nonGatingFailureIsMuted("grade.status.circuit_open"), true);
  assert.equal(nonGatingFailureIsMuted("work.bridge_egress.proxy_dead"), false);
});

test("backend_timeout_rejection_keeps_timeout_code_gray_and_opens_circuit", async () => {
  const circuit = createProbeCircuit();
  let taskCalls = 0;
  const first = await runNonGatingProbe({
    lane: "work",
    key: "runtime_update",
    timeoutMs: 100,
    cooldownMs: 1_000,
    circuit,
    task: async () => {
      taskCalls += 1;
      throw new Error("work.runtime_update.timeout: backend deadline exceeded");
    },
  });
  assert.equal(first.ok, false);
  if (first.ok) assert.fail("backend timeout unexpectedly succeeded");
  assert.equal(first.code, "work.runtime_update.timeout");
  assert.equal(first.timedOut, true);
  assert.equal(nonGatingFailureIsMuted(first.code, first.timedOut, first.skipped), true);

  const second = await runNonGatingProbe({
    lane: "work",
    key: "runtime_update",
    timeoutMs: 100,
    cooldownMs: 1_000,
    circuit,
    task: async () => {
      taskCalls += 1;
      return "must-not-run";
    },
  });
  assert.equal(second.ok, false);
  if (second.ok) assert.fail("backend timeout circuit unexpectedly ran the task");
  assert.equal(second.code, "work.runtime_update.circuit_open");
  assert.equal(taskCalls, 1);
});

test("semantic_grade_and_work_timeouts_open_their_circuits", async () => {
  const gradeCircuit = createProbeCircuit();
  const grade = await runNonGatingProbe({
    lane: "grade",
    key: "status",
    timeoutMs: 100,
    cooldownMs: 1_000,
    circuit: gradeCircuit,
    task: async () => ({ warnings: ["grade.wsl_inspection.timeout: WSL probe stopped"] }),
    classifyValue: (value) => classifyNonGatingFailure("grade", "status", value.warnings.join("\n")),
  });
  assert.equal(grade.ok, false);
  if (grade.ok) assert.fail("semantic Grade timeout unexpectedly succeeded");
  assert.equal(grade.code, "grade.wsl_inspection.timeout");
  assert.ok(gradeCircuit.openUntil > Date.now());

  const workCircuit = createProbeCircuit();
  let workCalls = 0;
  const workTask = async () => {
    workCalls += 1;
    return { ok: false, code: "work.network_quality.daemon_mount_io_busy" };
  };
  const firstWork = await runNonGatingProbe({
    lane: "work",
    key: "network_quality",
    timeoutMs: 100,
    cooldownMs: 1_000,
    circuit: workCircuit,
    task: workTask,
    classifyValue: (value) => classifyNonGatingFailure("work", "network_quality", value.code),
  });
  assert.equal(firstWork.ok, false);
  if (firstWork.ok) assert.fail("semantic Work timeout unexpectedly succeeded");
  assert.equal(firstWork.code, "work.network_quality.daemon_mount_io_busy");
  assert.equal(nonGatingFailureIsMuted(firstWork.code), true);

  const secondWork = await runNonGatingProbe({
    lane: "work",
    key: "network_quality",
    timeoutMs: 100,
    cooldownMs: 1_000,
    circuit: workCircuit,
    task: workTask,
  });
  assert.equal(secondWork.ok, false);
  if (secondWork.ok) assert.fail("semantic Work circuit unexpectedly ran the task");
  assert.equal(secondWork.code, "work.network_quality.circuit_open");
  assert.equal(workCalls, 1);
});
