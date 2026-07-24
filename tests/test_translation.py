import asyncio
import hashlib
import importlib.util
import json
import tempfile
import warnings
from contextlib import contextmanager
from pathlib import Path

try:
    from starlette.exceptions import StarletteDeprecationWarning
except ImportError:
    StarletteDeprecationWarning = DeprecationWarning

warnings.filterwarnings("ignore", category=StarletteDeprecationWarning)
from starlette.testclient import TestClient


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("proxy", ROOT / "proxy.py")
proxy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(proxy)


@contextmanager
def image_policy(policy: str):
    old = proxy.config._data.get("inline_image_policy")
    proxy.config._data["inline_image_policy"] = policy
    try:
        yield
    finally:
        proxy.config._data["inline_image_policy"] = old


@contextmanager
def reasoning_policy(policy: str):
    old = proxy.config._data.get("reasoning_content_policy")
    proxy.config._data["reasoning_content_policy"] = policy
    try:
        yield
    finally:
        proxy.config._data["reasoning_content_policy"] = old


@contextmanager
def config_values(**values):
    old = {key: proxy.config._data.get(key) for key in values}
    proxy.config._data.update(values)
    try:
        yield
    finally:
        proxy.config._data.update(old)


@contextmanager
def temporary_path():
    with tempfile.TemporaryDirectory() as directory:
        yield Path(directory)


def test_connect_diagnostic_marker_is_default_off_and_reads_latest_user_text_only():
    body = {
        "messages": [
            {"role": "user", "content": "[CSA#old] old"},
            {"role": "assistant", "content": "reply"},
            {"role": "user", "content": [{"type": "text", "text": "[CSA#test1] probe"}]},
        ]
    }

    with config_values(connect_diagnostic_tap_enabled=False):
        assert proxy.connect_diagnostic_marker(body) == ""
    with config_values(connect_diagnostic_tap_enabled=True):
        assert proxy.connect_diagnostic_marker(body) == "[CSA#test1]"
        assert proxy.connect_diagnostic_marker({"messages": [{"role": "user", "content": "prefix [CSA#test1]"}]}) == ""


def test_connect_diagnostic_sse_aggregator_handles_split_utf8_and_ignores_non_text_deltas():
    aggregator = proxy.DiagnosticSSETextAggregator()
    payload = (
        'event: content_block_delta\n'
        'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"收到"}}\n\n'
        'event: content_block_delta\n'
        'data: {"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"secret"}}\n\n'
        'event: content_block_delta\n'
        'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":" OK"}}\n\n'
        'event: message_stop\n'
        'data: {"type":"message_stop"}\n\n'
    ).encode("utf-8")
    split = payload.index("收".encode("utf-8")) + 1
    aggregator.feed(payload[:split])
    aggregator.feed(payload[split:])

    assert aggregator.message_stopped is True
    assert aggregator.finish() == "收到 OK"


def test_connect_diagnostic_log_contains_only_marker_length_and_hash():
    old_entries = list(proxy.request_log)
    try:
        proxy.request_log.clear()
        with config_values(connect_diagnostic_tap_enabled=True):
            proxy.log_connect_diagnostic_tap("[CSA#test1]", "M0-TEST1-OK.", stream=True)
        assert len(proxy.request_log) == 1
        entry = proxy.request_log[0]
        assert entry["marker"] == "[CSA#test1]"
        assert entry["response_chars"] == len("M0-TEST1-OK.")
        assert entry["response_sha256"] == hashlib.sha256(b"M0-TEST1-OK.").hexdigest()
        assert "response_text" not in entry
        assert "M0-TEST1-OK." not in json.dumps(entry)
    finally:
        proxy.request_log[:] = old_entries


def test_connect_tap_marker_is_default_off_and_requires_leading_marker():
    body = {"messages": [{"role": "user", "content": "[CSA#123e4567-e89b-12d3-a456-426614174000] hello"}]}

    with config_values(connect_tap_enabled=False):
        assert proxy.connect_tap_message_id(body) == ""
    with config_values(connect_tap_enabled=True):
        assert proxy.connect_tap_message_id(body) == "123e4567-e89b-12d3-a456-426614174000"
        assert proxy.connect_tap_message_id({"messages": [{"role": "user", "content": "hello [CSA#fake]"}]}) == ""


def test_connect_tap_requires_matching_claimed_ack():
    with temporary_path() as tmp_path:
        message_id = "123e4567-e89b-12d3-a456-426614174001"
        bridge_root = tmp_path / ".csa" / "connect" / "v1"
        (bridge_root / "ack").mkdir(parents=True)

        with config_values(connect_tap_enabled=True, connect_tap_workspace=str(tmp_path)):
            assert proxy.write_connect_tap_outbox(message_id, "reply") == "skipped_ack_missing"
            (bridge_root / "ack" / f"{message_id}.json").write_text(
                json.dumps({"messageId": "different", "status": "claimed"}),
                encoding="utf-8",
            )
            assert proxy.write_connect_tap_outbox(message_id, "reply") == "skipped_ack_mismatch"
            (bridge_root / "ack" / f"{message_id}.json").write_text(
                json.dumps({"messageId": message_id, "status": "queued"}),
                encoding="utf-8",
            )
            assert proxy.write_connect_tap_outbox(message_id, "reply") == "skipped_not_claimed"

        assert not (bridge_root / "outbox" / f"{message_id}.json").exists()


def test_connect_tap_accepts_active_delivery_ack_states():
    with temporary_path() as tmp_path:
        message_id = "123e4567-e89b-12d3-a456-426614174099"
        bridge_root = tmp_path / ".csa" / "connect" / "v1"
        ack_dir = bridge_root / "ack"
        ack_dir.mkdir(parents=True)

        with config_values(connect_tap_enabled=True, connect_tap_workspace=str(tmp_path)):
            for status in ("submitted", "delivery_unknown"):
                outbox = bridge_root / "outbox" / f"{message_id}.json"
                if outbox.exists():
                    outbox.unlink()
                (ack_dir / f"{message_id}.json").write_text(
                    json.dumps({"schemaVersion": 1, "messageId": message_id, "status": status}),
                    encoding="utf-8",
                )
                assert proxy.write_connect_tap_outbox(message_id, f"reply-{status}") == "written"


def test_connect_tap_atomically_writes_once_for_claimed_message():
    with temporary_path() as tmp_path:
        message_id = "123e4567-e89b-12d3-a456-426614174002"
        bridge_root = tmp_path / ".csa" / "connect" / "v1"
        ack_dir = bridge_root / "ack"
        ack_dir.mkdir(parents=True)
        (ack_dir / f"{message_id}.json").write_text(
            json.dumps({"schemaVersion": 1, "messageId": message_id, "status": "claimed"}),
            encoding="utf-8",
        )

        with config_values(connect_tap_enabled=True, connect_tap_workspace=str(tmp_path)):
            assert proxy.write_connect_tap_outbox(message_id, "first reply") == "written"
            assert proxy.write_connect_tap_outbox(message_id, "second reply") == "skipped_existing"

        outbox = bridge_root / "outbox" / f"{message_id}.json"
        payload = json.loads(outbox.read_text(encoding="utf-8"))
        assert payload["schemaVersion"] == 1
        assert payload["messageId"] == message_id
        assert payload["status"] == "replied"
        assert payload["text"] == "first reply"
        assert not list(outbox.parent.glob("*.tmp"))


def test_connect_tap_writes_ordered_progress_then_final_snapshot():
    with temporary_path() as tmp_path:
        message_id = "123e4567-e89b-12d3-a456-426614174004"
        bridge_root = tmp_path / ".csa" / "connect" / "v1"
        ack_dir = bridge_root / "ack"
        ack_dir.mkdir(parents=True)
        (ack_dir / f"{message_id}.json").write_text(
            json.dumps({"schemaVersion": 1, "messageId": message_id, "status": "claimed"}),
            encoding="utf-8",
        )

        with config_values(connect_tap_enabled=True, connect_tap_workspace=str(tmp_path)):
            assert proxy.write_connect_tap_progress(message_id, "first paragraph", 1) == "written"
            assert proxy.write_connect_tap_progress(message_id, "first paragraph\n\nsecond paragraph", 2) == "written"
            assert proxy.write_connect_tap_outbox(message_id, "complete response", 3) == "written"

        outbox = bridge_root / "outbox"
        progress_files = sorted(outbox.glob(f"{message_id}.*.progress.json"))
        assert len(progress_files) == 2
        assert json.loads(progress_files[0].read_text(encoding="utf-8"))["sequence"] == 1
        assert json.loads(progress_files[1].read_text(encoding="utf-8"))["sequence"] == 2
        final = json.loads((outbox / f"{message_id}.json").read_text(encoding="utf-8"))
        assert final["sequence"] == 3
        assert final["final"] is True
        assert final["text"] == "complete response"


def test_connect_tap_extracts_artifacts_and_hides_internal_markers():
    with temporary_path() as tmp_path:
        message_id = "123e4567-e89b-12d3-a456-426614174005"
        artifact_id = "54809de1-8d3d-4e54-87c2-d882159c2d69"
        bridge_root = tmp_path / ".csa" / "connect" / "v1"
        ack_dir = bridge_root / "ack"
        ack_dir.mkdir(parents=True)
        (ack_dir / f"{message_id}.json").write_text(
            json.dumps({"schemaVersion": 1, "messageId": message_id, "status": "claimed"}),
            encoding="utf-8",
        )

        marker = "{{artifact:" + artifact_id + "}}"
        response = f"图表已经生成。\n\n{marker}\n{marker}"
        with config_values(connect_tap_enabled=True, connect_tap_workspace=str(tmp_path)):
            assert proxy.write_connect_tap_outbox(message_id, response, 2) == "written"

        payload = json.loads((bridge_root / "outbox" / f"{message_id}.json").read_text(encoding="utf-8"))
        assert payload["text"] == "图表已经生成。"
        assert payload["artifactRefs"] == [artifact_id]
        assert "{{artifact:" not in payload["text"]


def test_connect_tap_artifact_only_reply_gets_visible_placeholder():
    artifact_id = "868ecfcf-7dc8-41d0-b773-4042d318b947"
    text, references = proxy.connect_reply_artifacts("{{artifact:" + artifact_id + "}}")
    assert text == "图片已生成。"
    assert references == [artifact_id]


def test_connect_tap_log_never_contains_response_text():
    with temporary_path() as tmp_path:
        message_id = "123e4567-e89b-12d3-a456-426614174003"
        ack_dir = tmp_path / ".csa" / "connect" / "v1" / "ack"
        ack_dir.mkdir(parents=True)
        (ack_dir / f"{message_id}.json").write_text(
            json.dumps({"messageId": message_id, "status": "claimed"}),
            encoding="utf-8",
        )
        old_entries = list(proxy.request_log)
        try:
            proxy.request_log.clear()
            with config_values(connect_tap_enabled=True, connect_tap_workspace=str(tmp_path)):
                proxy.complete_connect_taps(message_id, "", "private response body", stream=False)
            entry = proxy.request_log[-1]
            assert entry["status"] == "written"
            assert entry["response_chars"] == len("private response body")
            assert "private response body" not in json.dumps(entry)
            assert "response_text" not in entry
        finally:
            proxy.request_log[:] = old_entries


def test_tool_schema_root_type_is_object():
    body = {
        "model": "claude-sonnet-4-5",
        "max_tokens": 16,
        "tools": [
            {
                "name": "web_search",
                "description": "search",
                "input_schema": {
                    "type": None,
                    "properties": {
                        "query": {"type": ["string", "null"]},
                    },
                },
            }
        ],
        "messages": [{"role": "user", "content": "hi"}],
    }

    converted = proxy.anthropic_to_openai(body, "deepseek-chat")
    params = converted["tools"][0]["function"]["parameters"]

    assert params["type"] == "object"
    assert params["properties"]["query"]["type"] == "string"


def test_model_alias_routes_to_configured_backend_model_even_with_force_model():
    with config_values(
        default_backend="custom",
        custom_api_key="test-key",
        custom_base_url="https://api.siliconflow.cn",
        force_model="wrong-global-force-model",
        model_aliases=[{
            "id": "byok-model-0001",
            "display_name": "Kimi K2.6 Pro++",
            "backend": "custom",
            "model": "Pro/moonshotai/Kimi-K2.6",
        }],
    ):
        backend = proxy.config.resolve_backend("byok-model-0001")

    assert backend["backend"] == "custom"
    assert backend["model"] == "Pro/moonshotai/Kimi-K2.6"
    assert backend["base_url"] == "https://api.siliconflow.cn/v1"
    assert backend["mode"] == "openai"


def test_deepseek_native_anthropic_mode_normalizes_base_url():
    with config_values(
        default_backend="deepseek",
        deepseek_api_key="test-key",
        deepseek_base_url="https://api.deepseek.com",
        deepseek_upstream_mode="anthropic",
        force_model="deepseek-chat",
    ):
        backend = proxy.config.resolve_backend("claude-sonnet-4-5")

    assert backend["backend"] == "deepseek"
    assert backend["mode"] == "anthropic"
    assert backend["base_url"] == "https://api.deepseek.com/anthropic/v1"
    assert backend["model"] == "deepseek-chat"


def test_native_anthropic_auto_thinking_becomes_adaptive_without_mutating_input():
    body = {
        "model": "claude-opus-4-8",
        "max_tokens": 4096,
        "thinking": {"type": "auto", "budget_tokens": 2048},
        "messages": [{"role": "user", "content": "hi"}],
        "stream": True,
    }

    native = proxy.build_anthropic_backend_body(body, "deepseek-v4-pro")

    assert native["thinking"] == {"type": "adaptive"}
    assert native["stream"] is True
    assert body["thinking"] == {"type": "auto", "budget_tokens": 2048}


def test_native_anthropic_explicit_thinking_mode_is_preserved():
    body = {
        "model": "claude-opus-4-8",
        "thinking": {"type": "disabled"},
        "messages": [{"role": "user", "content": "hi"}],
    }

    native = proxy.build_anthropic_backend_body(body, "MiniMax-M3")

    assert native["thinking"] == {"type": "disabled"}


def test_deepseek_misspelled_legacy_model_repairs_only_the_typo():
    with config_values(
        default_backend="deepseek",
        deepseek_api_key="test-key",
        deepseek_base_url="https://api.deepseek.com/anthropic",
        deepseek_upstream_mode="anthropic",
        force_model="Deep-chat",
    ):
        backend = proxy.config.resolve_backend("claude-sonnet-5")

    assert backend["backend"] == "deepseek"
    assert backend["model"] == "deepseek-chat"


def test_deepseek_unrelated_model_is_not_silently_changed_to_a_paid_model():
    with config_values(
        default_backend="deepseek",
        deepseek_api_key="test-key",
        deepseek_base_url="https://api.deepseek.com/anthropic",
        deepseek_upstream_mode="anthropic",
        force_model="glm-5.2",
    ):
        backend = proxy.config.resolve_backend("claude-sonnet-5")

    assert backend["backend"] == "deepseek"
    assert backend["model"] == "glm-5.2"


def test_deepseek_current_official_v4_model_is_preserved():
    with config_values(
        default_backend="deepseek",
        deepseek_api_key="test-key",
        deepseek_base_url="https://api.deepseek.com/anthropic",
        deepseek_upstream_mode="anthropic",
        force_model="deepseek-v4-pro",
    ):
        backend = proxy.config.resolve_backend("claude-sonnet-5")

    assert backend["model"] == "deepseek-v4-pro"


def test_force_model_maps_claude_sonnet_5_to_active_custom_model():
    with config_values(
        default_backend="custom",
        custom_api_key="test-key",
        custom_base_url="https://10521052.xyz/v1",
        custom_upstream_mode="openai",
        force_model="grok-4.20-fast",
        model_aliases=[],
    ):
        backend = proxy.config.resolve_backend("claude-sonnet-5")

    assert backend["backend"] == "custom"
    assert backend["mode"] == "openai"
    assert backend["base_url"] == "https://10521052.xyz/v1"
    assert backend["model"] == "grok-4.20-fast"


def test_force_model_keeps_active_backend_even_for_gpt_or_deepseek_names():
    with config_values(
        default_backend="custom",
        custom_api_key="test-key",
        custom_base_url="https://10521052.xyz/v1",
        custom_upstream_mode="openai",
        openai_api_key="",
        deepseek_api_key="",
        force_model="grok-4.20-fast",
        model_aliases=[],
    ):
        gpt_backend = proxy.config.resolve_backend("gpt-4o")
        deepseek_backend = proxy.config.resolve_backend("deepseek-chat")

    assert gpt_backend["backend"] == "custom"
    assert gpt_backend["model"] == "grok-4.20-fast"
    assert deepseek_backend["backend"] == "custom"
    assert deepseek_backend["model"] == "grok-4.20-fast"


def test_max_tokens_cap_applies_to_openai_translation_and_native_body():
    body = {
        "model": "byok-model-0001",
        "max_tokens": 100000,
        "messages": [{"role": "user", "content": "hi"}],
    }
    with config_values(model_token_caps={"Pro/moonshotai/Kimi-K2.6": 8192}, default_max_tokens_cap=0):
        converted = proxy.anthropic_to_openai(
            body,
            "Pro/moonshotai/Kimi-K2.6",
            "custom",
            "https://api.siliconflow.cn/v1",
        )
        native = proxy.build_anthropic_backend_body(body, "Pro/moonshotai/Kimi-K2.6")

    assert converted["max_tokens"] == 8192
    assert native["max_tokens"] == 8192


def test_openai_translation_uses_upstream_default_when_caller_omits_max_tokens():
    body = {
        "model": "byok-model-0001",
        "messages": [{"role": "user", "content": "hi"}],
    }
    converted = proxy.anthropic_to_openai(body, "glm-5.2", "custom", "https://example.com/v1")
    assert "max_tokens" not in converted


def test_models_endpoint_can_return_only_third_party_aliases():
    client = TestClient(proxy.app)
    with config_values(
        model_list_mode="aliases",
        model_aliases=[{
            "id": "byok-model-0001",
            "display_name": "Kimi K2.6 Pro++",
            "backend": "custom",
            "model": "Pro/moonshotai/Kimi-K2.6",
        }],
    ):
        response = client.get("/v1/models")

    assert response.status_code == 200
    data = response.json()["data"]
    assert data == [{
        "id": "byok-model-0001",
        "type": "model",
        "display_name": "Kimi K2.6 Pro++",
    }]


def test_models_endpoint_adds_claude_role_aliases_when_force_model_is_active():
    client = TestClient(proxy.app)
    with config_values(
        default_backend="custom",
        force_model="grok-4.20-fast",
        model_list_mode="aliases",
        model_aliases=[{
            "id": "byok-model-0001",
            "display_name": "内置中转 · grok-4.20-fast",
            "backend": "custom",
            "model": "grok-4.20-fast",
        }],
    ):
        response = client.get("/v1/models")

    assert response.status_code == 200
    ids = [item["id"] for item in response.json()["data"]]
    assert "byok-model-0001" in ids
    assert "claude-sonnet-5" in ids
    assert "claude-opus-4-8" in ids
    assert "claude-haiku-4-5-20251001" in ids


def test_provider_presets_include_protocol_modes():
    client = TestClient(proxy.app)
    response = client.get("/api/provider-presets")

    assert response.status_code == 200
    presets = response.json()["presets"]
    assert presets["siliconflow_kimi"]["upstream_mode"] == "openai"
    assert presets["deepseek_anthropic"]["upstream_mode"] == "anthropic"
    assert all(preset["default_model"] == "" for preset in presets.values())
    assert all(preset["model_aliases"] == [] for preset in presets.values())


def test_models_endpoint_supports_intentional_empty_state():
    client = TestClient(proxy.app)
    with config_values(force_model="", model_aliases=[], model_list_mode="aliases"):
        response = client.get("/v1/models")

    assert response.status_code == 200
    assert response.json() == {
        "data": [],
        "has_more": False,
        "first_id": None,
        "last_id": None,
    }


def test_packaged_example_config_is_secret_free_empty_state():
    cfg = json.loads((ROOT / "config.example.json").read_text(encoding="utf-8"))

    assert cfg["deepseek_api_key"] == ""
    assert cfg["openai_api_key"] == ""
    assert cfg["custom_api_key"] == ""
    assert cfg["default_backend"] == ""
    assert cfg["force_model"] == ""
    assert cfg["model_aliases"] == []
    assert cfg["model_list_mode"] == "aliases"
    assert proxy.Config.DEFAULTS["default_backend"] == ""


def test_minimax_china_anthropic_base_url_is_normalized_without_duplication():
    assert (
        proxy.normalize_anthropic_base_url("https://api.minimaxi.com/anthropic")
        == "https://api.minimaxi.com/anthropic/v1"
    )
    assert (
        proxy.normalize_anthropic_base_url("https://api.minimaxi.com/anthropic/v1")
        == "https://api.minimaxi.com/anthropic/v1"
    )


def test_outbound_proxy_configures_httpx_client_kwargs():
    with config_values(outbound_proxy_url="http://127.0.0.1:7890"):
        kwargs = proxy.httpx_client_kwargs(timeout=10.0)

    assert kwargs["proxy"] == "http://127.0.0.1:7890"
    assert kwargs["trust_env"] is False


def test_required_path_secret_protects_v1_and_does_not_log_secret():
    client = TestClient(proxy.app)
    proxy.request_log.clear()
    with config_values(proxy_auth_token="secret-test-token", proxy_auth_mode="required"):
        denied = client.get("/v1/models")
        allowed = client.get("/secret-test-token/v1/models")

    assert denied.status_code == 403
    assert denied.headers.get("connection") == "close"
    assert allowed.status_code == 200
    logs = json.dumps(proxy.request_log, ensure_ascii=False)
    assert "secret-test-token" not in logs
    assert "/v1/models" in logs


def test_management_api_rejects_untrusted_browser_origin():
    client = TestClient(proxy.app)
    response = client.get("/api/config", headers={"Origin": "https://attacker.invalid"})

    assert response.status_code == 403
    assert response.json()["error"] == "untrusted browser origin"
    assert "access-control-allow-origin" not in response.headers


def test_required_mode_protects_management_api_with_control_header():
    client = TestClient(proxy.app)
    with config_values(proxy_auth_token="secret-test-token", proxy_auth_mode="required"):
        denied = client.get("/api/config")
        allowed = client.get(
            "/api/config",
            headers={"X-Proxy-Control-Token": "secret-test-token"},
        )

    assert denied.status_code == 403
    assert allowed.status_code == 200


def test_dashboard_request_log_uses_text_nodes_instead_of_html_templates():
    dashboard = (ROOT / "static" / "dashboard.html").read_text(encoding="utf-8")

    assert "list.innerHTML = res.requests.map" not in dashboard
    assert "model.textContent" in dashboard


def test_siliconflow_forced_tool_choice_is_downgraded_to_auto():
    body = {
        "model": "claude-sonnet-4-5",
        "max_tokens": 16,
        "tools": [
            {
                "name": "python",
                "description": "run python",
                "input_schema": {"type": "object", "properties": {"code": {"type": "string"}}},
            }
        ],
        "tool_choice": {"type": "tool", "name": "python"},
        "messages": [{"role": "user", "content": "use python"}],
    }

    converted = proxy.anthropic_to_openai(
        body,
        "Pro/moonshotai/Kimi-K2.6",
        "custom",
        "https://api.siliconflow.cn/v1",
    )

    assert converted["tool_choice"] == "auto"


def test_openai_forced_tool_choice_keeps_function_choice():
    body = {
        "model": "claude-sonnet-4-5",
        "max_tokens": 16,
        "tools": [
            {
                "name": "python",
                "description": "run python",
                "input_schema": {"type": "object", "properties": {"code": {"type": "string"}}},
            }
        ],
        "tool_choice": {"type": "tool", "name": "python"},
        "messages": [{"role": "user", "content": "use python"}],
    }

    converted = proxy.anthropic_to_openai(body, "gpt-4o", "openai", "https://api.openai.com/v1")

    assert converted["tool_choice"] == {"type": "function", "function": {"name": "python"}}


def test_parallel_tool_preference_is_preserved():
    body = {
        "model": "claude-sonnet-5",
        "max_tokens": 256,
        "tools": [{
            "name": "lookup",
            "input_schema": {"type": "object", "properties": {}},
        }],
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": True},
        "messages": [{"role": "user", "content": "look it up"}],
    }

    converted = proxy.anthropic_to_openai(body, "glm-5.2", "custom", "https://example.com/v1")

    assert converted["parallel_tool_calls"] is False


def test_o_series_uses_max_completion_tokens():
    body = {
        "model": "claude-sonnet-5",
        "max_tokens": 4096,
        "messages": [{"role": "user", "content": "hi"}],
    }

    converted = proxy.anthropic_to_openai(body, "openai/o3-mini")

    assert converted["max_completion_tokens"] == 4096
    assert "max_tokens" not in converted


def test_glm_explicit_thinking_is_mapped_without_implicit_injection():
    explicit = {
        "model": "claude-sonnet-5",
        "max_tokens": 4096,
        "thinking": {"type": "adaptive"},
        "output_config": {"effort": "high"},
        "messages": [{"role": "user", "content": "hi"}],
    }
    ordinary = {
        "model": "claude-sonnet-5",
        "max_tokens": 4096,
        "messages": [{"role": "user", "content": "hi"}],
    }

    explicit_result = proxy.anthropic_to_openai(
        explicit, "glm-5.2", "custom", "https://opencode.ai/zen/go/v1"
    )
    ordinary_result = proxy.anthropic_to_openai(
        ordinary, "glm-5.2", "custom", "https://opencode.ai/zen/go/v1"
    )

    assert explicit_result["thinking"] == {"type": "enabled"}
    assert "thinking" not in ordinary_result


def test_gpt5_explicit_effort_is_mapped_and_max_becomes_xhigh():
    body = {
        "model": "claude-opus-4-8",
        "max_tokens": 4096,
        "thinking": {"type": "adaptive"},
        "output_config": {"effort": "max"},
        "messages": [{"role": "user", "content": "hi"}],
    }

    converted = proxy.anthropic_to_openai(body, "gpt-5.5")

    assert converted["reasoning_effort"] == "xhigh"


def test_openrouter_platform_rule_wins_over_glm_model_rule():
    body = {
        "model": "claude-sonnet-5",
        "max_tokens": 4096,
        "thinking": {"type": "enabled", "budget_tokens": 8000},
        "messages": [{"role": "user", "content": "hi"}],
    }

    converted = proxy.anthropic_to_openai(
        body, "z-ai/glm-5.2", "custom", "https://openrouter.ai/api/v1"
    )

    assert converted["reasoning"] == {"effort": "medium"}
    assert "thinking" not in converted


def test_parameter_error_can_switch_to_max_completion_tokens_once():
    body = {"model": "o3", "messages": [], "max_tokens": 4096}
    retry = proxy.adapt_openai_body_after_error(
        body,
        400,
        "max_tokens is not supported for this model; use max_completion_tokens instead",
    )

    assert retry is not None
    adapted, reason = retry
    assert adapted["max_completion_tokens"] == 4096
    assert "max_tokens" not in adapted
    assert reason == "max_tokens->max_completion_tokens"


def test_provider_advertised_output_limit_is_used_for_retry():
    body = {"model": "example", "messages": [], "max_tokens": 32768}
    retry = proxy.adapt_openai_body_after_error(
        body,
        400,
        "max_tokens must be less than or equal to 8192",
    )

    assert retry is not None
    adapted, reason = retry
    assert adapted["max_tokens"] == 8192
    assert reason == "max_tokens-clamped-to-provider-limit"


def test_unknown_output_limit_falls_back_to_provider_default_without_global_cap():
    body = {"model": "example", "messages": [], "max_tokens": 32768}
    retry = proxy.adapt_openai_body_after_error(body, 422, "invalid max_tokens for selected model")

    assert retry is not None
    adapted, reason = retry
    assert "max_tokens" not in adapted
    assert reason == "max_tokens-omitted-for-provider-default"


def test_unrelated_or_auth_errors_are_not_retried():
    body = {"model": "example", "messages": [], "max_tokens": 32768}

    assert proxy.adapt_openai_body_after_error(body, 401, "invalid api key") is None
    assert proxy.adapt_openai_body_after_error(body, 400, "invalid messages array") is None


def test_tool_results_follow_assistant_tool_calls_immediately():
    body = {
        "model": "claude-sonnet-4-5",
        "max_tokens": 16,
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_123",
                        "name": "web_search",
                        "input": {"query": "test"},
                    }
                ],
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_123",
                        "content": "result",
                    },
                    {"type": "text", "text": "continue"},
                ],
            },
        ],
    }

    converted = proxy.anthropic_to_openai(body, "deepseek-chat")
    messages = converted["messages"]

    assert messages[0]["role"] == "assistant"
    assert messages[0]["tool_calls"][0]["id"] == "toolu_123"
    assert messages[1] == {"role": "tool", "tool_call_id": "toolu_123", "content": "result"}
    assert messages[2] == {"role": "user", "content": "continue"}


def test_siliconflow_custom_preserves_inline_base64_images():
    body = {
        "model": "claude-sonnet-4-5",
        "max_tokens": 16,
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "abc",
                        },
                    },
                ],
            }
        ],
    }

    with image_policy("auto"):
        converted = proxy.anthropic_to_openai(
            body,
            "Pro/moonshotai/Kimi-K2.6",
            "custom",
            "https://api.siliconflow.cn/v1",
        )

    content = converted["messages"][0]["content"]
    assert isinstance(content, list)
    assert content[0] == {"type": "text", "text": "describe"}
    assert content[1]["type"] == "image_url"
    assert content[1]["image_url"]["url"].startswith("data:image/png;base64,")


def test_deepseek_omits_images_for_text_only_backend():
    body = {
        "model": "claude-sonnet-4-5",
        "max_tokens": 16,
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "abc",
                        },
                    },
                ],
            }
        ],
    }

    with image_policy("auto"):
        converted = proxy.anthropic_to_openai(body, "deepseek-chat", "deepseek", "https://api.deepseek.com/v1")
    content = converted["messages"][0]["content"]

    assert isinstance(content, str)
    assert "describe" in content
    assert "omitted" in content


def test_explicit_preserve_keeps_images_for_vision_backends():
    body = {
        "model": "claude-sonnet-4-5",
        "max_tokens": 16,
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "describe"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/jpeg",
                            "data": "abc",
                        },
                    },
                ],
            }
        ],
    }

    with image_policy("preserve"):
        converted = proxy.anthropic_to_openai(body, "vision-model", "custom", "https://provider.example.com/v1")

    content = converted["messages"][0]["content"]
    assert content[1]["type"] == "image_url"
    assert content[1]["image_url"]["url"].startswith("data:image/jpeg;base64,")


def test_reasoning_content_is_hidden_when_policy_is_never():
    response = {
        "choices": [{
            "message": {
                "content": "",
                "reasoning_content": "The user asked to continue. I should inspect files first.",
            },
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 20},
    }

    with reasoning_policy("never"):
        converted = proxy.openai_to_anthropic_response(response, "claude-sonnet-4-5", "msg_reason")

    assert converted["content"] == []


def test_trace_preamble_before_tool_call_is_hidden():
    trace = (
        'The user said "继续" (continue). The session was resumed, which means my Python kernel was reset. '
        "Files on disk are intact. Let me check what files I have and continue with the GO/KEGG enrichment analysis.\n\n"
        "I have gseapy installed now. Let me:\n"
        "1. First check what files are available\n"
        "2. Run the GO/KEGG enrichment analysis using gseapy\n"
        "3. Create the enrichment plots\n\n"
        "用户要求继续分析。会话已恢复，Python内核已重置但文件仍在。让我检查文件并继续GO/KEGG富集分析。"
    )
    response = {
        "choices": [{
            "message": {
                "content": trace,
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": {"name": "python", "arguments": "{\"code\":\"print(1)\"}"},
                }],
            },
            "finish_reason": "tool_calls",
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 20},
    }

    converted = proxy.openai_to_anthropic_response(
        response,
        "claude-sonnet-4-5",
        "msg_trace",
        {"python": "python"},
    )

    assert len(converted["content"]) == 1
    assert converted["content"][0]["type"] == "tool_use"
    assert "The user said" not in json.dumps(converted, ensure_ascii=False)
    assert "用户要求继续分析" not in json.dumps(converted, ensure_ascii=False)


def test_kimi_embedded_tool_call_text_becomes_tool_use_block():
    args = {
        "human_description": "Building PPI network and calculating topology",
        "code": "import networkx as nx\nG = nx.Graph()\nprint(G.number_of_nodes())",
    }
    leaked = (
        "现在让我用networkx构建网络。"
        "<|tool_calls_section_begin|>"
        "<|tool_call_begin|>functions.python:47"
        "<|tool_call_argument_begin|>"
        f"{json.dumps(args, ensure_ascii=False)}"
        "<|tool_call_end|><|tool_calls_section_end|>"
    )
    response = {
        "choices": [{"message": {"content": leaked}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 20},
    }

    converted = proxy.openai_to_anthropic_response(
        response,
        "claude-sonnet-4-5",
        "msg_test",
        {"python": "python", "functions.python": "python"},
    )

    assert converted["stop_reason"] == "tool_use"
    assert len(converted["content"]) == 1
    tool_block = converted["content"][0]
    assert tool_block["type"] == "tool_use"
    assert tool_block["name"] == "python"
    assert tool_block["input"]["human_description"] == args["human_description"]
    assert "networkx" in tool_block["input"]["code"]
    assert "现在让我" not in json.dumps(converted, ensure_ascii=False)
    assert "<|tool_call" not in json.dumps(converted, ensure_ascii=False)


def _stream_payloads(events):
    payloads = []
    for event in events:
        for line in event.splitlines():
            if line.startswith("data: "):
                payloads.append(json.loads(line[6:]))
    return payloads


class _FakeOpenAIStream:
    def __init__(self, chunks):
        self.chunks = chunks

    async def aiter_lines(self):
        for chunk in self.chunks:
            yield "data: " + json.dumps(chunk, ensure_ascii=False)
        yield "data: [DONE]"


class _PausingOpenAIStream:
    def __init__(self, first_chunk, later_chunks):
        self.first_chunk = first_chunk
        self.later_chunks = later_chunks
        self.release = asyncio.Event()

    async def aiter_lines(self):
        yield "data: " + json.dumps(self.first_chunk, ensure_ascii=False)
        await self.release.wait()
        for chunk in self.later_chunks:
            yield "data: " + json.dumps(chunk, ensure_ascii=False)
        yield "data: [DONE]"


def test_streaming_kimi_embedded_tool_call_is_not_emitted_as_text():
    args = {"human_description": "run code", "code": "print('ok')"}
    chunks = [
        {"choices": [{"delta": {"content": "现在让我用networkx构建网络、计算拓扑参数。"}}]},
        {"choices": [{"delta": {"content": "<|tool_calls_section"}}]},
        {"choices": [{"delta": {"content": "_begin|><|tool_call_begin|>functions.python:47<|tool_call_argument_begin|>"}}]},
        {"choices": [{"delta": {"content": json.dumps(args, ensure_ascii=False) + "<|tool_call_end|><|tool_calls_section_end|>"}}]},
        {"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"completion_tokens": 30}},
    ]

    async def collect():
        return [
            event
            async for event in proxy.translate_stream(
                _FakeOpenAIStream(chunks),
                "claude-sonnet-4-5",
                "msg_stream",
                {"python": "python", "functions.python": "python"},
            )
        ]

    events = asyncio.run(collect())
    joined = "".join(events)
    payloads = _stream_payloads(events)

    assert "<|tool_call" not in joined
    assert "现在让我" not in joined
    assert any(p.get("delta", {}).get("stop_reason") == "tool_use" for p in payloads)
    tool_starts = [
        p for p in payloads
        if p.get("type") == "content_block_start"
        and p.get("content_block", {}).get("type") == "tool_use"
    ]
    assert tool_starts
    assert tool_starts[0]["content_block"]["name"] == "python"
    assert any("print('ok')" in p.get("delta", {}).get("partial_json", "") for p in payloads)


def test_streaming_standard_tool_call_uses_zero_based_index_without_text():
    chunks = [
        {
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_123",
                        "function": {"name": "python", "arguments": "{\"code\":\"print(1)\"}"},
                    }]
                }
            }]
        },
        {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]},
    ]

    async def collect():
        return [
            event
            async for event in proxy.translate_stream(
                _FakeOpenAIStream(chunks),
                "claude-sonnet-4-5",
                "msg_tool",
                {"python": "python"},
            )
        ]

    payloads = _stream_payloads(asyncio.run(collect()))
    starts = [p for p in payloads if p.get("type") == "content_block_start"]

    assert starts[0]["index"] == 0
    assert starts[0]["content_block"]["type"] == "tool_use"
    assert starts[0]["content_block"]["name"] == "python"


def test_streaming_text_block_stops_before_standard_tool_block_starts():
    chunks = [
        {"choices": [{"delta": {"content": "I will use python."}}]},
        {
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_123",
                        "function": {"name": "python", "arguments": "{\"code\":\"print(1)\"}"},
                    }]
                }
            }]
        },
        {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]},
    ]

    async def collect():
        return [
            event
            async for event in proxy.translate_stream(
                _FakeOpenAIStream(chunks),
                "claude-sonnet-4-5",
                "msg_tool_order",
                {"python": "python"},
            )
        ]

    payloads = _stream_payloads(asyncio.run(collect()))
    text_stop_pos = next(
        i for i, p in enumerate(payloads)
        if p.get("type") == "content_block_stop" and p.get("index") == 0
    )
    tool_start_pos = next(
        i for i, p in enumerate(payloads)
        if p.get("type") == "content_block_start"
        and p.get("content_block", {}).get("type") == "tool_use"
    )

    assert text_stop_pos < tool_start_pos


def test_streaming_trace_preamble_before_tool_call_is_hidden():
    trace = (
        'The user said "继续" (continue). The session was resumed, which means my Python kernel was reset. '
        "Files on disk are intact. Let me check what files I have and continue with the GO/KEGG enrichment analysis.\n"
        "1. First check what files are available\n"
        "2. Run the GO/KEGG enrichment analysis using gseapy\n"
        "用户要求继续分析。会话已恢复，Python内核已重置但文件仍在。让我检查文件并继续GO/KEGG富集分析。"
    )
    chunks = [
        {"choices": [{"delta": {"content": trace[:120]}}]},
        {"choices": [{"delta": {"content": trace[120:]}}]},
        {
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_123",
                        "function": {"name": "python", "arguments": "{\"code\":\"print(1)\"}"},
                    }]
                }
            }]
        },
        {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]},
    ]

    async def collect():
        return [
            event
            async for event in proxy.translate_stream(
                _FakeOpenAIStream(chunks),
                "claude-sonnet-4-5",
                "msg_trace_stream",
                {"python": "python"},
            )
        ]

    events = asyncio.run(collect())
    joined = "".join(events)
    payloads = _stream_payloads(events)
    starts = [p for p in payloads if p.get("type") == "content_block_start"]

    assert "The user said" not in joined
    assert "用户要求继续分析" not in joined
    assert starts[0]["content_block"]["type"] == "tool_use"


def test_streaming_normal_answer_with_tools_available_is_delivered_on_finish():
    chunks = [
        {"choices": [{"delta": {"content": "GO enrichment finished."}}]},
        {"choices": [{"delta": {}, "finish_reason": "stop"}]},
    ]

    async def collect():
        return [
            event
            async for event in proxy.translate_stream(
                _FakeOpenAIStream(chunks),
                "claude-sonnet-4-5",
                "msg_normal_stream",
                {"python": "python"},
            )
        ]

    joined = "".join(asyncio.run(collect()))

    assert "GO enrichment finished." in joined


def test_streaming_normal_text_with_tools_available_flushes_before_finish():
    first_chunk = {
        "choices": [{
            "delta": {
                "content": (
                    "GO enrichment finished successfully. The top biological processes "
                    "are inflammatory response, apoptosis, and oxidative stress regulation."
                )
            }
        }]
    }
    later_chunks = [{"choices": [{"delta": {}, "finish_reason": "stop"}]}]

    async def collect_prefix():
        stream = _PausingOpenAIStream(first_chunk, later_chunks)
        agen = proxy.translate_stream(
            stream,
            "claude-sonnet-4-5",
            "msg_no_stall",
            {"python": "python"},
        )
        first = await asyncio.wait_for(agen.__anext__(), timeout=1)
        second = await asyncio.wait_for(agen.__anext__(), timeout=1)
        stream.release.set()
        rest = []
        async for event in agen:
            rest.append(event)
        return [first, second, *rest]

    events = asyncio.run(collect_prefix())
    payloads = _stream_payloads(events[:2])

    assert payloads[0]["type"] == "message_start"
    assert payloads[1]["type"] == "content_block_start"


def test_streaming_heartbeat_emits_ping_after_message_start_idle():
    async def idle_events():
        yield proxy.sse_event("message_start", {"type": "message_start"})
        await asyncio.Event().wait()

    async def collect():
        agen = proxy.stream_events_with_heartbeat(idle_events(), interval=0.01)
        first = await asyncio.wait_for(agen.__anext__(), timeout=1)
        second = await asyncio.wait_for(agen.__anext__(), timeout=1)
        await agen.aclose()
        return [first, second]

    payloads = _stream_payloads(asyncio.run(collect()))

    assert payloads[0]["type"] == "message_start"
    assert payloads[1]["type"] == "ping"


def test_invalid_json_returns_400_without_exception():
    client = TestClient(proxy.app)
    response = client.post("/v1/messages", data="", headers={"Content-Type": "application/json"})

    assert response.status_code == 400
    assert response.json()["error"]["type"] == "invalid_request_error"


def test_oauth_profile_mock_matches_claude_science_shape():
    client = TestClient(proxy.app)
    response = client.get("/api/oauth/profile")

    assert response.status_code == 200
    data = response.json()
    assert data["account"]["uuid"] == proxy.FAKE_ACCOUNT_UUID
    assert data["account"]["email_address"] == "byok@localhost"
    assert data["organization"]["uuid"] == proxy.FAKE_ORG_UUID
    assert data["organization"]["organization_type"] == "claude_max"
    assert isinstance(data["enabled_plugins"], list)


def test_oauth_token_mock_uses_claude_ai_provider_and_scopes():
    client = TestClient(proxy.app)
    response = client.post("/api/oauth/token")

    assert response.status_code == 200
    data = response.json()
    assert data["provider"] == "claude_ai"
    for scope in ["user:inference", "user:profile", "user:mcp_servers", "user:plugins"]:
        assert scope in data["scope"].split()
