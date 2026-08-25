# CSA V0.1.7 公开发布源码验收（2026-08-25）

## 结论

V0.1.7 的源码、版本身份、三套外观、帮助入口和既有功能合同已完成发布前验收，可以进入 clean-source 正式打包门。开发过程中曾使用的 `v0.1.8 feature-candidate` 只是未发布的内部 UI 候选；用户最终明确把本轮公开编号定为 **V0.1.7**，本次没有覆盖任何已经发布的 V0.1.8。

本报告记录源码与隔离调试程序的验收结果。正式 ZIP 是否完成，必须继续以产物目录中的 `manifest.json`、同名 `.sha256`、包内 secret scan 和独立解压验收为准，不能只凭本报告宣称已上传 GitHub Release。

## 本轮范围

- 保留 V0.1.5 的单 Key/三 Key 聚合、Provider 目录、Bridge 事务和失败回滚。
- 保留 V0.1.6 的端口所有权、受管运行时身份、WSL 挂载阻塞与 HTTP/SOCKS 沙盒出口检测，并继续让 GRADE/WORK 非门禁。
- 在同一功能 DOM 上提供亮色终端、经典面板、深色面板三套外观；Classic/Dark 主控、F1、F2、F3 等宽，自适应窄窗口。
- 修复亮色终端、Classic 和深色面板的小字/激活态对比度；深色卡片使用浅色文字，二维码始终保留白底原图。
- 底部新增“帮助”，提供第一次添加和启用 Key、切换故障归因、三个 Key 分工、深检/能力体检/修复区别、版本历史与人工支持二维码。
- 人工帮助页明确要求只提供版本、脱敏错误与复现步骤，禁止发送 API Key、Token、完整配置或未脱敏截图。

## 版本身份

以下发布面统一为 0.1.7：

| 发布面 | 结果 |
| --- | --- |
| Cargo.toml / Cargo.lock 根包 | 0.1.7 |
| package.json | 0.1.7 |
| tauri.conf.json | 0.1.7 |
| Windows 窗口标题 | `CSA V0.1.7 - Claude Science Assistant` |
| 页面版本徽标 | `v0.1.7` |
| Bridge 启动、修复 Skill、WSL bootstrap fallback | 0.1.7 |
| 当前 Release 文档与预期产物名 | v0.1.7 |

`package-launcher-portable.ps1` 会在构建前解析上述五个应用身份面，并额外核对三处 Bridge package fallback 和动态 `docs/github-release-v<version>.md`。任一处缺失或不一致即拒绝打包。以脏工作树试跑时，版本检查先通过，随后按预期在 clean-source 门 fail-closed；没有开始构建或写正式资产。

历史兼容夹具没有做机械全局替换：旧 0.1.6 Bridge 身份、runtime lifecycle、host-grant 备份名和旧 Release 文档仍保留。当前版本相关的 smoke 期望改为从 `CARGO_PKG_VERSION` 动态派生。

## API Key 切换验收口径

本轮没有修改 `activate_api_key`、聚合方案提交、Bridge patch、真实路由验证、revision 核对或失败回滚事务；UI 只增加外观、帮助、命名/重命名和两步删除呈现。相对 `8a7e12e3043d6fdc9e6d1104097fa907b168a7ae`，44 个 Provider/API Key/Bridge 权威函数中 43 个字节完全相同，未变函数组合 SHA-256 为 `8149dbd35e3bad1124c48f276710a9978fce5929cf7e6b2b3e6f873095a259f9`；唯一变化的 `save_api_key_impl` 只把显示名称改为共用校验，仍只保存设置，不触发 Bridge、激活或请求。既有 V0.1.7 出口候选已真实完成 DeepSeek → OpenRouter → 原 DeepSeek 往返，两次 `max_tokens=1` 均成功，活动接入与 Claude PID 均恢复/保持；用户也在现场确认当前版本可以切换。

因此本轮复用同一事务实现的真实证据，没有为重复截图再次发送计费请求。发布策略固定为：零费用合同测试每次运行；只有 Provider/Bridge 事务切片变化或最终 RC 缺少同实现证据时，才在明确费用和恢复方案后补一次真实往返。聚合模式不是无效 Key 的绕过办法。

费用文案同时做了纠偏：“确认切换”仍明确为一条 `max_tokens=1`，三 Key 聚合仍明确为三条；新增接入中的“测试”会读模型列表，并可能遍历多个候选模型和两档输出预算，因此可能发送多次真实请求。界面与 FAQ 已禁止反复点击，并要求 401/403 先修正 Key、额度或权限后只重试一次。

## 自动化结果

```text
Node ALLOW/GRADE/WORK + UI contracts: 23 passed, 0 failed
Vite production build:                  passed, 38 modules
Rust library:                           113 passed, 0 failed, 4 ignored
Rust integration:                       9 passed, 0 failed
Rust doctests:                          0 failed
Translation tests:                      54 passed
Python network/host-grant tests:         35 passed, 3 skipped
Package policy:                         passed
Cargo check / Cargo fmt:                passed
git diff --check:                       passed
```

四个 ignored Rust 测试均有明确外部条件：三个会访问真实运行时/Key/模型或修改当前接入，一个读取官方运行时索引。本轮没有把 ignored 当成失败，也没有无授权代跑。

第一次从独立 clean worktree 运行 release gate 时，源码合同测试在 Git 的 CRLF checkout 下无法匹配写死的 LF 多行边界，出现 112 passed / 1 failed。功能代码没有失败，但正式门按预期停止，未生成资产。测试随后在匹配前统一把 CRLF 规范化为 LF，并从 `launcher/src-tauri` 工作目录复测通过；正式打包必须从包含该修复的新提交重新开始，不能沿用失败构建。

后续 clean-source 门、Rust/WSL 故障注入和 20 次生命周期测试全部通过后，首次优化版 Tauri 冷编译中的 `rustc` 进程以 Windows `STATUS_ACCESS_VIOLATION` 退出，没有产生源码诊断，也没有生成 ZIP。打包脚本原本只把测试固定为单 Cargo job，最终 release build 仍可能并行；现已让最终 Tauri 构建在调用方未显式设置时默认使用 `CARGO_BUILD_JOBS=1`，并在结束后恢复原进程环境。正式资产仍须从包含该修复的新提交完整重跑，不能复用崩溃构建。

新增 UI 回归覆盖：三皮肤 LauncherSettings 往返和非法值回退、单一主 DOM、三个 `aria-pressed` 选项、Help dialog 语义、背景 inert、Escape/Tab/焦点恢复源码合同、二维码资源存在且不引用过期群码，以及关键前景/背景组合的 WCAG AA 4.5:1 下限。

## 隔离 Windows 可视验收

使用全新隔离 APPDATA 启动最终 V0.1.7 debug 程序，没有读取真实 LauncherSettings 或 Key：

1. 窗口标题为 `CSA V0.1.7 - Claude Science Assistant`，首次页完整显示亮色终端、经典面板、深色面板三项，三列在当前窗口内无横向溢出。
2. 选择深色后，页面徽标为 `v0.1.7`；主控、F1、F2、F3 四键等宽，状态、面板、空接入区和激活态均未出现暗底暗字或突兀白块。
3. 底部“帮助”可打开；FAQ、费用提示、安全告警和用户授权的个人微信二维码完整显示，二维码保持白底且未反色。
4. Escape 可关闭帮助；关闭测试窗口并以同一隔离 APPDATA 重启后，深色外观直接恢复，不再出现首次选择页。
5. 最终隔离窗口已关闭；没有停止用户真实 CSA、没有关闭 WSL，也没有触碰 2222 等无关服务。

## 正式交付门

只有 clean source 的 `package-launcher-portable.ps1 -Profile release` 全部通过，才生成并考虑上传：

- `claude-science-assistant-v0.1.7-release-portable.zip`
- `claude-science-assistant-v0.1.7-release-portable.zip.sha256`

打包门必须完成离线 release gate、release EXE 重建、源码 commit/tree 稳定性复核、vendor runtime 哈希、空示例配置、包内 secret scan、ZIP/SHA-256 与独立目录解压验收。远端 tag、push 和 GitHub Release 是另一项外部写操作；本报告不把本地通过冒充成远端发布完成。
