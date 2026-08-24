# CSA v0.1.6

CSA v0.1.6 继续锁定 Claude Science 0.1.25，把“端口是否监听”升级为可复核的端口、进程、Bridge 身份与沙盒出口质检，同时保留 v0.1.5 的多订阅三模型聚合和事务化切换。

## 下载

- `claude-science-assistant-v0.1.6-release-portable.zip`
- `claude-science-assistant-v0.1.6-release-portable.zip.sha256`

请只从本仓库的 v0.1.6 GitHub Release 下载完整 ZIP，并使用同页 `.sha256` 文件核验。不要只复制或替换 EXE；本版的健康身份同时依赖启动器、`proxy.py`、脚本、Skill 和受管运行时清单。

## 新增

- 分层端口质检：验证 Bridge `9876` 与 Claude Science `8765/8766` 的实际监听者、受管路径、可执行文件和 PID 拓扑。
- Bridge 包迁移：Bridge 以 `bridge-0.1.6-<bundle-hash>` 内容寻址目录激活，`/health.runtime_identity` 同时报告版本、build ID、源码哈希和 PID。
- 沙盒出口探针：经真实 `analysis/socks.sock`、SOCKS5H 和固定 PyPI HTTPS HEAD 验证出口；不发送模型请求，不产生模型费用，并要求同一进程身份下相邻两次成功才发布绿色缓存。
- 挂载 I/O 诊断：单独报告 Linux `D` 状态与 `p9_client_rpc` 等 wait channel，区分 WSL DrvFS/9P 卡住、沙盒转发失败和公网出口故障。
- MCP 真正按需加载且保留计算环境首装：禁用启动阶段 `QT9` 内置 MCP 与 custom-MCP 元数据预热，catalog snapshot 不再启动 server；保留默认 Python/R/BYOC 的 `NYz` 排队、创建与重试。
- Git 安全扫描按需执行：启动时不再递归所有持久 RW host grant；首次真实 sandbox 命令和后续新增授权仍经过未削弱的 `_ensureGitScan()` 安全门。
- 持久授权质检与一键收窄：只读取授权字符串、不遍历目标目录，兼容现代/旧授权格式并识别全部有效 DrvFS RW；“修复并重启”先保存 0600 私有备份，再保留读取、撤销 DrvFS 持久写入。
- 核心服务优先唤起：启动器首次加载只尝试一次幂等初始化；无监听冲突且运行时可用时自动启动 Claude Science，周期刷新不重复启动。技术依赖仍保持先验证 Bridge、再启动指向它的 Claude Science daemon。
- 版本身份统一：界面、窗口标题、Tauri/Cargo、便携包 manifest、启动脚本和 Bridge runtime ID 统一为 0.1.6；内置 Claude Science 仍保持 0.1.25。

## 修复与加固

- “打开 Claude Science”不再把一次性的 control-socket/锁文件切换误报成“服务未启动”：启动器会串行等待 Windows 与 WSL 生命周期锁，固定使用体检确认的 WSL 用户，对瞬态控制通道失败做有限退避重试，并分别报告运行时缺失、生命周期忙、daemon 未就绪和控制通道超时。一次性 nonce 只在 Rust 后端交给系统浏览器，不经过前端状态或诊断文本；仅允许打开 `localhost`/loopback 的 `8765` 登录地址。
- 不再把“端口打开”直接显示成“系统正常”，也不会把另一个或旧目录的 Bridge 当作当前包。
- Bridge-only 且没有 Claude 监听者时按普通“待启动”处理；单端口或未验证监听者显示为“拓扑/身份待验证”，不再重复提示“Claude Science 尚未启动”。
- 旧 Windows Bridge 会阻断自动与手动 WSL 启动，并始终显示显式迁移入口，避免形成双 Bridge；经 `patched-current` 启动的受管 daemon 在运行时指针升级后仍能被检查器与生命周期一致识别。
- Claude Science 就绪探针不发送任何 daemon HTTP：只验证 `8765/8766` 同 PID、受管 EXE、`serve` argv 和进程线程可安全检查性；事件循环与外网出口由独立 deep SOCKS5H canary 判定，避免把 SPA、feature flag 或图像 provider 混入本地启动门。
- 代理变量冲突只在新建 Claude Science 子进程的环境里清理，不改 Windows/WSL 系统代理、VPN、DNS、hosts 或证书。
- 生命周期操作只针对通过端口、可执行路径、启动代际和运行时身份联合验证的 CSA 进程；普通自检与修复不会自动执行全局 `wsl --shutdown`。
- 接受已发布 v0.1.5 PowerShell 清单的 UTF-8 BOM，旧包可迁移，同时保持路径、进程与 manifest 所有权检查。

## 首次安装

1. 下载完整 ZIP 和 `.sha256`，核验后解压到独立短路径，例如 `D:\CSA-v0.1.6`。
2. 阅读 `docs/quick-start.zh-CN.md`，或把 `docs/prompts/csa-install-or-upgrade-agent-prompt.zh-CN.md` 交给本地 Codex 先做只读体检。
3. 双击 `claude-science-assistant.exe`。若 WSL/Ubuntu 尚未安装，系统级安装仍需单独确认并可能要求重启。
4. 添加供应商、测试连接，再选择 API 接入或三模型聚合方案。

## 从 v0.1.5 或更早版本升级

1. 不卸载 WSL、Ubuntu 或 Claude Science 数据，也不要覆盖旧解压目录。
2. 把 v0.1.6 完整 ZIP 解压到新目录，关闭旧启动器后运行新 EXE。
3. 点击“修复并重启”，让新版把 Bridge 激活到受管 `bridge-0.1.6-<bundle-hash>` 运行时；若检测到有效 DrvFS RW 授权，此动作会先备份偏好并将其转换为 RO（ext4 RW 不变）。不要要求 `source_path` 继续指向便携包原目录。
4. 刷新状态，确认 Bridge runtime identity 的 `version=0.1.6`、build ID/源码哈希匹配，`8765/8766` 属于同一个受管 Claude Science PID，网络质检无冲突。
5. 完成一次你实际使用的模型或视觉请求。验收稳定前保留旧目录；需要回退时关闭新版，再从旧完整包启动。

## 验证边界与已知限制

- 网络质检使用固定、匿名、非计费的 PyPI canary，只证明本机指定沙盒出口链路可用，不等于所有模型供应商都已通过真实鉴权和计费请求。
- Claude Science 本地就绪检查不请求 `GET /`、`GET /health` 或任何业务 API；固定 PyPI canary 只经沙盒 SOCKS5H 出口执行匿名 HEAD，不进入提示词、模型或 MCP 业务调用链。
- V0.1.6 不会在 daemon 或默认 Python/R 首装阶段递归扫描 Windows RW grant，因此 UI 和端口就绪不再被该扫描拖死；首次真实 analysis/MCP sandbox 命令仍会 fail-closed 地执行 Git safety scan。检测到 DrvFS RW 时请用“修复并重启”自动保留读取并撤销持久写入，或把需写入的热代码与 `.git` 放到 WSL ext4。手工工具仍支持 `--apply` 精确删除标准宽泛项和 `--convert-drvfs-rw-to-ro` 转 RO，且都会先保存 0600 原始备份。
- Visual/多模态能力仍取决于所选上游模型与供应商。模型名匹配只能生成建议，不能替代真实图片请求验收。

## 发布校验

正式 Release 以同页 `.sha256`、包内 `manifest.json` 和 GitHub 资产下载复核为准。`manifest.json` 应显示 `version=0.1.6`、`profile=release`、`sourceTreeDirty=false`，且 bundled Claude Science 为 0.1.25。
