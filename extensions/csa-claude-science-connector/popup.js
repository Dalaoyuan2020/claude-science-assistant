const statusBadge = document.getElementById("statusBadge");
const statusText = document.getElementById("statusText");
const pairForm = document.getElementById("pairForm");
const pairCode = document.getElementById("pairCode");
const disconnectButton = document.getElementById("disconnectButton");

function setStatus(label, detail, kind = "") {
  statusBadge.textContent = label;
  statusBadge.className = kind;
  statusText.textContent = detail;
}

async function refresh() {
  const state = await chrome.runtime.sendMessage({ type: "popupState" });
  if (!state.paired) {
    pairForm.hidden = false;
    setStatus("未配对", "请先在 CSA 桌面端生成插件配对码。");
    disconnectButton.disabled = true;
    return;
  }
  pairForm.hidden = true;
  disconnectButton.disabled = false;
  if (state.status === "pageReady") {
    setStatus("页面就绪", "已连接 Claude Science 页面。", "ready");
  } else if (state.status === "online") {
    setStatus("在线", "插件在线，但当前页面输入框不可用。");
  } else if (state.status === "paired") {
    setStatus("已配对", "请打开本机 Claude Science 页面。");
  } else {
    setStatus("离线", state.lastError || "无法连接 CSA 桌面端。", "error");
  }
}

pairForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const code = pairCode.value.trim();
  if (!code) {
    setStatus("缺少配对码", "请填写 CSA 桌面端生成的配对码。", "error");
    return;
  }
  try {
    const result = await chrome.runtime.sendMessage({ type: "pair", code });
    if (!result?.ok) throw new Error(result?.error || "配对失败");
    pairCode.value = "";
    await refresh();
  } catch (error) {
    setStatus("配对失败", String(error.message || error), "error");
  }
});

disconnectButton.addEventListener("click", async () => {
  const result = await chrome.runtime.sendMessage({ type: "disconnect" });
  if (!result?.ok && result?.error) {
    setStatus("已本地断开", result.error, "error");
    return;
  }
  await refresh();
});

refresh().catch((error) => setStatus("读取失败", String(error.message || error), "error"));
