const CSA_API = "http://127.0.0.1:9882/api/browser-extension";

async function getToken() {
  const data = await chrome.storage.local.get(["csaToken"]);
  return data.csaToken || "";
}

async function setToken(token) {
  await chrome.storage.local.set({ csaToken: token });
}

async function clearToken() {
  await chrome.storage.local.remove(["csaToken"]);
}

async function requestJson(path, options = {}) {
  const headers = { "Content-Type": "application/json", ...(options.headers || {}) };
  const token = await getToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  const response = await fetch(`${CSA_API}${path}`, { ...options, headers });
  const text = await response.text();
  const data = text ? JSON.parse(text) : {};
  if (response.status === 401) await clearToken();
  if (!response.ok || data.ok === false) {
    throw new Error(data.error || `CSA request failed: ${response.status}`);
  }
  return data;
}

async function pairWithDesktop(code) {
  const data = await fetch(`${CSA_API}/pair`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code, extensionId: chrome.runtime.id }),
  }).then(async (response) => {
    const text = await response.text();
    const body = text ? JSON.parse(text) : {};
    if (!response.ok || body.ok === false) throw new Error(body.error || "Pairing failed");
    return body;
  });
  await setToken(data.token);
  return data;
}

async function tryAutoPair() {
  if (await getToken()) return true;
  try {
    const response = await fetch(`${CSA_API}/pairing-offer`, { method: "GET", cache: "no-store" });
    const offer = await response.json();
    if (!response.ok || !offer?.available || !offer?.code) return false;
    await pairWithDesktop(String(offer.code));
    return true;
  } catch (_error) {
    return false;
  }
}

async function popupState() {
  const token = await getToken();
  if (!token) return { paired: false, status: "notPaired" };
  try {
    const state = await requestJson("/status", { method: "GET" });
    return { paired: true, ...state };
  } catch (error) {
    return { paired: true, status: "offline", lastError: String(error.message || error) };
  }
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  (async () => {
    if (message?.type === "pair") {
      sendResponse(await pairWithDesktop(String(message.code || "").trim()));
      return;
    }
    if (message?.type === "disconnect") {
      let result = { ok: true };
      try {
        result = await requestJson("/disconnect", { method: "POST", body: "{}" });
      } catch (error) {
        result = { ok: false, error: String(error.message || error) };
      } finally {
        await clearToken();
      }
      sendResponse(result);
      return;
    }
    if (message?.type === "popupState") {
      await tryAutoPair();
      sendResponse(await popupState());
      return;
    }
    if (message?.type === "heartbeat") {
      if (!(await getToken()) && !(await tryAutoPair())) {
        sendResponse({ ok: false, error: "notPaired" });
        return;
      }
      const heartbeat = { ...(message.heartbeat || {}) };
      heartbeat.tabId = sender?.tab?.id || heartbeat.tabId || 0;
      sendResponse(await requestJson("/heartbeat", {
        method: "POST",
        body: JSON.stringify(heartbeat),
      }));
      return;
    }
    if (message?.type === "pollTask") {
      if (!(await getToken())) {
        sendResponse({ ok: true, task: null });
        return;
      }
      sendResponse(await requestJson("/tasks", { method: "GET" }));
      return;
    }
    if (message?.type === "taskResult") {
      const result = message.result || {};
      const taskId = encodeURIComponent(result.taskId || "");
      if (!taskId) throw new Error("Missing task id");
      sendResponse(await requestJson(`/tasks/${taskId}/result`, {
        method: "POST",
        body: JSON.stringify(result),
      }));
      return;
    }
    sendResponse({ ok: false, error: "unknown message" });
  })().catch((error) => sendResponse({ ok: false, error: String(error.message || error) }));
  return true;
});
