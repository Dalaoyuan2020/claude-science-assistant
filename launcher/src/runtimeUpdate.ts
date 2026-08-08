export interface RuntimeReleaseSummary {
  version: string;
  sha8: string;
  buildDate: string;
}

export interface RuntimeUpdateStatus {
  bundledVersion: string;
  bundledSha8: string;
  recommendedVersion: string;
  latest: RuntimeReleaseSummary;
  stable: RuntimeReleaseSummary;
  updateAvailable: boolean;
  checkedAtUnix: number;
  releaseNotesUrl: string;
  note: string;
}

const safeValue = (value: string) => String(value || "").replace(/[\r\n\u0000-\u001f]+/g, " ").trim();

export const browserPreviewRuntimeStatus: RuntimeUpdateStatus = {
  bundledVersion: "0.1.25",
  bundledSha8: "b7190511",
  recommendedVersion: "0.1.25",
  latest: { version: "0.1.27", sha8: "a8cf9eae", buildDate: "2026-08-07" },
  stable: { version: "0.1.27", sha8: "a8cf9eae", buildDate: "2026-08-07" },
  updateAvailable: true,
  checkedAtUnix: 0,
  releaseNotesUrl: "https://claude.com/docs/claude-science/changelog",
  note: "浏览器预览数据；真实版本请在桌面启动器中检查。",
};

export function buildRuntimeUpgradePrompt(status: RuntimeUpdateStatus): string {
  const target = safeValue(status.latest.version);
  const targetSha8 = safeValue(status.latest.sha8);
  return [
    "请作为本地 Codex 协助我评估 CSA（Claude Science Assistant）的 Claude Science runtime 升级。当前打开目录应是 CSA 完整包或源码目录。",
    "",
    "目标与已知基线：",
    `- 当前 CSA 已验证并内置：Claude Science ${safeValue(status.bundledVersion)}（sha8 ${safeValue(status.bundledSha8)}）`,
    `- 官方 latest：${target}（sha8 ${targetSha8}）`,
    `- 官方 stable：${safeValue(status.stable.version)}（sha8 ${safeValue(status.stable.sha8)}）`,
    `- 官方更新日志：${safeValue(status.releaseNotesUrl)}`,
    "",
    "必须遵守：",
    "1. 先阅读当前目录 AGENTS.md、README.md、vendor/claude-science/linux-x64/manifest.json 和 docs/v0.1.4-runtime-update.zh-CN.md；文件缺失就报告，不从网络下载同名脚本替代。",
    "2. 只从 Claude Science 官方发布索引取得版本指针、manifest 和 Linux x64 二进制；校验指针、manifest.sha8 与 SHA-256 三者一致。",
    "3. 不输出或复制 API Key、token、Cookie、私钥、DPAPI 明文、.env 或 Bridge 凭据。",
    "4. 不修改 VPN、代理、DNS、hosts、证书、端口 443 或任何网络工具。",
    "5. 不覆盖当前不可变的 0.1.25 vendor 基线，不删除现有数据，不执行 wsl --unregister。",
    "6. 不直接在真实 ~/.claude-science 数据上试跑新版本。先用 SQLite backup API 备份，再复制到隔离数据目录，并使用备用端口验证。",
    "7. 验证至少包含：二进制哈希、可执行性、数据库 quick_check、schema 变化、备用实例健康、CSA URL 补丁兼容、Bridge /health、启动器自测。",
    "8. 如果当前包没有明确的 runtime selector/双槽切换机制，不得临时改 vendor 后直接启动；应输出需要修改和重新打包的文件清单。",
    "9. 同一错误修复两次仍失败、数据格式不可逆、哈希不匹配或需要破坏性操作时立即停止。",
    "",
    "按以下阶段执行：",
    "A. 只读检查当前 CSA 版本、Git 状态、运行进程、真实 runtime 路径、数据目录和可用磁盘空间。",
    `B. 重新读取官方索引，确认 ${target}/${targetSha8} 仍是目标；如已变化，报告差异，不擅自追更。`,
    "C. 下载到隔离 staging，验证官方 manifest 与 Linux x64 SHA-256。",
    "D. 创建一致性数据库备份和隔离副本；在备用端口启动候选，完成兼容测试，不停止真实服务。",
    "E. 输出升级前后证据、已知风险、回退点、需改文件和最终切换命令。",
    "F. 到此停止，等待我明确批准切换或制作新 CSA 包；未经批准不得替换真实运行时。",
    "",
    "最终报告必须明确区分：官方 latest、官方 stable、CSA 已验证推荐版。不能把 latest 自动宣称为 CSA 兼容版。",
  ].join("\n");
}

export function buildRuntimeRollbackPrompt(status: RuntimeUpdateStatus): string {
  return [
    "请作为本地 Codex 协助我把 CSA 的 Claude Science runtime 安全回退到当前安装包内置的已验证基线。",
    "",
    `目标基线：Claude Science ${safeValue(status.bundledVersion)}（sha8 ${safeValue(status.bundledSha8)}）。`,
    "0.1.21 仅作为历史重要节点记录；在缺少当前包内官方 manifest、Linux x64 SHA-256 和隔离兼容证据时，不得执行回退到 0.1.21。",
    "",
    "硬性要求：",
    "1. 先阅读 AGENTS.md、README.md、vendor/claude-science/linux-x64/manifest.json 和 docs/v0.1.4-runtime-update.zh-CN.md。",
    "2. 只读确认当前二进制、真实数据目录、数据库 schema、运行进程和可用备份；不要先停止服务。",
    "3. 校验包内 vendor 二进制 SHA-256 与 manifest 完全一致，任何不一致都停止。",
    "4. 二进制回退不能替代数据库回退。若新版已迁移 schema，必须使用升级前 SQLite backup API 快照；没有兼容快照就先报告，不强行启动旧版。",
    "5. 不输出任何密钥，不修改网络设置，不删除数据，不执行 wsl --unregister，不覆盖唯一备份。",
    "6. 先在隔离数据副本与备用端口验证 0.1.25，检查 quick_check、健康页、Bridge 和启动器兼容。",
    "7. 输出停机窗口、原子切换步骤、验收项和可再次恢复到当前版本的路径，然后停止等待批准。",
    "",
    "验收至少包括：Claude Science 8765、Bridge 9876 /health、source_path、API Key 配置未泄露、旧项目/会话可见、启动器 self-test.ps1 与 verify-proxy.ps1 全绿。",
  ].join("\n");
}
