# CSA v0.1.3 — 启动器稳定性、磁盘防护与 Key 切换一致性修复

这是一个启动器稳定性、存储体检、模型协议和并排升级版本，重点解决“窗口能打开但显示又用不了 / 实验写满 WSL 宿主盘 / 中转模型输出与工具调用变弱 / 换包后仍由旧 Bridge 提供服务”等问题。

> 验证边界：本版本已在 Windows 11 10.0.22631 + Ubuntu-24.04 完成端到端验证；309/其他差异环境尚未完成本版本 DeepSeek thinking 全链路复测。

> 中转说明：`https://10521052.xyz/v1` 是 CSA 项目方自建中转服务，不是模型厂商官方 API；使用前仍需确认域名与服务条款。

## 四项核心变化

### 1. 2×3 六状态界面与 WSL 存储体检

- 首页固定为可折叠的 2 行 × 3 列六项状态：WSL2、运行时、Bridge、Claude Science、WSL 存储和当前 API Key。
- “WSL 存储”显示 VHDX 的 Windows 位置、宿主盘余量、Linux 根分区余量和 VHDX 大小。
- 空间不足、根文件系统只读、临时目录不可写或 inode 紧张时给出警告；达到阻断阈值时不再盲目重启服务。
- Bridge 日志超过 50 MB 时只轮转自身日志并保留一份备份，避免日志持续挤占 VHDX。
- r2 在 C 盘或低空间场景显示“推荐迁移 / 辅助迁移”，弹窗展示本机存储证据并生成可复制给 Codex 的只读体检与迁移计划 Prompt。

> r2 的启动器只生成 Prompt，不执行迁移。实际 Move/export/import/unregister 仍未进入 v0.1.3 EXE，迁移执行继续保持 `BUILD NO-GO`；该入口可以忽略，不阻断启动或并排增量升级。

### 2. 输出参数与工具调用适配

- 正式请求不再注入统一的小 `max_tokens`；调用方没有传预算时，让上游模型采用自身默认值。
- OpenAI o-series 使用 `max_completion_tokens`；明确模型上限只在显式配置或上游明确返回限制时应用。
- Anthropic `tool_choice`、强制函数选择和 `disable_parallel_tool_use` 会转换到对应 OpenAI-compatible 参数，并保留并行工具调用偏好。
- 只有明确 HTTP 400/422 参数错误才做一次受限兼容重试；认证、额度、模型、网络和 5xx 不会被伪装成参数问题。

### 3. DeepSeek、MiniMax 与更多 Provider

- MiniMax 使用中国区 Anthropic-compatible 地址；DeepSeek 使用官方 Anthropic-compatible 地址。
- DeepSeek 原生请求会把 `thinking.type=auto` 转为 `adaptive`，避免有效 Key 被 400 误判不可用。
- Provider 初始模型保持为空；优先读取真实 `/models`，只有经过核验的官方候选且真实短对话成功后才允许保存。
- 自动映射支持“单模型映射全部 Claude 角色”和“多模型区分主力/快速模型”，第三方中转不会套用官方候选回退。

### 4. 从重装改为并排增量升级

- 从 v0.1.2/更早版本升级时，不卸载 WSL、Ubuntu 或 Claude Science 运行时。
- 下载完整新 ZIP 到新目录，由新版识别旧目录 Bridge、执行受控接管并校验 `source_path` 与配置版本。
- 同一电脑、同一 Windows 用户继续读取 APPDATA 设置和 DPAPI Key；验收成功前保留旧目录作为回退。
- 这是可回退的并排升级流程，不是静默下载和自动替换 EXE 的应用内更新器。

## 稳定性与一致性修复

- 修复状态显示矛盾：Claude Science 已由端口检测到时，不再显示“已停止”。
- 修复启动脚本假成功：WSL 启动脚本失败时，Windows 包装层会返回真实失败，不再把失败当成功。
- 状态探测改为一次只读体检并设置总超时，WSL/VHDX 异常时窗口不再无限转圈。
- 增强 WSL 只读诊断：如果 `/tmp`、Linux 用户目录或根文件系统异常，停止自动重启，避免扩大损坏。
- 改进非 systemd Bridge fallback：进程会可靠脱离启动 shell，并验证连续健康，而不是短暂监听后消失。
- Bridge 健康检查会验证 `source_path`，旧解压目录的进程不能再冒充当前版本。
- API Key/模型切换采用“预写 Windows 设置 → 更新并重启 Bridge → 验证配置版本 → 原子提交”的事务流程；失败会回滚。
- 默认模型、Provider 面板预设和示例配置改为空状态；用户输入模型或获取实时模型列表后再映射。
- `/v1/models` 正式支持空列表，不再因零模型触发崩溃。
- 自定义中转可填写自己的名称；留空时按“自定义中转 + 当前日期 + 当日序号”生成，不再出现多条同名配置。
- API Key 连通测试不再用极小的固定输出长度；先以 256 tokens 测试，仅在 reasoning/长度截断时对同一模型重试到 1024，避免把有效模型误判失败并擅自切换。
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

CSA v0.1.3 r2 提供位置检测、容量/可写性告警、危险操作阻断和只读辅助迁移 Prompt，不会自动迁移、注销、压缩或重建用户的 WSL 发行版。Codex 会先重新体检、判断迁移是否对症、推荐候选目标盘并输出计划；实际执行仍需后续关闭兼容矩阵和异机验收门槛。

## r2 发布后补丁

- 修复 Windows DrvFs 路径仅大小写不同却被判定为“其他或旧包 Bridge”的问题。`/mnt/c/.../New project 5` 与 `/mnt/c/.../new project 5` 现在按 Windows 路径身份比较；Linux 原生路径仍保持大小写敏感。
- Release EXE 的项目根优先来自当前 EXE/argv 路径，不再静默使用开发机编译期源码目录兜底。
- Bridge 归属失败时诊断会同时输出 expected、actual 和 `source_match`，便于异机定位。
- 当 WSL VHDX 位于 `C:` 时，“WSL 存储”会直接显示警告；该警告不等于授权迁移，也不会阻断普通启动。
- 新增存储辅助迁移弹窗与本机化 Codex Prompt；Prompt 生成器是纯前端只读逻辑，不调用 Tauri 服务命令、网络请求或文件写入。
- 包内 self-test 现在会检查现有 `.venv` 的依赖是否完整；首次安装依赖中断后可以直接重跑修复，不会因为半成品 venv 跳过安装。
