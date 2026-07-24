# CSA 本地可观测性与外部 Agent 调用可行性测试

本文定义第一阶段建设范围：先验证 CSA 是否能以只读方式读取本机项目/会话/运行状态，并判断是否具备授权调用外部 CLI Agent 的基础条件。该阶段不做完整后台 UI，不执行真实模型任务，不修改网络、系统代理、WSL 配置或用户密钥。

## 目标

第一阶段只回答六个问题：

1. 本机是否存在可观测的 CSA、Claude、Codex 或 Claude Science 本地状态目录。
2. 这些目录是否可读，是否存在 JSON、JSONL、SQLite 等结构化状态文件。
3. 是否能从本地文件中发现项目和会话索引。
4. 是否能采集环境、硬件和资源状态。
5. 是否存在可调用的外部 CLI Agent，例如 `claude` 或 `codex`。
6. 如果要进入后台化建设，第一批 API 边界应如何设计。

## 已交付脚本

核心脚本：

```powershell
.\scripts\csa-feasibility-probe.ps1
```

默认行为：

- 只读扫描。
- 默认采样本项目目录、Windows 用户目录下的 `.claude`、`.claude-science`、`.codex`、CSA AppData，以及 WSL home 下的相关目录。
- 只抽取 JSON/JSONL 结构字段，不输出完整会话内容。
- 遇到 `.env`、token、secret、private key、cookie 等敏感命名文件时跳过内容采样。
- 默认只执行 CLI 的 `--version` / `--help` 类探测，不调用真实模型。
- 生成 JSON 和 Markdown 两份报告。

示例：

```powershell
.\scripts\csa-feasibility-probe.ps1
```

更快的只读扫描：

```powershell
.\scripts\csa-feasibility-probe.ps1 -ExternalCliProbeMode none -MaxFilesPerRoot 60
```

指定额外目录：

```powershell
.\scripts\csa-feasibility-probe.ps1 -TargetRoots "D:\ResearchProject","C:\Users\Admin\.claude"
```

真实 Agent smoke test 需要显式开启：

```powershell
.\scripts\csa-feasibility-probe.ps1 -ExternalCliProbeMode agent
```

`agent` 模式可能触发外部 Agent 认证、额度或模型调用，因此不得作为后台默认巡检模式。

## 输出

报告默认写入：

```text
reports/csa-feasibility/<timestamp>/feasibility_report.json
reports/csa-feasibility/<timestamp>/feasibility_report.md
```

`feasibility_report.json` 是后续后台服务应优先消费的机器可读结果。关键字段：

| 字段 | 含义 |
|---|---|
| `feasibility.overall` | `feasible`、`conditional` 或 `not_ready` |
| `feasibility.local_storage_found` | 是否发现目标本地状态目录 |
| `feasibility.local_storage_readable` | 是否能读取这些目录 |
| `feasibility.structured_state_detected` | 是否发现 JSON/JSONL/SQLite 类结构化文件 |
| `feasibility.session_index_detected` | 是否发现项目/会话索引 |
| `feasibility.external_cli_agent_detected` | 是否发现且能执行至少一个外部 CLI Agent |
| `local_storage_roots` | 每个目录的存在性、可读性、采样文件、会话索引 |
| `external_cli_tools` | CLI 是否存在、版本探测、帮助信息特征 |
| `resources` | OS、CPU、内存、磁盘、GPU、电池等资源快照 |
| `csa_runtime_status_probe` | 对现有 `scripts/status-probe.ps1` 的聚合结果 |

## 当前本机验证结论

在当前机器上，默认探针已验证：

- 可以读取项目目录。
- 可以读取 Windows 用户目录下的 `.claude`。
- 可以发现 `.claude/projects` 下的会话索引。
- 可以读取 `.claude-science`、`.codex`、CSA AppData 和 WSL `.claude-science` 的结构信息。
- 可以采集 OS、CPU、内存、磁盘、GPU/电源能力范围内的资源状态。
- `claude` CLI 可执行，版本探测通过。
- `codex` 当前在 WindowsApps 路径下被系统拒绝直接执行，应标记为“发现但不可调用”，不能作为第一阶段可用 Agent 依据。

因此本机第一阶段结论为：**可行**。后续后台化可以先依赖 Claude CLI 作为外部 Agent 通道，同时保留 Codex CLI 的修复/替代路径。

## 后台接口建议

第一批后台接口只围绕“探测、索引、外部调用、资源快照”建设：

```http
POST /api/probes/local-observability/runs
GET  /api/probes/local-observability/runs/:runId

GET  /api/projects/discovered
GET  /api/projects/:id/local-sessions

GET  /api/external-tools
POST /api/external-runs
GET  /api/external-runs/:runId/status
GET  /api/external-runs/:runId/logs

GET  /api/resource-snapshots/latest
POST /api/resource-snapshots
```

后台不应直接读取用户目录或执行 CLI。建议通过本地 Worker 执行探针，后台只消费结构化报告。

## 安全边界

第一阶段必须保持以下边界：

- 默认只读。
- 不读取或输出完整密钥、token、cookie、私钥、完整 Prompt 或完整会话内容。
- 不修改 Clash、VPN、TUN、DNS、hosts、系统代理、证书或 443 端口。
- 不执行真实模型调用，除非用户显式开启 `-ExternalCliProbeMode agent`。
- 外部 CLI 调用必须支持命令白名单、工作目录限制、超时、stdout/stderr 捕获和审计日志。
- 后台 UI 展示报告时必须继续做脱敏，不能把 JSON 当成完全可信内容直接渲染为 HTML。

## 下一阶段

建议第二步实现一个最小 CSA Worker：

1. 封装 `csa-feasibility-probe.ps1` 为后台任务。
2. 保存最新 `feasibility_report.json`。
3. 暴露 `/api/probes/local-observability/runs`。
4. 把 `.claude/projects` 的会话索引映射为 CSA 项目候选。
5. 把 `claude` CLI 注册为第一个可用外部 Agent。
6. 对 `codex` CLI 记录不可调用原因，后续通过安装方式或 alias 修复。

## 报告 schema v1 补充字段

为了让报告可以直接进入后台系统，`feasibility_report.json` 除基础探测结果外，还包含以下结构：

| 字段 | 用途 |
|---|---|
| `target` | 记录目标软件、运行环境、测试机器和允许范围 |
| `environment` | 记录 OS、当前用户、shell、工作目录、WSL/Docker/本机 CLI 运行形态线索 |
| `assessment.risk_level` | 统一风险等级：`medium`、`high`、`critical` |
| `assessment.recommend_next_stage` | 是否建议进入下一阶段 Worker/后台接口开发 |
| `assessment.verified_capabilities` | 已由本机实测证明的能力清单 |
| `assessment.unverified_capabilities` | 尚未验证或验证失败的能力清单 |
| `assessment.risk_register` | 风险、证据来源和缓解方案 |
| `assessment.recommended_remediations` | 按失败场景给出的补救方案 |
| `assessment.official_interface_priority` | 官方 export、CLI JSON、SDK、hook、API 优先于内部文件解析的策略和候选线索 |
| `assessment.acceptance_matrix` | 对本阶段验收标准的逐项判定 |

证据来源使用 `source_kind` 标记：

- `machine_test`：本机实测确认。
- `official_docs`：需要或已经来自官方文档确认。当前本地探针默认不联网，因此这类项通常标记为 `not_checked_by_probe`。
- `inference`：基于文件结构、CLI help 或风险模型的推断，不能当作稳定产品契约。

后台判断是否允许进入下一阶段时，至少应检查：

```text
feasibility.overall == "feasible"
assessment.recommend_next_stage == true
assessment.acceptance_matrix[*].status 不存在 incomplete
```

如果 `official_docs_contract` 仍为 `not_checked_by_probe`，可以进入 Worker 原型阶段，但不能把内部文件格式当成长期稳定接口发布。
