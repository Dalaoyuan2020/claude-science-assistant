# CSA v0.1.7 T0 出口基线复核

- 日期：2026-08-25（Asia/Shanghai）
- 分支基线：`c9ca823 docs: record CSA v0.1.6 S5 pending acceptance`
- 复核原则：只读；未改 Bridge 配置、系统代理、VPN、DNS、hosts、证书或端口。

## 1. 当前出口配置与 Bridge 健康

配置文件只读取 `outbound_proxy_url` 字段，结果：

```json
{"outbound_proxy_url":"http://127.0.0.1:12334","proxy_enable":1,"proxy_server":"http://127.0.0.1:12334"}
```

`GET http://127.0.0.1:9876/health` 原始输出：

```json
{"status":"ok","deepseek_configured":true,"openai_configured":false,"custom_configured":false,"default_backend":"deepseek","force_model":"deepseek-v4-pro","model_list_mode":"aliases","model_aliases":5,"aggregate_upstreams":0,"active_aggregate_scheme_id":"","upstream_modes":{"deepseek":"anthropic","openai":"openai","custom":"openai"},"proxy_auth_mode":"optional","proxy_auth_configured":false,"outbound_proxy_configured":true,"outbound_proxy_url":"http://127.0.0.1:12334","inline_image_policy":"auto","proxy_dir":"/home/<wsl-user>/.claude-science/proxy","source_path":"/home/<wsl-user>/.local/share/csa/runtime/bridge/versions/bridge-0.1.6-6ec4b3246671dc3b/proxy.py","runtime_identity":{"schemaVersion":1,"component":"bridge","runtimeId":"bridge-0.1.6-6ec4b3246671dc3b","version":"0.1.6","buildId":"58139102631dd5b7","sourcePath":"/home/<wsl-user>/.local/share/csa/runtime/bridge/versions/bridge-0.1.6-6ec4b3246671dc3b/proxy.py","sourceSha256":"58139102631dd5b7d620d8ea7dfa294c3fda34b528cd662c6a40e80c5d06d410","pid":108539,"capabilities":["anthropicBridge","configRevision","health"],"managed":true},"config_revision":"152768-1787558525675221700"}
```

结论：人工临时处置仍然生效；Bridge 当前值与 Windows 系统代理声明值一致。

## 2. 三路线实测

测试目标：DeepSeek `/v1/models`、OpenAI `/v1/models`、PyPI `/simple/`。请求不携带凭据；401 表示已到达服务端认证层。

| 路线 | DeepSeek | OpenAI | PyPI |
|---|---|---|---|
| 直连 | HTTP 401 | 连接超时 | HTTP 200 |
| 当前代理 12334 | HTTP 401 | HTTP 401 | HTTP 200 |
| 已知死口 10808 | 连接拒绝 | 连接拒绝 | 连接拒绝 |

原始输出：

```text
ROUTE=direct TARGET=DeepSeek
http_code=401 remote_ip=223.109.219.89 time_connect=0.028451s time_total=0.610757s error=
exit_code=0
ROUTE=direct TARGET=OpenAI
http_code=000 remote_ip= time_connect=0.000000s time_total=4.005131s error=Connection timed out after 4005 milliseconds
curl: (28) Connection timed out after 4005 milliseconds
exit_code=28
ROUTE=direct TARGET=PyPI
http_code=200 remote_ip=151.101.64.223 time_connect=0.082167s time_total=4.107600s error=
exit_code=0
ROUTE=current_proxy_12334 TARGET=DeepSeek
http_code=401 remote_ip=127.0.0.1 time_connect=0.001170s time_total=0.067657s error=
exit_code=0
ROUTE=current_proxy_12334 TARGET=OpenAI
http_code=401 remote_ip=127.0.0.1 time_connect=0.001578s time_total=0.795717s error=
exit_code=0
ROUTE=current_proxy_12334 TARGET=PyPI
http_code=200 remote_ip=127.0.0.1 time_connect=0.006482s time_total=6.075776s error=
exit_code=0
ROUTE=dead_proxy_10808 TARGET=DeepSeek
http_code=000 remote_ip= time_connect=0.000000s time_total=2.046201s error=Failed to connect to 127.0.0.1 port 10808 after 2046 ms: Could not connect to server
curl: (7) Failed to connect to 127.0.0.1 port 10808 after 2046 ms: Could not connect to server
exit_code=7
ROUTE=dead_proxy_10808 TARGET=OpenAI
http_code=000 remote_ip= time_connect=0.000000s time_total=2.045150s error=Failed to connect to 127.0.0.1 port 10808 after 2045 ms: Could not connect to server
curl: (7) Failed to connect to 127.0.0.1 port 10808 after 2045 ms: Could not connect to server
exit_code=7
ROUTE=dead_proxy_10808 TARGET=PyPI
http_code=000 remote_ip= time_connect=0.000000s time_total=2.030317s error=Failed to connect to 127.0.0.1 port 10808 after 2030 ms: Could not connect to server
curl: (7) Failed to connect to 127.0.0.1 port 10808 after 2030 ms: Could not connect to server
exit_code=7
```

结论：置空并不能覆盖当前网络；直连 OpenAI 失败。当前 Windows 系统代理 12334 是三组目标均可达的候选。

## 3. 启动器对出口字段的当前感知

```text
outbound_proxy_count=0
```

命令等价于对 `launcher/src-tauri/src/lib.rs` 中 `outbound_proxy` 的精确匹配计数。T0 时仍为零。

## 4. 完整 smoke

```text
PASS allow 535ms inputs=claudeRunning,windowsBridgePid wslInstalled=true distro=Ubuntu-24.04 linuxUser=<wsl-user> claudeRunning=true claudePid=108819 windowsBridgePid=none windowsBridgeProbe=checked runtimePresent=true listenerProbeOk=true listenerPresent=true daemonState=managed_ready pid8765=108819 pid8766=108819 controlSocket=true canOpen=true canStart=false
PASS paint 535ms budget=3000ms
PASS open 906ms login.url_ready loopback=true port=8765 nonce=present
PASS bridge 508ms health=200 models=200 identity=current modelCount=5
PASS egress 1054ms work.bridge_egress.ok health=passed proxy=passed models=passed request=passed direct=skipped billable=true gating=false
PASS grade 3410ms state=running warnings=0 gating=false
SUMMARY required_pass=4 required_fail=0 non_gating_fail=0 elapsed_ms=6417 exit=0
```

结论：当前修复态下 required 四项全绿，出口探针仍为 `gating=false`。
