# CSA v0.1.3 — 启动器稳定性、磁盘防护与 Key 切换一致性修复

这是一个启动器稳定性修复版，重点解决“窗口能打开但显示又用不了 / 接入模型状态混乱 / 修复按钮执行后仍失败”的问题。

> 验证边界：本版本已在 Windows 11 10.0.22631 + Ubuntu-24.04 完成端到端验证；309/其他差异环境尚未完成本版本 DeepSeek thinking 全链路复测。

> 中转说明：`https://10521052.xyz/v1` 是 CSA 项目方自建中转服务，不是模型厂商官方 API；使用前仍需确认域名与服务条款。

## 这一版修复了什么

- 修复状态显示矛盾：Claude Science 已由端口检测到时，不再显示“已停止”。
- 修复启动脚本假成功：WSL 启动脚本失败时，Windows 包装层会返回真实失败，不再把失败当成功。
- 状态探测改为一次只读体检并设置总超时，WSL/VHDX 异常时窗口不再无限转圈。
- 启动器会显示 WSL 虚拟磁盘的 Windows 位置、宿主盘余量、Linux 根分区余量和 VHDX 大小。
- 增强 WSL 只读诊断：如果 `/tmp`、Linux 用户目录或根文件系统异常，停止自动重启，避免扩大损坏。
- 改进非 systemd Bridge fallback：进程会可靠脱离启动 shell，并验证连续健康，而不是短暂监听后消失。
- Bridge 健康检查会验证 `source_path`，旧解压目录的进程不能再冒充当前版本。
- API Key/模型切换采用“预写 Windows 设置 → 更新并重启 Bridge → 验证配置版本 → 原子提交”的事务流程；失败会回滚。
- 默认模型、Provider 面板预设和示例配置改为空状态；用户输入模型或获取实时模型列表后再映射。
- `/v1/models` 正式支持空列表，不再因零模型触发崩溃。
- Bridge 日志超过 50 MB 时仅轮转自身日志并保留一份备份，避免日志长期挤占 VHDX。
- 首页环境状态固定为 2 行 × 3 列的六项状态栏，并可折叠；折叠偏好不会影响诊断和后台刷新。
- 自定义中转可填写自己的名称；留空时按“自定义中转 + 当前日期 + 当日序号”生成，不再出现多条同名配置。
- API Key 连通测试不再用极小的固定输出长度；先以 256 tokens 测试，仅在 reasoning/长度截断时对同一模型重试到 1024，避免把有效模型误判失败并擅自切换。
- 正式对话不注入统一输出长度：Bridge 透传调用方的 `max_tokens`；调用方未传时让 OpenAI-compatible 上游采用自身默认，只有显式模型上限配置才会夹取。
- OpenAI o-series 自动使用 `max_completion_tokens`；并行工具偏好会转换为 `parallel_tool_calls`。
- Thinking/effort 采用“平台优先、模型家族其次”的显式请求适配；没有 caller 信号时不擅自开启，明确 400/422 参数错误最多兼容重试一次。
- 修复 DeepSeek/MiniMax 原生 Anthropic 兼容接口的推理参数：Claude Science 发出的 `thinking.type=auto` 会转换为上游支持的 `adaptive`，并移除与自适应模式冲突的固定预算；显式 `enabled` / `disabled` 保持不变。
- DeepSeek 在 `/models` 不提供可用列表时，可尝试经过官方资料核验的候选模型，但只有真实短对话成功后才会写入模型与映射，初始状态仍为空。
- 新增 `scripts/probe-provider-capabilities.ps1`，可在用户明确需要时检查已保存 Provider 的模型列表、短文本、输出参数、函数调用和 reasoning 控制；它会产生真实请求，不在日常启动中自动运行。

## 从 v0.1.2 / 更早版本升级

不需要卸载 WSL、Ubuntu 或旧版运行时。把 v0.1.3 完整 ZIP 解压到新目录，从新目录启动并让新版接管 Bridge；确认六项状态和模型连通测试正常后，再删除旧解压目录。不要只替换 exe，也不要把新包散文件覆盖进正在运行的旧目录。

同一台电脑、同一 Windows 用户下，CSA 会继续读取 `%APPDATA%\ClaudeScienceAssistant\settings.json`。旧版目录可暂时保留作为回退；若新版接管失败，停止新版后仍可从旧目录恢复。当前版本不执行静默自动更新或自动迁移 WSL 虚拟磁盘。

## 当前已知故障含义

如果诊断出现类似：

```text
WSL filesystem is read-only or not writable
Sandbox unavailable: cannot create the seccomp live-exec probe dir under /tmp
Read-only file system
Input/output error
```

这不是 API Key 或模型映射问题，而是 WSL 发行版进入了不可写状态。建议先执行：

```powershell
wsl --shutdown
```

然后重新打开 Ubuntu 和 CSA。若仍然只读，需要修复或重建该 WSL 发行版。

## 防止实验再次写满系统盘

启动器首页的“WSL 存储”会显示发行版实际位置。例如 `E:\WSL\Ubuntu-24.04` 表示 Linux 根文件系统主要占用 E 盘，而不是 C 盘。运行大实验前同时确认：

- WSL 虚拟磁盘所在宿主盘有足够空间；
- WSL Linux 根分区有足够空间和 inode；
- Windows 配置盘（通常为 C 盘）仍有余量；
- 大型数据集和实验输出写入专门的数据盘目录。

CSA 只提供诊断、告警和危险操作阻断，不会自动迁移、注销、压缩或重建用户的 WSL 发行版。
