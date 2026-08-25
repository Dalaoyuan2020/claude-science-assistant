# CSA v0.1.7 出口自愈功能候选验收（2026-08-25）

## 结论与版本边界

T0–T6 已在 `codex/csa-v0.1.7` 完成。交付物是 **v0.1.7 feature-candidate 源码与 debug 验证产物**；启动器 UI/Cargo 元数据仍显示 `0.1.6`，没有制作、打开或发布正式 V0.1.7 ZIP。

最终现场：

```json
{
  "activeProvider": "deepseek",
  "bridgeRevision": "166128-1787631265733624000",
  "forceModel": "deepseek-v4-pro",
  "bridgePid": 157675,
  "claudePid": 108819,
  "pid8765": 108819,
  "pid8766": 108819,
  "outboundProxyUrl": "http://127.0.0.1:12334"
}
```

验收期间没有关闭启动器 GUI、Claude Science 或整个 WSL，没有触碰 2222，没有修改 Windows/WSL 系统代理、VPN、DNS、hosts、证书或 443。

## T0–T6 结果

| 任务 | 结果 | 关键证据 |
|---|---|---|
| T0 只读基线 | 完成 | `CSA_v017_T0_egress_baseline.md`；12334 三组可达，10808 三组拒绝；基线 smoke exit 0 |
| T1 候选发现 | 完成 | Windows 系统代理、白名单监听端口/进程名、直连；TCP 与完整 upstream base-URL path 分层；排序与单一推荐 |
| T2 修复 Prompt | 完成 | 沿用 `buildBridgeEgressRepairPrompt()`；五层、候选表、理由、直连反面提示、单字段 POST、验证与审批边界 |
| T3 一键应用 | 完成 | 新 command `apply_bridge_egress_fix`；0600 备份、单字段局部 POST、读回、真实探针、失败单字段回滚 |
| T4 Key 错误归因 | 完成 | 连接层指向出口；401/403 指向 Key；激活改为受管 Bridge 路径真实验证，不再被旧死代理预检鬼打墙 |
| T5 回归测试 | 完成 | 任务书五个指定测试全部存在并通过；`allow_inputs_frozen` 集合未变 |
| T6 文档/S5 | 完成 | troubleshooting 新章节；S5 增补真实切换/恢复与最终 smoke；本报告与 `TASKS_PROGRESS.md` |

## 候选发现与安全边界

现场只读候选结果的核心字段如下；401 是未带 Key 到达上游认证层，不是认证成功：

```json
{
  "candidates": [
    {
      "address": "http://127.0.0.1:12334",
      "source": "windows_system_proxy",
      "processName": "Hiddify",
      "tcp": {"state": "passed", "code": "work.bridge_egress.candidate.tcp_ok"},
      "upstream": {"state": "passed", "code": "work.bridge_egress.candidate.upstream_ok", "httpStatus": 401},
      "recommended": true
    },
    {
      "address": "direct",
      "source": "direct",
      "tcp": {"state": "passed", "code": "work.bridge_egress.candidate.tcp_ok"},
      "upstream": {"state": "passed", "code": "work.bridge_egress.candidate.upstream_ok", "httpStatus": 401},
      "recommended": false
    }
  ]
}
```

最终实现不是只测 origin 根路径：它请求当前配置的完整 base-URL path。完整 URL 经 `curl --config -` 的 stdin 传入，不进入 `/proc/<pid>/cmdline`；DTO/Prompt 只公开脱敏 origin。带 userinfo、query、fragment 或不安全 path 的 URL直接拒绝重放。

候选 proof：

- 只缓存已通过 TCP + base URL 的候选；5 分钟过期；单次消费并清空同批 sibling。
- proof 绑定 WSL distro、Bridge PID、source SHA-256、`/proc` starttime 和 upstream origin。
- 应用前重跑完整非计费权威探针，并要求当前报告中的 **同一个候选** TCP/base-URL 两层仍为 Passed；候选消失或退化即拒绝写入。
- 不信任 `_csa_revision` 代替内容/进程证明。

## 应用、备份与回滚

用户点击后发出的管理 body 严格为：

```json
{"outbound_proxy_url":"http://127.0.0.1:12334"}
```

只有 `outbound_proxy_url` 一个键；不修改 Key、Provider、模型、聚合路由、系统代理、VPN、DNS、hosts、证书或端口。备份位于受管 Bridge 配置目录：

```text
~/.claude-science/proxy/config.json.bak-<yyyymmdd-HHMMSS>
mode=0600
```

顺序固定为：

```text
验证受管 9876 owner/source
→ 0600 备份
→ POST /api/config 单字段
→ /health 读回
→ 一次 max_tokens=1 权威 egress 验证
→ 成功保留；失败恢复备份中的旧 outbound_proxy_url 单字段
```

回滚不会把整份旧配置覆盖回去，避免冲掉并发产生的 Key/模型改动。若旧代理 URL 带凭据，`/health` 的脱敏值按 proxy.py 同一规则比较，不会把成功回滚误报为失败；原始凭据不输出。应用与 Provider 切换共用跨进程 service lock 和进程内 transition lock。

当前配置已经是正确的 12334，因此本轮没有为了“做截图”强行执行一次无必要的真实 egress 写入；T3 写入/回滚由可执行脚本合同和 `apply_fix_*` 故障注入测试验收。

## Key 切换：真实切换并恢复

真实受管事务（ID 已脱敏）：

| 操作 | revision | Bridge PID | 产品请求 |
|---|---|---:|---|
| DeepSeek → OpenRouter | `135900-1787631237919379600` | 156999 | `stealth/ox-alpha`，success，`max_tokens=1` |
| OpenRouter → 原 DeepSeek | `166128-1787631265733624000` | 157675 | DeepSeek 路由，success，`max_tokens=1` |

两次 `live_api_key_switch_diagnostic` 均 `switchReturnedOk=true`、退出码 0；最终 `activeApiKeyId` 已恢复原 DeepSeek，Claude PID 始终为 108819。Bridge config patch/rollback 使用严格限长 JSON envelope 走 stdin，固定 argv 不含 Key 明文或 hex。

Bridge-only 重启现有 guest `42s` + host `48s` 双层 watchdog。初次激活失败后，只有配置文件恢复且 Bridge 回滚重启/运行态确认成功才会说“回滚完成”；回滚重启错误不再被吞掉。

## 最终测试

```text
Rust lib:          107 passed, 0 failed, 4 ignored
Rust integration:    9 passed, 0 failed
Node lanes:          13 passed, 0 failed
Vite build:          35 modules, passed
self-test:            54 translation tests passed
Python/package:       35 passed, 3 skipped; package policy passed
cargo fmt:            passed
git diff --check:     passed
```

最终 debug `--smoke`：

```text
PASS allow 431ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=lyuwinnie claudeRunning=true claudePid=108819 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=108819 pid8766=108819 controlSocket=true canOpen=true canStart=false
PASS paint 431ms budget=3000ms
PASS open 849ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 392ms health=200 models=200 identity=current modelCount=5
PASS egress 729ms work.bridge_egress.ok health=passed proxy=passed models=passed request=passed direct=skipped billable=true gating=false
PASS grade 3792ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=0 elapsed_ms=6195 exit=0
```

## 两条错误文案

连接层：

```text
bridge.api_key_activate_failed: 上游不可达。
Bridge 的出口代理 http://127.0.0.1:10808 无人监听或不可达（work.bridge_egress.proxy_dead）。
这不是 Key 的问题 —— 请打开「能力体检」查看出口修复建议。
详情：All connection attempts failed
```

认证层：

```text
bridge.api_key_activate_failed: 认证失败（HTTP 401/403 或上游认证错误）。这是 API Key 或账号权限问题，不是出口代理问题。
详情：HTTP 401: api key ****-key is invalid
```

## 任务书 §6 三问

1. **把出口配置改成死端口会发生什么？** `bridge_egress_detects_dead_proxy` 命中 `work.bridge_egress.proxy_dead`，模型与请求层立即 skipped，报告列候选并推荐 12334；`all_probes_red_button_still_open / bridge_egress_never_gates` 证明主按钮仍为“打开 Claude Science”、`disabled=false`。
2. **用户点“应用此修复”改了什么？** 只 POST `outbound_proxy_url`；先在受管目录创建 0600 时间戳备份，读回并真实验证；失败只恢复旧 `outbound_proxy_url`。`apply_fix_is_partial_update` 与 `apply_fix_rolls_back_on_failure` 通过。
3. **切 Key 时怎么区分代理和 Key？** 连接失败首行写“上游不可达”并给出 `proxy_dead` 和当前代理；401/403 首行写“认证失败”并明确不是出口问题。`preflight_error_attributes_layer` 同时冻结两条文案且禁止混淆。

## 尚未关闭的非阻断风险/发布项

- Provider 事务在 Bridge 已提交、Windows settings 尚未提交之间仍缺跨进程 durable journal。进程或机器在此窗口崩溃，可能出现 Bridge 新配置、UI 旧配置；正常错误路径已有回滚和双 watchdog，但崩溃恢复应在后续实现 `PREPARED / COMMIT_DECIDED` revision journal。此项不影响本轮正常/错误路径验收，但不能隐瞒。
- `bridge_egress_detects_dead_proxy` 的可执行故障夹具依赖本机 WSL；后续 CI 宜拆出纯逻辑 fixture，并把真实 WSL 版标为受控集成测试。
- A2/A7、真实聚合提交、故障现场真实点击“应用此修复”、正式 ZIP/SHA/secret scan/双入口/并排升级仍按 S5 标为待人工或发布阶段。
