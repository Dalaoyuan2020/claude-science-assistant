let busy = false;
const TASK_LEDGER_KEY = "csaTaskLedgerV1";
const TASK_LEDGER_LIMIT = 200;

async function loadTaskLedger() {
  const stored = await chrome.storage.local.get([TASK_LEDGER_KEY]);
  return stored[TASK_LEDGER_KEY] && typeof stored[TASK_LEDGER_KEY] === "object"
    ? stored[TASK_LEDGER_KEY]
    : {};
}

async function setTaskState(taskId, status) {
  const ledger = await loadTaskLedger();
  ledger[taskId] = { status, updatedAt: Date.now() };
  const entries = Object.entries(ledger)
    .sort((left, right) => right[1].updatedAt - left[1].updatedAt)
    .slice(0, TASK_LEDGER_LIMIT);
  await chrome.storage.local.set({ [TASK_LEDGER_KEY]: Object.fromEntries(entries) });
}

async function clearTaskState(taskId) {
  const ledger = await loadTaskLedger();
  delete ledger[taskId];
  await chrome.storage.local.set({ [TASK_LEDGER_KEY]: ledger });
}

function markerAppearsOutsideComposer(marker) {
  return countOccurrencesOutsideComposer(marker) > 0;
}

function countOccurrences(text, needle) {
  if (!text || !needle) return 0;
  let count = 0;
  let offset = 0;
  while (true) {
    const index = text.indexOf(needle, offset);
    if (index < 0) return count;
    count += 1;
    offset = index + needle.length;
  }
}

function countOccurrencesOutsideComposer(needle) {
  if (!needle) return 0;
  const bodyText = document.body?.innerText || "";
  const composerText = composerValue(findComposer());
  return Math.max(0, countOccurrences(bodyText, needle) - countOccurrences(composerText, needle));
}

function textSignal(element) {
  return [
    element.getAttribute("aria-label"),
    element.getAttribute("placeholder"),
    element.getAttribute("data-placeholder"),
    element.getAttribute("title"),
    "value" in element ? element.value : "",
    element.textContent,
  ].filter(Boolean).join(" ");
}

function composerValue(element) {
  if (!element) return "";
  if (element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement) {
    return element.value || "";
  }
  return element.innerText || element.textContent || "";
}

function isVisible(element) {
  const rect = element.getBoundingClientRect();
  const style = window.getComputedStyle(element);
  return rect.width > 40 && rect.height > 20 && style.visibility !== "hidden" && style.display !== "none";
}

function scoreComposer(element) {
  if (!isVisible(element)) return -1;
  let score = 0;
  const signal = textSignal(element).toLowerCase();
  const role = (element.getAttribute("role") || "").toLowerCase();
  if (signal.includes("ask anything")) score += 8;
  if (signal.includes("artifacts") || signal.includes("sessions") || signal.includes("skills")) score += 3;
  if (role === "textbox") score += 2;
  if (element.matches("textarea,input,[contenteditable='true'],[contenteditable='plaintext-only']")) score += 3;
  const parentText = element.closest("form,section,div")?.textContent?.toLowerCase() || "";
  if (parentText.includes("notebook")) score += 2;
  if (parentText.includes("opus") || parentText.includes("claude")) score += 1;
  return score;
}

function findComposer() {
  const selectors = [
    "textarea",
    "input[type='text']",
    "[contenteditable='true']",
    "[contenteditable='plaintext-only']",
    "[role='textbox']",
    ".ProseMirror",
  ];
  const candidates = [...document.querySelectorAll(selectors.join(","))];
  return candidates
    .map((element) => ({ element, score: scoreComposer(element) }))
    .filter((item) => item.score >= 4)
    .sort((left, right) => right.score - left.score)[0]?.element || null;
}

function setNativeValue(element, value) {
  const prototype = element instanceof HTMLTextAreaElement
    ? HTMLTextAreaElement.prototype
    : element instanceof HTMLInputElement
      ? HTMLInputElement.prototype
      : null;
  if (prototype) {
    const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
    if (setter) setter.call(element, value);
    else element.value = value;
    element.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: value }));
    element.dispatchEvent(new Event("change", { bubbles: true }));
    return;
  }
  element.focus();
  const selection = window.getSelection();
  const range = document.createRange();
  range.selectNodeContents(element);
  selection.removeAllRanges();
  selection.addRange(range);
  const inputType = value ? "insertText" : "deleteContentBackward";
  element.dispatchEvent(new InputEvent("beforeinput", {
    bubbles: true,
    cancelable: true,
    inputType,
    data: value,
  }));
  const inserted = value
    ? document.execCommand("insertText", false, value)
    : document.execCommand("delete", false);
  const valueMatches = value
    ? composerValue(element).includes(value)
    : composerValue(element).trim() === "";
  if (!inserted || !valueMatches) {
    element.textContent = value;
  }
  element.dispatchEvent(new InputEvent("input", { bubbles: true, inputType, data: value }));
  element.dispatchEvent(new Event("change", { bubbles: true }));
}

function findSendButton() {
  const buttons = [...document.querySelectorAll("button")].filter(isVisible);
  const matches = buttons.map((button) => {
    const label = [
      button.getAttribute("aria-label"),
      button.getAttribute("title"),
      button.textContent,
    ].filter(Boolean).join(" ").toLowerCase();
    return { button, label: label.trim() };
  }).filter(({ button, label }) => (
    !button.disabled
    && !label.includes("more send")
    && (label === "send" || label.includes("send message") || label.includes("发送"))
  ));
  return matches[0]?.button || null;
}

function findAttachmentButton() {
  return [...document.querySelectorAll("button")].filter(isVisible).find((button) => {
    const label = [
      button.getAttribute("aria-label"),
      button.getAttribute("title"),
      button.textContent,
    ].filter(Boolean).join(" ").toLowerCase();
    return label.includes("attach") || label.includes("upload") || label.includes("file")
      || label.includes("附件") || label.includes("上传") || label.includes("添加文件");
  }) || null;
}

async function findFileInput(timeoutMs = 1600) {
  const deadline = Date.now() + timeoutMs;
  let clicked = false;
  while (Date.now() < deadline) {
    const input = document.querySelector("input[type='file']");
    if (input) return input;
    if (!clicked) {
      const button = findAttachmentButton();
      if (button) {
        button.click();
        clicked = true;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return null;
}

async function sha256Hex(blob) {
  const digest = await crypto.subtle.digest("SHA-256", await blob.arrayBuffer());
  return [...new Uint8Array(digest)].map((value) => value.toString(16).padStart(2, "0")).join("");
}

async function attachTaskFiles(task) {
  const attachments = Array.isArray(task.attachments) ? task.attachments : [];
  if (attachments.length === 0) return { ok: true };
  const input = await findFileInput();
  if (!input) return { ok: false, reason: "Claude Science file input was not found" };
  const beforeImages = document.querySelectorAll("img").length;
  const files = [];
  for (const attachment of attachments) {
    const response = await fetch(attachment.downloadUrl, { method: "GET", cache: "no-store" });
    if (!response.ok) return { ok: false, reason: "Local attachment capability was unavailable" };
    const blob = await response.blob();
    if (blob.size !== attachment.sizeBytes || blob.size <= 0 || blob.size > 20 * 1024 * 1024) {
      return { ok: false, reason: "Attachment size verification failed" };
    }
    if (attachment.sha256 && await sha256Hex(blob) !== attachment.sha256.toLowerCase()) {
      return { ok: false, reason: "Attachment digest verification failed" };
    }
    files.push(new File([blob], attachment.fileName, {
      type: attachment.mimeType,
      lastModified: Date.now(),
    }));
  }
  const transfer = new DataTransfer();
  for (const file of files) transfer.items.add(file);
  input.files = transfer.files;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
  const deadline = Date.now() + 2400;
  while (Date.now() < deadline) {
    const bodyText = document.body?.innerText || "";
    const fileNamesVisible = files.every((file) => bodyText.includes(file.name));
    const imagePreviewAdded = document.querySelectorAll("img").length > beforeImages;
    if (fileNamesVisible || imagePreviewAdded) return { ok: true };
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  const emptyTransfer = new DataTransfer();
  input.files = emptyTransfer.files;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.dispatchEvent(new Event("change", { bubbles: true }));
  return { ok: false, reason: "Claude Science did not confirm the attachment preview" };
}

async function waitForComposerReadyToSend(composer, expectedText, timeoutMs = 2600) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const currentValue = composerValue(composer);
    const button = findSendButton();
    if (currentValue.includes(expectedText) && button) return button;
    await new Promise((resolve) => setTimeout(resolve, 80));
  }
  return null;
}

async function waitForSubmissionConfirmation(needle, initialCount, timeoutMs = 5200) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const composer = findComposer();
    const composerStillHasText = Boolean(composer && composerValue(composer).trim());
    const submittedCount = countOccurrencesOutsideComposer(needle);
    if (!composerStillHasText && submittedCount > initialCount) return true;
    await new Promise((resolve) => setTimeout(resolve, 120));
  }
  return false;
}

async function injectMessage(task) {
  const ledger = await loadTaskLedger();
  const previous = ledger[task.taskId]?.status || "";
  if (previous === "submitted" || markerAppearsOutsideComposer(task.marker)) {
    await setTaskState(task.taskId, "submitted");
    return { status: "submitted", reason: "Task was already submitted" };
  }
  if (previous === "submitting") {
    return {
      status: "deliveryUnknown",
      reason: "A previous submit attempt was interrupted; automatic resubmission is disabled",
    };
  }
  const composer = findComposer();
  if (!composer) {
    return { status: "pageUnavailable", reason: "Composer not found" };
  }
  const confirmationNeedle = String(task.marker || "").trim() || task.text;
  const initialConfirmationCount = countOccurrencesOutsideComposer(confirmationNeedle);
  try {
    const attached = await attachTaskFiles(task);
    if (!attached.ok) {
      await clearTaskState(task.taskId);
      return { status: "failed", reason: attached.reason };
    }
  } catch (_error) {
    await clearTaskState(task.taskId);
    return { status: "failed", reason: "Attachment preparation failed" };
  }
  composer.focus();
  setNativeValue(composer, task.text);
  const button = await waitForComposerReadyToSend(composer, task.text);
  if (!button) {
    setNativeValue(composer, "");
    await clearTaskState(task.taskId);
    return {
      status: "failed",
      reason: "Claude Science did not expose an enabled Send button after the composer accepted the text",
    };
  }
  await setTaskState(task.taskId, "submitting");
  button.click();
  if (!(await waitForSubmissionConfirmation(
    confirmationNeedle,
    initialConfirmationCount,
  ))) {
    return {
      status: "deliveryUnknown",
      reason: "Send was triggered but the submitted user message could not be confirmed",
    };
  }
  await setTaskState(task.taskId, "submitted");
  return { status: "submitted", reason: "" };
}

function collectHeartbeat() {
  const composer = findComposer();
  const bodyText = document.body?.innerText || "";
  const frameMatch = bodyText.match(/Frame ID\s*(?:（本会话）)?\s*([a-z0-9-]{12,})/i);
  const projectMatch = bodyText.match(/Project ID\s*(proj_[a-z0-9_-]+)/i);
  return {
    schemaVersion: 4,
    extensionId: chrome.runtime.id,
    tabId: 0,
    url: location.href,
    pageTitle: document.title,
    composerReady: Boolean(composer),
    frameId: frameMatch?.[1] || "",
    projectId: projectMatch?.[1] || "",
    lastSeenAt: Date.now(),
  };
}

async function tick() {
  if (busy) return;
  busy = true;
  try {
    const heartbeat = collectHeartbeat();
    const tabs = await chrome.runtime.sendMessage({ type: "heartbeat", heartbeat });
    if (tabs?.ok === false && tabs.error === "notPaired") return;
    const taskResponse = await chrome.runtime.sendMessage({ type: "pollTask" });
    const task = taskResponse?.task;
    if (!task || task.kind !== "sendMessage") return;
    const result = await injectMessage(task);
    await chrome.runtime.sendMessage({
      type: "taskResult",
      result: {
        schemaVersion: 1,
        taskId: task.taskId,
        status: result.status,
        reason: result.reason,
        submittedAt: Date.now(),
      },
    });
  } catch (_error) {
    // Silent by design: the desktop status and fallback path handle failures.
  } finally {
    busy = false;
  }
}

setInterval(tick, 1000);
tick();
