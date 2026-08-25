import { useEffect, useRef } from "react";

import supportWechatQr from "./assets/csa-support-wechat.jpg";

interface HelpDialogProps {
  version: string;
  onClose: () => void;
}

export function HelpDialog({ version, onClose }: HelpDialogProps) {
  const dialogRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : undefined;
    const focusFrame = window.requestAnimationFrame(() => closeButtonRef.current?.focus());
    const keepFocusInDialog = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;

      const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
        'button:not([disabled]), summary, a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ) ?? []).filter((element) => element.offsetParent !== null);
      if (focusable.length === 0) {
        event.preventDefault();
        closeButtonRef.current?.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", keepFocusInDialog);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener("keydown", keepFocusInDialog);
      previouslyFocused?.focus();
    };
  }, [onClose]);

  return (
    <div
      className="help-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.currentTarget === event.target) onClose();
      }}
    >
      <section
        ref={dialogRef}
        className="help-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="help-dialog-title"
        aria-describedby="help-dialog-summary"
      >
        <div className="help-dialog-head">
          <div>
            <span className="eyebrow">CSA Help</span>
            <h2 id="help-dialog-title">使用帮助与常见问题</h2>
            <p id="help-dialog-summary">先看操作步骤和错误归因；仍无法解决时，再扫码联系人工帮助。</p>
          </div>
          <button ref={closeButtonRef} className="quiet-button" onClick={onClose}>关闭</button>
        </div>

        <div className="help-layout">
          <div className="help-faq">
            <details open>
              <summary>第一次怎么添加并启用 API Key？</summary>
              <ol>
                <li>等待顶部显示“Claude Science 已准备好/已就绪”。</li>
                <li>进入“接入模型 → 用一个 Key”，点击“＋ 新增接入”。</li>
                <li>选择供应商，粘贴 Key；“测试”会读取模型列表，并可能对多个候选模型以两档输出预算发送多次真实请求；可能产生费用，请勿反复点击。</li>
                <li>自动匹配或手动选择三个角色模型，给接入起名，再点“保存并启用”。</li>
                <li>保存只把它放进列表；在“我的接入”中预选并“确认切换”后，才会重启 Bridge 并发送一次 <code>max_tokens=1</code> 验证请求。</li>
              </ol>
            </details>

            <details>
              <summary>API Key 切换不了怎么办？</summary>
              <ol>
                <li>不要连续快速点击。先等当前操作结束，再“刷新状态”一次并展开“完整诊断与维护”。</li>
                <li>看到“上游不可达/代理失联”时检查“能力体检”；看到 HTTP 401/403 时先检查并修正 Key、额度和账号权限，修正后只重试一次，不要用同一错误 Key 反复测试。</li>
                <li>仍未恢复时，可先“修复并重启”；必要时“停止”后重新打开启动器。修复会备份配置、收窄 DrvFS 写授权并重启受管服务。</li>
                <li>只有手头确有三条可用接入时，才改用“三个 Key 分工”；聚合模式不是无效 Key 的绕过办法。</li>
              </ol>
            </details>

            <details>
              <summary>三个 Key 分工怎么用？</summary>
              <ol>
                <li>先分别添加并测试可用接入。</li>
                <li>切到“三个 Key 分工”，为决策、视觉、日常三个角色选择接入和模型。</li>
                <li>点击“保存并应用整套方案”。应用会重启 Bridge，并对三条路由各发一次 <code>max_tokens=1</code> 验证请求。</li>
              </ol>
            </details>

            <details>
              <summary>深度检测、能力体检和修复有什么区别？</summary>
              <ul>
                <li><strong>深度检测：</strong>检查沙盒 HTTP/SOCKS 实链路，不发送模型请求。</li>
                <li><strong>能力体检：</strong>检查 Bridge 到上游模型 API；确认后最多发送一次小请求。</li>
                <li><strong>修复并重启：</strong>执行有副作用的受管修复，不会关闭整个 WSL，也不会触碰 2222 等无关服务。</li>
                <li><strong>常用端口：</strong>Claude Science 为 8765/8766，Bridge 为 9876；端口监听不等于上游 API 一定可用。</li>
              </ul>
            </details>

            <details>
              <summary>这几个版本分别更新了什么？</summary>
              <dl className="help-version-list">
                <div><dt>v0.1.5</dt><dd>三模型聚合、多接入管理和统一安全启动入口。</dd></div>
                <div><dt>v0.1.6</dt><dd>加入端口、进程、Bridge 身份和沙盒出口深度检测，并把“能否打开”与“体检是否全绿”拆开。</dd></div>
                <div><dt>{version}</dt><dd>界面与可用性收口：亮色/经典/深色外观、接入命名、两步删除、帮助与发布前回归。</dd></div>
              </dl>
            </details>
          </div>

          <aside className="help-support-card" aria-label="人工帮助">
            <span className="eyebrow">Human Support</span>
            <h3>仍然解决不了？</h3>
            <p>扫码添加微信，可协助进入交流群。请备注“CSA”。</p>
            <img src={supportWechatQr} alt="微信二维码：Sheep_珐德" draggable={false} />
            <strong>联系前请准备</strong>
            <ul>
              <li>CSA 版本号</li>
              <li>已脱敏的诊断首行/错误码</li>
              <li>问题发生前的操作步骤</li>
            </ul>
            <p className="help-secret-warning">不要发送 API Key、Token、完整配置或未脱敏截图。</p>
          </aside>
        </div>
      </section>
    </div>
  );
}
