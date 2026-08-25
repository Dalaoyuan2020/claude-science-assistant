import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { deleteConfirmationText, screenMessage } from "../src/uiPresentation.ts";

const source = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");

function sourceSlice(startMarker: string, endMarker: string) {
  const start = source.indexOf(startMarker);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.ok(start >= 0, `missing ${startMarker}`);
  assert.ok(end > start, `missing ${endMarker} after ${startMarker}`);
  return source.slice(start, end);
}

test("skin_choice_persists in LauncherSettings and cannot blank the UI", () => {
  assert.match(source, /invoke<UiPreferences>\("get_ui_preferences"\)/);
  assert.match(source, /invoke<UiPreferences>\("save_ui_skin", \{ uiSkin: nextSkin \}\)/);
  assert.match(source, /skinPreferenceResolved/);
  assert.match(source, /inert=\{!skinPreferenceResolved \|\| showSkinChooser\}/);
  assert.match(backend, /struct LauncherSettings[\s\S]*ui_skin: Option<String>/);
  assert.match(backend, /Err\(error\) if error\.kind\(\) == std::io::ErrorKind::NotFound => UiPreferences \{ skin: None \}/);
  assert.match(backend, /Err\(_\) => UiPreferences \{[\s\S]*Some\("console"\.into\(\)\)/);

  const chooserPath = sourceSlice("async function chooseSkin", "function updateBusy");
  assert.equal(chooserPath.includes("localStorage"), false, "skin preference must not use WebView storage");
  assert.match(chooserPath, /busyRef\.current \|\| allowActionBusyRef\.current \|\| repairBusyRef\.current/);
  assert.match(source, /className="appearance-button" disabled=\{appearanceBlocked\}/);
});

test("both_skins_keep_all_entries in one DOM tree", () => {
  assert.equal(source.match(/<main className="app-shell" data-skin=\{skin\}>/g)?.length, 1);
  assert.equal(source.match(/className="control-deck"/g)?.length, 1);
  assert.equal(source.match(/className=\{`kit-section/g)?.length, 1);
  assert.match(source, /document\.documentElement\.dataset\.skin = skin/);
  assert.match(styles, /\.app-shell\[data-skin="console"\]/);
  assert.match(styles, /\.screen-readout-head,[\s\S]*\.screen-readout \{ display: none; \}/);

  for (const capability of [
    "刷新状态",
    "深度检测",
    "能力体检",
    "修复并重启",
    "完整诊断与维护",
    "检查更新",
    "配置面板",
    "新增接入",
    "自动匹配",
    "重新获取模型",
    "保存并应用整套方案",
    "重命名",
    "确认删除",
  ]) {
    assert.ok(source.includes(capability), `missing shared capability: ${capability}`);
  }
});

test("delete_requires_two_steps", () => {
  const firstClick = sourceSlice("function beginDeleteKey", "function cancelDeleteKey");
  const confirmation = sourceSlice("async function confirmDeleteKey", "async function primaryAction");
  assert.equal(firstClick.includes("deleteKey("), false, "first delete click must only select a confirmation row");
  assert.match(confirmation, /await deleteKey\(apiKeyId\)/);
  assert.equal(source.includes("window.confirm"), false);
  assert.equal(source.match(/useState<string>\(\).*deleteConfirmApiKeyId/g)?.length ?? 0, 0);
  assert.match(source, /const \[deleteConfirmApiKeyId, setDeleteConfirmApiKeyId\] = useState<string>\(\)/);
  assert.match(source, /deleteConfirmationText\(entry\.label\)/);
  assert.match(source, /ref=\{deleteCancelRef\}[\s\S]*>取消<\/button>/);
  assert.match(source, /deleteButtonRefs\.current\.get\(apiKeyId\)\?\.focus\(\)/);
});

test("delete_confirm_names_target", () => {
  assert.equal(
    deleteConfirmationText("备用 · OpenRouter"),
    "确定删除「备用 · OpenRouter」？删除后无法恢复。",
  );
});

test("custom_label_accepted_for_any_provider and rename share validation", () => {
  assert.match(source, /displayName: draftDisplayName/);
  assert.match(source, /<strong>给它起个名字<\/strong>/);
  assert.match(source, /maxLength=\{80\}/);
  assert.match(source, /invoke<LauncherSettings>\("rename_api_key"/);
  assert.match(backend, /fn label_for_provider\(/);
  assert.match(backend, /fn rename_api_key_in_settings\([\s\S]*validate_display_name\(display_name\)/);
});

test("screen_never_gates_main_button and retains a bounded diagnostic copy", () => {
  assert.match(source, /const SCREEN_LINE_INTERVAL_MS = 3_000/);
  assert.match(source, /const SCREEN_LINE_LIMIT = 40/);
  assert.match(source, /\.slice\(-SCREEN_LINE_LIMIT\)/);
  assert.match(source, /aria-live="polite"/);
  assert.match(source, /setScreenPaused\(\(current\) => !current\)/);
  assert.match(source, /if \(skin !== "console" \|\| !screenReady/);
  assert.match(source, /\{screenReady && <>[\s\S]*className="screen-readout"/);
  assert.match(source, /screenMessage\(probeNotice\.message\)/);
  assert.match(source, /rows\.push\(\{ key, value, tone: "fault" \}\);[\s\S]*仍可打开 Claude Science/);
  assert.match(source, /<details className="diagnostics-drawer"/);

  const screenEffects = sourceSlice("const screenReady = canOpenClaude", "return (");
  assert.equal(screenEffects.includes("requestAnimationFrame"), false, "screen animation must use timers, not a resident frame loop");
  for (const forbidden of [
    "setAllowStatus",
    "commitAllowStatus",
    "updateAllowActionBusy",
    "setAllowActionBusy",
  ]) {
    assert.equal(
      screenEffects.includes(forbidden),
      false,
      `presentation-only screen logic must not call ${forbidden}`,
    );
  }
  const primaryMarkup = sourceSlice('<div className="primary-control">', '<div className="function-keys">');
  assert.match(primaryMarkup, /disabled=\{primaryButton\.disabled\}/);
  assert.match(primaryMarkup, /\{primaryButton\.label\}/);
  assert.equal(screenMessage("login.open_failed: 无法打开登录页"), "无法打开登录页");
  assert.equal(screenMessage("transport.stop_services_failed: 停止失败"), "停止失败");
  assert.equal(screenMessage("普通用户提示"), "普通用户提示");
});

test("function_keys_have_independent_busy_state", () => {
  assert.match(source, /onClick=\{runNetworkQualityCheck\} disabled=\{networkChecking \|\| !allowStatus\.claudeRunning\}/);
  assert.match(source, /onClick=\{openBridgeEgressAssistant\} disabled=\{bridgeEgressChecking\}/);
  assert.match(source, /onClick=\{\(\) => runAction\("restart_services"\)\}[\s\S]*disabled=\{repairBusy \|\| !allowStatus\.wslInstalled \|\| status\.restartBlocked\}/);

  const repair = sourceSlice("async function runRepairAction", "async function applyDraftKey");
  assert.equal(repair.includes("updateAllowActionBusy"), false);
  assert.equal(repair.includes("setAllowActionBusy"), false);
  const network = sourceSlice("async function runNetworkQualityCheck", "async function runAction");
  assert.equal(network.includes("updateAllowActionBusy"), false);
});

test("console visual contract uses the bright adaptive tokens and reduced motion", () => {
  for (const token of [
    "--chassis: #f1f6f3",
    "--screen: #fbfefc",
    "--phosphor: #176b49",
    "--phosphor-dim: #668f7c",
    "--amber: #a86508",
    "--rust: #c8463b",
    "--ink: #203029",
    "--ink-dim: #607168",
    "--ink-faint: #91a098",
  ]) assert.ok(styles.includes(token), `missing token ${token}`);
  assert.match(styles, /width: clamp\(640px, 88vw, 1040px\)/);
  assert.match(styles, /\.app-shell\[data-skin="classic"\] \.control-deck \{[\s\S]*grid-template-columns: repeat\(4, minmax\(0, 1fr\)\)/);
  assert.match(styles, /\.app-shell\[data-skin="classic"\] \.primary-button,[\s\S]*min-height: 68px/);
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/);
  assert.match(styles, /"IBM Plex Mono", ui-monospace, Consolas, monospace/);
  assert.match(styles, /"IBM Plex Sans", system-ui, "Microsoft YaHei", sans-serif/);
});
