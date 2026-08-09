# CSA v0.1.5 发布流程 · 复盘检测 → 加固 → 打包 → 包上检测 → 推 GitHub

> 2026-08-10 Napoleon 实地核对环境后写入。**功能已经可用，本流程是收尾发布。**
> 要求**一路执行到底，中途不要停下来等人确认**，遇到卡点记台账标「待人工」后继续。
> 工作副本：`csa-v0.1.5-model-roles`。

---

## 〇、当前实测状态（已核对，不用再查）

| 项 | 实测值 |
|---|---|
| Bridge | PID 4058529，00:35 启动，`source_path` 指向 model-roles 副本 ✓ |
| 聚合 | `aggregate_upstreams = 3`，`active_aggregate_scheme_id = scheme-2` ✓ |
| 配置来源 | `config_revision` 前缀 = 启动器 PID 14896 ✓ 说明切换确实由启动器写入 |
| 启动器 | PID 14896，00:11 启动，跑的是 model-roles 的 debug 构建 ✓ |
| 未提交改动 | 41 项 |
| 打包脚本 | `scripts/package-launcher-portable.ps1`（现成的） |
| 远端 | `github.com/Dalaoyuan2020/claude-science-assistant` |

**注意：T8 的三项改动并未应用**（`CSA_BRIDGE_ONLY=1` 仍在 `lib.rs:2499`、超时仍是 45s、预检仍在切换路径上）。功能可用是因为下面这个原因 ——

## 〇之二、为什么没打 T8 补丁也能用（重要，写进复盘）

**角色映射（聚合）这个设计本身把问题解决掉了。**

- v0.1.4 坏掉的场景是：**裸切 Provider/API Key**，模型别名集合本身变了，而 Claude Science 本体只在启动时取一次模型列表 → 本体拿着旧列表 → 表现为"切了没反应"。
- v0.1.5 的角色映射把客户端看到的模型名**固定成了稳定别名**（default / vision / fast），切换只改后端绑定，**客户端看到的名字始终不变** → 本体压根不需要刷新 → 只重启 Bridge 就够。

**结论：v0.1.5 的角色映射功能，顺带把 v0.1.4 的回归给治了。** 这是本次最值得记下的一条设计经验。

---

## 一、P0 · 复盘检测（发布前回归，在源码副本上做）

按顺序跑，全部记录**实际数字**：

1. `cd launcher && npm run build` → 通过
2. `cd launcher/src-tauri && cargo test` → 全绿，记录 passed/failed 数
3. `scripts\self-test.ps1` → 全绿
4. `scripts\verify-proxy.ps1` → 全绿
5. 功能实测（用命令行，**不要动正在运行的 GUI**）：
   - `curl -s http://127.0.0.1:9876/health` 确认 `aggregate_upstreams=3`
   - 切换 scheme-1 ↔ scheme-2 各一次，**记录每次实际耗时秒数**
   - 每次切换后发一条真实请求，确认响应来自绑定的上游

**任一项不过：记台账、标「待人工」、继续往下走，不要停。**

---

## 二、P1 · 最小加固（推荐做，风险极低）

**只做一件事**：给预检加进程级超时。

**为什么必须做**：`test_api_key_impl` 走 `run_powershell_with_stdin`（`lib.rs:1506`，内部 `.wait_with_output()`），**没有进程级超时**。今晚"一直转圈"就是它造成的。聚合模式下还会**逐 role 预检三次**。如果发给用户的包里留着这个，别人网络一慢就会复现同样的卡死。

**动作**：把 `test_api_key_impl` 里的 `run_powershell_with_stdin` 换成带超时的执行（仓库已有 `command_output_with_timeout`，见 `lib.rs:73`），**上限 20 秒**，超时按预检失败处理并给出明确错误。

**不要做的**：本次**不要**动 `CSA_BRIDGE_ONLY`、**不要**改 45 秒超时、**不要**删预检调用。功能已经可用，发布前只做这一处防卡死的加固，其余留到下个版本。

改完重跑 P0 的第 1–4 步确认没退化。

---

## 三、P2 · 整理提交

41 项未提交改动，分类处理：
- 属于 v0.1.5 功能与本次加固的 → 提交
- 属于本机环境/构建产物/临时文件的 → 确认是否该进 `.gitignore`，不要提交
- 无法判断归属的 → 记台账标「待人工」，**不要擅自提交**

提交信息用英文，遵循仓库既有风格（`feat:` / `fix:` / `docs:`）。

---

## 四、P3 · 打包

用现成脚本：`scripts\package-launcher-portable.ps1`

产物放 `dist/`，命名对齐既有惯例（参考 v0.1.4 的 `release-v0.1.4-publish-20260808`），本次用 `release-v0.1.5-publish-20260810`。

打 release 版，不是 debug 版。记录产物完整路径和大小。

---

## 五、P4 · 包上检测（关键，必须在包上做，不是在源码上）

**这一步不能省**：源码能跑不代表打出来的包能跑。今晚就吃过"跑的是旧构建"的亏。

1. 校验包内 exe 的时间戳晚于最新源码改动
2. 校验包内前端产物包含本次 UI（grep 包内 js 是否含「确认切换」「方案一」「方案二」）
3. 如包内含 `self-test`，在**包目录**下跑一次
4. 生成 `.sha256` 校验文件

---

## 六、P5 · 推 GitHub

1. 推分支到 `origin`（`github.com/Dalaoyuan2020/claude-science-assistant`）
2. 打 tag `v0.1.5`
3. 建 GitHub Release，上传 P3 的便携包 + `.sha256`
4. Release 说明必须写清楚：
   - 本版新增：多订阅角色映射（default / vision / fast）、方案一/方案二、预选+确认式切换
   - 本版修复：切换可靠性（去静默失败、revision 校验、可见日志）
   - **已知限制**：裸切 Provider 时若模型别名集合发生变化，Claude Science 本体需要重启才会刷新模型列表；使用角色映射则无此问题
   - 安装/升级方式，沿用既有 README 写法

**⚠️ 推送前自查**：确认没有任何 API Key、token、密码进入仓库或 Release 包。这条不过就停下来标「待人工」，不要硬推。

---

## 七、P6 · 更新 GitHub 页面

1. 更新 `README.md`：加 v0.1.5 段落，说明角色映射怎么用（两张表：订阅列表 + 角色/方案绑定）
2. 更新下载链接指向 v0.1.5
3. 如仓库有 GitHub Pages / 首页展示，同步更新版本号与新功能说明

---

## 八、交付（明早吕博士要看的）

写一份 `docs/reports/CSA_v015_release_report_20260810.md`，包含：
- P0 各项实际数字（测试通过数、切换耗时秒数）
- P1 是否做了、改了哪里
- P2 提交清单 + 哪些标了「待人工」
- P3 包路径、大小、sha256
- P4 包上检测结果
- P5 tag / Release 链接
- P6 页面更新了什么
- 开头一段「明早人工只需做什么」

## 九、纪律
- 一路做到底，遇卡点记台账标「待人工」后继续，不要停下等人
- 不要 kill 正在运行的启动器 GUI（权限不够）
- 不许改、删、放宽 `self-test.ps1` / `verify-proxy.ps1` 检查项
- 报告必须给实际数字，不许只写"正常了"
