# CSA v0.1.5 T1 诊断报告：Bridge-only 是否足够

## 结论

**B：只重启 Bridge 足够。** Claude Science 本体不读取或缓存 CSA 的 API Key、Provider、Base URL 映射和 `force_model`。本体只在启动时取得一个稳定不变的本地入口 `ANTHROPIC_BASE_URL=http://127.0.0.1:9876`；每次请求到达 Bridge 后，Bridge 才根据请求中的模型 ID 解析实际上游。

因此，本轮不应改成完整重启 Claude Science。T2 应走 **T2-B**，修复 `restart_bridge_after_config` 的启动、等待和 revision 校验路径。

## 代码证据

1. `scripts/start-claude-science-wsl.sh:452` 启动本体时只注入固定的 `ANTHROPIC_BASE_URL="$PROXY_URL"`，其中地址始终是本地 Bridge。
2. 同一脚本 `317` 行的 `CSA_BRIDGE_ONLY=1` 在 token、本体停止和本体启动逻辑之前退出；这只替换 Bridge，不改变 Claude Science 的本地入口。
3. `proxy.py:328` 的 `config = Config()` 属于 Bridge 进程；Bridge 重启会重新读取配置文件。
4. `proxy.py:1998-2001` 对每个消息请求读取 `body["model"]`，随后调用 `config.resolve_backend(original_model)`。实际 Provider、Key、Base URL 和模型映射在 Bridge 请求路径中决定，而不是 Claude Science 本体中决定。
5. `launcher/src-tauri/src/lib.rs:2246-2247` 已明确使用 `CSA_FORCE_RESTART=1 + CSA_BRIDGE_ONLY=1`；方向正确，问题在执行可靠性而不是重启范围。

## 已定位的 T2-B 嫌疑

- `restart_bridge_after_config` 在状态快照显示 `bridge_running=false` 时直接 `Ok(())`，配置虽然写入但 Bridge 没启动，界面会得到假成功。
- WSL 发行版缺失时同样直接 `Ok(())`，真实错误被吞掉。
- revision 校验只在实际走到重启后执行；上述早退绕开了生效证明。

## 病根一句话

不是 Claude Science 缓存旧模型，而是启动器在 Bridge 未运行或缺少发行版时跳过了应用步骤，却仍把切换报告为成功。

## T2 路线

执行 **T2-B**：让切换事务无论 Bridge 原先是否运行，都必须启动/重启当前包的 Bridge，并核对 revision；缺少 WSL、启动失败或 revision 不一致必须返回错误并回滚。

## 本阶段未做

- 未修改任何产品代码。
- 未切换真实 API Key。
- 未重启 Claude Science 或 Bridge。
