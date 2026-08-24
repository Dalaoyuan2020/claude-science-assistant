export interface AllowStatus {
  wslInstalled: boolean;
  distro?: string;
  linuxUser?: string;
  runtimePresent: boolean;
  claudeRunning: boolean;
  claudePid?: number;
  listenerPresent: boolean;
  pid8765?: number;
  pid8766?: number;
  daemonState: string;
  listenerProbeOk: boolean;
  controlSocketPresent: boolean;
  windowsBridgePid?: number;
  windowsBridgeProbe: "checked" | "present" | "port_conflict" | "unknown";
  canOpen: boolean;
  canStart: boolean;
}

export const ALLOW_OPEN_INPUTS = ["claudeRunning", "windowsBridgePid"] as const satisfies readonly (keyof AllowStatus)[];
export const PRIMARY_LABEL_INPUTS = ["claudeRunning", "windowsBridgePid", "runtimePresent"] as const satisfies readonly (keyof AllowStatus)[];
type AllowOpenInput = (typeof ALLOW_OPEN_INPUTS)[number];
type PrimaryLabelInput = (typeof PRIMARY_LABEL_INPUTS)[number];

export const canOpenFromAllow = (allow: Pick<AllowStatus, AllowOpenInput>) => Boolean(
  allow.claudeRunning
  && !allow.windowsBridgePid
);

export const primaryLabelFromAllow = (allow: Pick<AllowStatus, PrimaryLabelInput>) => {
  if (allow.windowsBridgePid) return "先停止旧 Windows Bridge";
  if (canOpenFromAllow(allow)) return "打开 Claude Science";
  if (!allow.runtimePresent) return "安装运行环境";
  return "启动 Claude Science";
};

export type PrimaryAllowAction = "open" | "start" | "stop_legacy_bridge" | "install";

export interface PrimaryButtonView {
  action: PrimaryAllowAction;
  label: string;
  disabled: boolean;
}

export function primaryButtonView(
  allow: Pick<AllowStatus, PrimaryLabelInput>,
  allowLoaded: boolean,
  allowActionBusy: boolean,
): PrimaryButtonView {
  const action: PrimaryAllowAction = allow.windowsBridgePid
    ? "stop_legacy_bridge"
    : canOpenFromAllow(allow)
      ? "open"
      : !allow.runtimePresent
        ? "install"
        : "start";
  return {
    action,
    label: primaryLabelFromAllow(allow),
    disabled: !allowLoaded || allowActionBusy,
  };
}

export function nonGatingFailurePresentation(message: string, allow: Pick<AllowStatus, AllowOpenInput>): string {
  return canOpenFromAllow(allow)
    ? `${message}；仍可打开 Claude Science`
    : message;
}

export function nonGatingFailureIsMuted(
  code: string,
  timedOut = false,
  skipped = false,
): boolean {
  return timedOut
    || skipped
    || /(?:^|[._-])(timeout|not_checked|unknown|daemon_busy|daemon_mount_io_busy|circuit_open|superseded)$/.test(code);
}

export interface NonGatingProbeFailureClassification {
  code: string;
  message: string;
  timedOut: boolean;
}

export function classifyNonGatingFailure(
  lane: NonGatingLane,
  key: string,
  evidence: unknown,
): NonGatingProbeFailureClassification | undefined {
  const message = String(evidence);
  const explicitCodes = message.match(/\b(?:grade|work)\.[a-z0-9_.-]+/gi) || [];
  const explicitMutedCode = explicitCodes
    .map((code) => code.replace(/[.-]+$/, ""))
    .find((code) => code.startsWith(`${lane}.`) && nonGatingFailureIsMuted(code));
  if (explicitMutedCode) {
    return {
      code: explicitMutedCode,
      message,
      timedOut: /(?:^|[._-])timeout$/.test(explicitMutedCode),
    };
  }
  if (/\b(?:timed?\s*out|timeout)\b|超时|没有响应/i.test(message)) {
    return { code: `${lane}.${key}.timeout`, message, timedOut: true };
  }
  if (/daemon[_\s.-]*mount[_\s.-]*io[_\s.-]*busy|p9_client_rpc/i.test(message)) {
    return { code: `${lane}.${key}.daemon_mount_io_busy`, message, timedOut: false };
  }
  if (/daemon[_\s.-]*busy/i.test(message)) {
    return { code: `${lane}.${key}.daemon_busy`, message, timedOut: false };
  }
  return undefined;
}

export interface LaneState<Allow, Grade, Work> {
  allow: Allow;
  grade: Grade;
  work: Work;
}

export type LaneEvent<Allow, Grade, Work> =
  | { lane: "allow"; value: Allow }
  | { lane: "grade"; value: Grade }
  | { lane: "work"; value: Work };

export function createLaneState<Allow, Grade, Work>(allow: Allow, grade: Grade, work: Work): LaneState<Allow, Grade, Work> {
  return { allow, grade, work };
}

export function laneReducer<Allow, Grade, Work>(
  state: LaneState<Allow, Grade, Work>,
  event: LaneEvent<Allow, Grade, Work>,
): LaneState<Allow, Grade, Work> {
  switch (event.lane) {
    case "allow":
      return { ...state, allow: event.value };
    case "grade":
      return { ...state, grade: event.value };
    case "work":
      return { ...state, work: event.value };
  }
}

export type NonGatingLane = "grade" | "work";

export interface ProbeCircuit {
  generation: number;
  openUntil: number;
}

export const createProbeCircuit = (): ProbeCircuit => ({ generation: 0, openUntil: 0 });

export type NonGatingProbeResult<Value> =
  | {
    ok: true;
    code: string;
    value: Value;
    timedOut: false;
    skipped: false;
  }
  | {
    ok: false;
    code: string;
    message: string;
    timedOut: boolean;
    skipped: boolean;
  };

interface NonGatingProbeOptions<Value> {
  lane: NonGatingLane;
  key: string;
  timeoutMs: number;
  circuit: ProbeCircuit;
  task: () => Promise<Value>;
  classifyValue?: (value: Value) => NonGatingProbeFailureClassification | undefined;
  cooldownMs?: number;
  now?: () => number;
}

export async function runNonGatingProbe<Value>({
  lane,
  key,
  timeoutMs,
  circuit,
  task,
  classifyValue,
  cooldownMs = Math.max(timeoutMs, 30_000),
  now = Date.now,
}: NonGatingProbeOptions<Value>): Promise<NonGatingProbeResult<Value>> {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new Error(`${lane}.${key}.invalid_timeout`);
  }
  if (circuit.openUntil > now()) {
    return {
      ok: false,
      code: `${lane}.${key}.circuit_open`,
      message: "probe circuit is cooling down",
      timedOut: false,
      skipped: true,
    };
  }

  const generation = ++circuit.generation;
  return new Promise<NonGatingProbeResult<Value>>((resolve) => {
    let settled = false;
    const finish = (
      result: NonGatingProbeResult<Value>,
      commitCircuitState?: () => void,
    ) => {
      if (settled) return;
      settled = true;
      globalThis.clearTimeout(timeoutHandle);
      if (generation === circuit.generation) {
        commitCircuitState?.();
        resolve(result);
        return;
      }
      resolve({
        ok: false,
        code: `${lane}.${key}.superseded`,
        message: "probe result was superseded by a newer invocation",
        timedOut: false,
        skipped: true,
      });
    };
    const openCircuit = () => {
      circuit.openUntil = now() + cooldownMs;
    };
    const timeoutHandle = globalThis.setTimeout(() => {
      finish({
        ok: false,
        code: `${lane}.${key}.timeout`,
        message: `probe exceeded ${timeoutMs}ms hard deadline`,
        timedOut: true,
        skipped: false,
      }, openCircuit);
    }, timeoutMs);

    Promise.resolve()
      .then(task)
      .then((value) => {
        const semanticFailure = classifyValue?.(value);
        if (semanticFailure) {
          finish({
            ok: false,
            code: semanticFailure.code,
            message: semanticFailure.message,
            timedOut: semanticFailure.timedOut,
            skipped: false,
          }, openCircuit);
          return;
        }
        finish({
          ok: true,
          code: `${lane}.${key}.ok`,
          value,
          timedOut: false,
          skipped: false,
        }, () => {
          circuit.openUntil = 0;
        });
      })
      .catch((reason: unknown) => {
        const message = String(reason);
        const classified = classifyNonGatingFailure(lane, key, message);
        finish({
          ok: false,
          code: classified?.code || `${lane}.${key}.failed`,
          message,
          timedOut: classified?.timedOut || false,
          skipped: false,
        }, openCircuit);
      });
  });
}
