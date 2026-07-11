#!/usr/bin/env python3
r"""
Local proxy that lets Claude Science use DeepSeek and ChatGPT APIs.

Features:
  - Anthropic ↔ OpenAI format translation (streaming + non-streaming)
  - Model-based routing to DeepSeek / OpenAI
  - Fake OAuth token generation
  - Web management dashboard at http://127.0.0.1:9876/dashboard
  - Persistent config via ~/.claude-science/proxy/config.json
  - Request logging and health monitoring

Quick start:
  .\scripts\install-safe.ps1
  Then open http://127.0.0.1:9876/dashboard
"""

from __future__ import annotations

import asyncio
import hmac
from io import BytesIO
import json
import os
import re
import base64
import subprocess
import sys
import tempfile
import uuid
from datetime import datetime
from pathlib import Path
from typing import Optional
from urllib.parse import urlsplit, urlunsplit

import httpx
from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse, JSONResponse, FileResponse
from fastapi.staticfiles import StaticFiles
from starlette.middleware.base import BaseHTTPMiddleware

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
APP_DIR = Path(__file__).resolve().parent
DEFAULT_PROXY_DIR = Path.home() / ".claude-science" / "proxy"
PROXY_DIR = Path(os.environ.get("CLAUDE_SCIENCE_PROXY_DIR", str(DEFAULT_PROXY_DIR))).expanduser()
CONFIG_FILE = PROXY_DIR / "config.json"
STATIC_DIR = Path(os.environ.get("CLAUDE_SCIENCE_STATIC_DIR", str(APP_DIR / "static"))).expanduser()
TOKEN_DIR = Path.home() / ".claude-science" / ".oauth-tokens"
ENC_KEY_FILE = Path.home() / ".claude-science" / "encryption.key"


# ---------------------------------------------------------------------------
# Config persistence
# ---------------------------------------------------------------------------
class Config:
    """Persistent config backed by config.json."""

    DEFAULTS = {
        "deepseek_api_key": "",
        "openai_api_key": "",
        "custom_api_key": "",
        "deepseek_base_url": "https://api.deepseek.com/anthropic",
        "openai_base_url": "https://api.openai.com",
        "custom_base_url": "",
        "default_backend": "",
        "force_model": "",
        "deepseek_model_map": {},
        "openai_model_map": {},
        "custom_model_map": {},
        "model_aliases": [],
        "model_list_mode": "aliases",
        "model_token_caps": {},
        "default_max_tokens_cap": 0,
        "deepseek_upstream_mode": "anthropic",
        "openai_upstream_mode": "openai",
        "custom_upstream_mode": "openai",
        "proxy_auth_token": "",
        "proxy_auth_mode": "optional",
        "outbound_proxy_url": "",
        "deepseek_model_pattern": r"deepseek|deep-seek",
        "openai_model_pattern": r"^(gpt-|o1|o3|o4|chatgpt)",
        "custom_model_pattern": "",
        "reasoning_content_policy": "never",
        "inline_image_policy": "auto",
        "proxy_host": "127.0.0.1",
        "proxy_port": 9876,
    }

    ENV_KEYS = {
        "deepseek_api_key": "DEEPSEEK_API_KEY",
        "openai_api_key": "OPENAI_API_KEY",
        "custom_api_key": "CUSTOM_API_KEY",
        "deepseek_base_url": "DEEPSEEK_BASE_URL",
        "openai_base_url": "OPENAI_BASE_URL",
        "custom_base_url": "CUSTOM_BASE_URL",
        "default_backend": "DEFAULT_BACKEND",
        "force_model": "FORCE_MODEL",
        "deepseek_model_map": "DEEPSEEK_MODEL_MAP",
        "openai_model_map": "OPENAI_MODEL_MAP",
        "custom_model_map": "CUSTOM_MODEL_MAP",
        "model_aliases": "MODEL_ALIASES",
        "model_list_mode": "MODEL_LIST_MODE",
        "model_token_caps": "MODEL_TOKEN_CAPS",
        "default_max_tokens_cap": "DEFAULT_MAX_TOKENS_CAP",
        "deepseek_upstream_mode": "DEEPSEEK_UPSTREAM_MODE",
        "openai_upstream_mode": "OPENAI_UPSTREAM_MODE",
        "custom_upstream_mode": "CUSTOM_UPSTREAM_MODE",
        "proxy_auth_token": "PROXY_AUTH_TOKEN",
        "proxy_auth_mode": "PROXY_AUTH_MODE",
        "outbound_proxy_url": "OUTBOUND_PROXY_URL",
        "deepseek_model_pattern": "DEEPSEEK_MODEL_PATTERN",
        "openai_model_pattern": "OPENAI_MODEL_PATTERN",
        "custom_model_pattern": "CUSTOM_MODEL_PATTERN",
        "reasoning_content_policy": "REASONING_CONTENT_POLICY",
        "inline_image_policy": "INLINE_IMAGE_POLICY",
        "proxy_host": "PROXY_HOST",
        "proxy_port": "PROXY_PORT",
    }
    JSON_KEYS = {"deepseek_model_map", "openai_model_map", "custom_model_map", "model_aliases", "model_token_caps"}

    def __init__(self):
        self._data = dict(self.DEFAULTS)
        self._load()
        self._load_env()

    def _load(self):
        if CONFIG_FILE.exists():
            try:
                with open(CONFIG_FILE) as f:
                    stored = json.load(f)
                self._data.update(stored)
            except Exception:
                pass

    def _load_env(self):
        for key, env_key in self.ENV_KEYS.items():
            value = os.environ.get(env_key)
            if value in (None, ""):
                continue
            try:
                if key in self.JSON_KEYS:
                    value = json.loads(value)
                elif key in {"proxy_port", "default_max_tokens_cap"}:
                    value = int(value)
            except Exception:
                continue
            self._data[key] = value

    def save(self):
        PROXY_DIR.mkdir(parents=True, exist_ok=True)
        temp_path = None
        try:
            with tempfile.NamedTemporaryFile(
                "w",
                encoding="utf-8",
                dir=CONFIG_FILE.parent,
                prefix=f".{CONFIG_FILE.name}.",
                suffix=".tmp",
                delete=False,
            ) as f:
                temp_path = Path(f.name)
                json.dump(self._data, f, indent=2, ensure_ascii=False)
                f.write("\n")
                f.flush()
                os.fsync(f.fileno())
            os.chmod(temp_path, 0o600)
            os.replace(temp_path, CONFIG_FILE)
            os.chmod(CONFIG_FILE, 0o600)
        finally:
            if temp_path and temp_path.exists():
                temp_path.unlink(missing_ok=True)

    def get(self, key: str, default=None):
        return self._data.get(key, default)

    def update(self, d: dict):
        self._data.update(d)
        self.save()

    def public_dict(self) -> dict:
        """Return config with API keys masked."""
        d = dict(self._data)
        for k in ("deepseek_api_key", "openai_api_key", "custom_api_key"):
            val = d.get(k, "")
            if val and len(val) > 8:
                d[k] = val[:4] + "•" * (len(val) - 8) + val[-4:]
        val = d.get("proxy_auth_token", "")
        if val and len(val) > 8:
            d["proxy_auth_token"] = val[:4] + "•" * (len(val) - 8) + val[-4:]
        return d

    @property
    def deepseek_api_key(self) -> str: return self._data["deepseek_api_key"]
    @property
    def openai_api_key(self) -> str: return self._data["openai_api_key"]
    @property
    def custom_api_key(self) -> str: return self._data["custom_api_key"]
    @property
    def deepseek_base_url(self) -> str: return self._data["deepseek_base_url"]
    @property
    def openai_base_url(self) -> str: return self._data["openai_base_url"]
    @property
    def custom_base_url(self) -> str: return self._data["custom_base_url"]
    @property
    def default_backend(self) -> str: return self._data["default_backend"]
    @property
    def force_model(self) -> str: return self._data["force_model"]
    @property
    def deepseek_model_map(self) -> dict: return self._data["deepseek_model_map"]
    @property
    def openai_model_map(self) -> dict: return self._data["openai_model_map"]
    @property
    def custom_model_map(self) -> dict: return self._data["custom_model_map"]
    @property
    def model_aliases(self) -> list: return self._data["model_aliases"]
    @property
    def model_list_mode(self) -> str: return self._data["model_list_mode"]
    @property
    def model_token_caps(self) -> dict: return self._data["model_token_caps"]
    @property
    def default_max_tokens_cap(self) -> int: return int(self._data.get("default_max_tokens_cap") or 0)
    @property
    def deepseek_upstream_mode(self) -> str: return self._data["deepseek_upstream_mode"]
    @property
    def openai_upstream_mode(self) -> str: return self._data["openai_upstream_mode"]
    @property
    def custom_upstream_mode(self) -> str: return self._data["custom_upstream_mode"]
    @property
    def proxy_auth_token(self) -> str: return self._data["proxy_auth_token"]
    @property
    def proxy_auth_mode(self) -> str: return self._data["proxy_auth_mode"]
    @property
    def outbound_proxy_url(self) -> str: return self._data["outbound_proxy_url"]
    @property
    def deepseek_model_pattern(self) -> str: return self._data["deepseek_model_pattern"]
    @property
    def openai_model_pattern(self) -> str: return self._data["openai_model_pattern"]
    @property
    def custom_model_pattern(self) -> str: return self._data["custom_model_pattern"]
    @property
    def reasoning_content_policy(self) -> str: return self._data["reasoning_content_policy"]
    @property
    def inline_image_policy(self) -> str: return self._data["inline_image_policy"]
    @property
    def proxy_host(self) -> str: return self._data["proxy_host"]
    @property
    def proxy_port(self) -> int: return self._data["proxy_port"]

    def resolve_backend(self, model: str) -> dict:
        """Determine which backend to use and what model name to send."""
        alias = self.get_model_alias(model)
        backend = self.default_backend
        alias_model = ""
        if alias:
            backend = (alias.get("backend") or backend or "").lower()
            alias_model = str(alias.get("model") or model).strip()
        try:
            ds_pat = re.compile(self.deepseek_model_pattern, re.IGNORECASE)
            oa_pat = re.compile(self.openai_model_pattern, re.IGNORECASE)
            custom_pat = re.compile(self.custom_model_pattern, re.IGNORECASE) if self.custom_model_pattern else None
        except re.error:
            ds_pat = re.compile(r"deepseek|deep-seek", re.IGNORECASE)
            oa_pat = re.compile(r"^(gpt-|o1|o3|o4|chatgpt)", re.IGNORECASE)
            custom_pat = None

        force_all_models_to_active_backend = bool((self.force_model or "").strip())
        if not alias and not force_all_models_to_active_backend:
            if ds_pat.search(model):
                backend = "deepseek"
            elif oa_pat.search(model):
                backend = "openai"
            elif custom_pat and custom_pat.search(model):
                backend = "custom"

        if backend == "deepseek":
            api_key = self.deepseek_api_key
            mode = normalize_upstream_mode(self.deepseek_upstream_mode)
            base_url = normalize_backend_base_url(self.deepseek_base_url, mode)
            mapped_model = alias_model or self.force_model or self.deepseek_model_map.get(model, model)
        elif backend == "openai":
            api_key = self.openai_api_key
            mode = normalize_upstream_mode(self.openai_upstream_mode)
            base_url = normalize_backend_base_url(self.openai_base_url, mode)
            mapped_model = alias_model or self.force_model or self.openai_model_map.get(model, model)
        elif backend == "custom":
            api_key = self.custom_api_key
            mode = normalize_upstream_mode(self.custom_upstream_mode)
            base_url = normalize_backend_base_url(self.custom_base_url, mode)
            mapped_model = alias_model or self.force_model or self.custom_model_map.get(model, model)
        else:
            raise ValueError(f"Unsupported backend '{backend}'. Use deepseek, openai, or custom.")

        if not api_key:
            raise ValueError(
                f"No API key configured for backend '{backend}'. "
                f"Set it in the dashboard: http://{self.proxy_host}:{self.proxy_port}/dashboard"
            )

        mapped_model = normalize_backend_model_id(backend, mapped_model)

        return {
            "backend": backend,
            "model": mapped_model,
            "api_key": api_key,
            "base_url": base_url,
            "mode": mode,
        }

    def get_model_alias(self, model: str) -> Optional[dict]:
        """Return a configured third-party model alias by Claude-facing model id."""
        for alias in normalized_model_aliases(self.model_aliases):
            if alias["id"] == model:
                return alias
        if (self.force_model or "").strip() and is_claude_compat_model_id(model):
            return {
                "id": model,
                "backend": self.default_backend,
                "model": self.force_model,
                "display_name": f"{model} → {self.force_model}",
            }
        return None


# Global config
config = Config()


def normalize_openai_base_url(base_url: str) -> str:
    """Return the OpenAI-compatible /v1 base URL without duplicating /v1."""
    cleaned = (base_url or "").rstrip("/")
    if not cleaned:
        return ""
    return cleaned if cleaned.endswith("/v1") else cleaned + "/v1"


def normalize_backend_model_id(backend: str, model: str) -> str:
    """Repair proven input typos without guessing a different upstream model."""
    cleaned = (model or "").strip()
    if not cleaned:
        return cleaned
    normalized = cleaned.lower()
    if backend == "deepseek":
        if normalized == "deep-chat":
            return "deepseek-chat"
        official_ids = {
            "deepseek-chat",
            "deepseek-reasoner",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
        }
        if normalized in official_ids:
            return normalized
    return cleaned


def normalize_upstream_mode(mode: str) -> str:
    mode = (mode or "openai").strip().lower()
    return "anthropic" if mode in {"anthropic", "native", "passthrough"} else "openai"


def normalize_anthropic_base_url(base_url: str) -> str:
    """Return an Anthropic Messages-compatible /v1 base URL."""
    cleaned = (base_url or "").rstrip("/")
    if not cleaned:
        return ""
    if cleaned.endswith("/v1"):
        return cleaned
    if cleaned.endswith("/anthropic"):
        return cleaned + "/v1"
    if "api.deepseek.com" in cleaned and "/anthropic" not in cleaned:
        return cleaned + "/anthropic/v1"
    return cleaned + "/v1"


def normalize_backend_base_url(base_url: str, mode: str) -> str:
    if normalize_upstream_mode(mode) == "anthropic":
        return normalize_anthropic_base_url(base_url)
    return normalize_openai_base_url(base_url)


def clamp_max_tokens_for_model(value, model: str) -> int:
    """Clamp max_tokens only when a per-model or default cap is configured."""
    try:
        requested = int(value)
    except (TypeError, ValueError):
        return value
    caps = config.model_token_caps if isinstance(config.model_token_caps, dict) else {}
    cap_value = caps.get(model) or caps.get(str(model).lower()) or config.default_max_tokens_cap
    try:
        cap = int(cap_value)
    except (TypeError, ValueError):
        cap = 0
    if cap > 0:
        return min(requested, cap)
    return requested


def build_anthropic_backend_body(body: dict, backend_model: str) -> dict:
    """Prepare a native Anthropic request for providers with /v1/messages support."""
    out = dict(body)
    out["model"] = backend_model
    thinking = out.get("thinking")
    if isinstance(thinking, dict) and str(thinking.get("type", "")).strip().lower() == "auto":
        # Claude Science emits `auto`, while DeepSeek and MiniMax native
        # Anthropic endpoints accept `adaptive`/`enabled`/`disabled`.
        # Adaptive mode chooses its own budget, so a caller-side fixed budget
        # must not leak through with the normalized type.
        normalized_thinking = dict(thinking)
        normalized_thinking["type"] = "adaptive"
        normalized_thinking.pop("budget_tokens", None)
        out["thinking"] = normalized_thinking
    if "max_tokens" in out:
        out["max_tokens"] = clamp_max_tokens_for_model(out["max_tokens"], backend_model)
    return out


def anthropic_backend_headers(api_key: str) -> dict:
    return {
        "x-api-key": api_key,
        "anthropic-version": "2023-06-01",
        "content-type": "application/json",
    }


def proxy_base_url(include_required_secret: bool = True) -> str:
    url = f"http://{config.proxy_host}:{config.proxy_port}"
    token = config.proxy_auth_token.strip()
    if include_required_secret and token and (config.proxy_auth_mode or "optional").lower() == "required":
        url += "/" + token
    return url


def mask_proxy_url(url: str) -> str:
    return re.sub(r"(://[^/]+/).+", r"\1****", url)


def mask_url_credentials(url: str) -> str:
    if not url:
        return ""
    try:
        parts = urlsplit(url)
        if "@" not in parts.netloc:
            return url
        host = parts.hostname or ""
        if parts.port:
            host = f"{host}:{parts.port}"
        return urlunsplit((parts.scheme, f"****@{host}", parts.path, parts.query, parts.fragment))
    except Exception:
        return "****"


def httpx_client_kwargs(timeout=120.0) -> dict:
    kwargs = {
        "timeout": httpx.Timeout(timeout, connect=10.0) if isinstance(timeout, (int, float)) else timeout,
        "trust_env": False,
    }
    outbound_proxy = (config.outbound_proxy_url or "").strip()
    if outbound_proxy:
        kwargs["proxy"] = outbound_proxy
    return kwargs


PROVIDER_PRESETS = {
    "deepseek_openai": {
        "label": "DeepSeek OpenAI-compatible",
        "backend": "deepseek",
        "base_url": "https://api.deepseek.com",
        "upstream_mode": "openai",
        "default_model": "",
        "model_aliases": [],
    },
    "deepseek_anthropic": {
        "label": "DeepSeek native Anthropic",
        "backend": "deepseek",
        "base_url": "https://api.deepseek.com/anthropic",
        "upstream_mode": "anthropic",
        "default_model": "",
        "model_aliases": [],
    },
    "siliconflow_kimi": {
        "label": "SiliconFlow Kimi",
        "backend": "custom",
        "base_url": "https://api.siliconflow.cn",
        "upstream_mode": "openai",
        "default_model": "",
        "model_aliases": [],
    },
    "dashscope_qwen": {
        "label": "Alibaba DashScope Qwen",
        "backend": "custom",
        "base_url": "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "upstream_mode": "openai",
        "default_model": "",
        "model_aliases": [],
    },
    "moonshot_anthropic": {
        "label": "Moonshot native Anthropic",
        "backend": "custom",
        "base_url": "https://api.moonshot.cn/anthropic",
        "upstream_mode": "anthropic",
        "default_model": "",
        "model_aliases": [],
    },
}


BUILTIN_COMPAT_MODELS = [
    {"id": "claude-sonnet-5", "type": "model", "display_name": "Claude Sonnet 5"},
    {"id": "claude-sonnet-4-5", "type": "model", "display_name": "Claude Sonnet 4.5"},
    {"id": "claude-opus-4-8", "type": "model", "display_name": "Claude Opus 4.8"},
    {"id": "claude-haiku-4-5-20251001", "type": "model", "display_name": "Claude Haiku 4.5"},
    {"id": "deepseek-v4-pro", "type": "model", "display_name": "DeepSeek V4 Pro"},
    {"id": "deepseek-v4-flash", "type": "model", "display_name": "DeepSeek V4 Flash"},
    {"id": "deepseek-chat", "type": "model", "display_name": "DeepSeek Chat (legacy)"},
    {"id": "gpt-4o", "type": "model", "display_name": "GPT-4o"},
]


CLAUDE_ROLE_COMPAT_MODELS = [
    {"id": "claude-sonnet-5", "type": "model", "display_name": "Claude Sonnet 5"},
    {"id": "claude-sonnet-4-5", "type": "model", "display_name": "Claude Sonnet 4.5"},
    {"id": "claude-opus-4-8", "type": "model", "display_name": "Claude Opus 4.8"},
    {"id": "claude-haiku-4-5-20251001", "type": "model", "display_name": "Claude Haiku 4.5"},
]


def is_claude_compat_model_id(model: str) -> bool:
    normalized = (model or "").strip().lower()
    return normalized.startswith("claude-") or normalized in {"sonnet", "opus", "haiku"}


def active_force_model_aliases(cfg: Config) -> list[dict]:
    force_model = (cfg.force_model or "").strip()
    if not force_model:
        return []
    existing = {a["id"] for a in normalized_model_aliases(cfg.model_aliases)}
    aliases = []
    for item in CLAUDE_ROLE_COMPAT_MODELS:
        if item["id"] in existing:
            continue
        aliases.append({
            "id": item["id"],
            "type": "model",
            "display_name": f"{item['display_name']} → {force_model}",
        })
    return aliases


def normalized_model_aliases(raw_aliases) -> list[dict]:
    """Normalize user-facing model aliases from config/env into list form."""
    if isinstance(raw_aliases, dict):
        items = []
        for alias_id, value in raw_aliases.items():
            if isinstance(value, dict):
                item = dict(value)
                item.setdefault("id", alias_id)
            else:
                item = {"id": alias_id, "model": value}
            items.append(item)
    elif isinstance(raw_aliases, list):
        items = raw_aliases
    else:
        items = []

    normalized = []
    for item in items:
        if not isinstance(item, dict):
            continue
        alias_id = str(item.get("id", "")).strip()
        if not alias_id:
            continue
        backend = str(item.get("backend") or "").strip().lower()
        if backend not in {"", "deepseek", "openai", "custom"}:
            continue
        model = str(item.get("model") or alias_id).strip()
        display_name = str(
            item.get("display_name") or item.get("name") or model or alias_id
        ).strip()
        normalized.append({
            "id": alias_id,
            "backend": backend,
            "model": model,
            "display_name": display_name,
        })
    return normalized


def model_list_for_config(cfg: Config) -> list[dict]:
    aliases = [
        {"id": a["id"], "type": "model", "display_name": a["display_name"]}
        for a in normalized_model_aliases(cfg.model_aliases)
    ]
    force_aliases = active_force_model_aliases(cfg)
    if force_aliases:
        seen = {m["id"] for m in aliases}
        aliases = aliases + [m for m in force_aliases if m["id"] not in seen]
    mode = (cfg.model_list_mode or "aliases_first").lower()
    if mode in {"aliases", "alias", "third_party", "third-party"}:
        return aliases
    if mode in {"builtin", "builtins", "compat"}:
        return list(BUILTIN_COMPAT_MODELS)
    if mode in {"aliases_first", "aliases-first", "mixed"}:
        seen = {m["id"] for m in aliases}
        return aliases + [m for m in BUILTIN_COMPAT_MODELS if m["id"] not in seen]
    return aliases or list(BUILTIN_COMPAT_MODELS)

# ---------------------------------------------------------------------------
# Request log (in-memory ring buffer)
# ---------------------------------------------------------------------------
MAX_LOG_ENTRIES = 200
request_log: list[dict] = []


def log_request(backend: str, model: str, stream: bool, status: str):
    entry = {
        "time": datetime.now().strftime("%H:%M:%S"),
        "backend": backend,
        "model": model,
        "stream": stream,
        "status": status,
    }
    request_log.append(entry)
    if len(request_log) > MAX_LOG_ENTRIES:
        request_log.pop(0)


def redact_proxy_auth_path(path: str) -> str:
    token = config.proxy_auth_token.strip()
    if token and (path == f"/{token}" or path.startswith(f"/{token}/")):
        return "/****" + path[len(token) + 1:]
    return path


def log_local_event(request: Request, status_code: int):
    path = redact_proxy_auth_path(request.url.path)
    if path.startswith("/static") or path in {"/dashboard", "/favicon.ico"}:
        return
    host = request.headers.get("host", "")
    entry = {
        "time": datetime.now().strftime("%H:%M:%S"),
        "backend": "local",
        "model": f"{request.method} {host}{path}",
        "stream": False,
        "status": str(status_code),
    }
    request_log.append(entry)
    if len(request_log) > MAX_LOG_ENTRIES:
        request_log.pop(0)
    print(f"[proxy] <- {request.method} host={host} path={path} status={status_code}")


# ---------------------------------------------------------------------------
# App
# ---------------------------------------------------------------------------
app = FastAPI(title="Claude Science BYOK Proxy", version="2.0.0")


def trusted_browser_origins() -> set[str]:
    """Origins used by the local dashboard and Claude Science browser UI."""
    port = config.proxy_port
    return {
        f"http://127.0.0.1:{port}",
        f"http://localhost:{port}",
        "http://127.0.0.1:8765",
        "http://localhost:8765",
        "http://127.0.0.1:8766",
        "http://localhost:8766",
    }


MANAGEMENT_API_PATHS = {
    "/api/config",
    "/api/provider-presets",
    "/api/test-backend",
    "/api/setup-global-env",
    "/api/install-service",
    "/api/refresh-token",
    "/api/recent-requests",
}


def is_trusted_browser_origin(origin: str) -> bool:
    return not origin or origin.rstrip("/") in trusted_browser_origins()


def valid_control_token(request: Request) -> bool:
    expected = config.proxy_auth_token.strip()
    if not expected:
        return False
    supplied = request.headers.get("x-proxy-control-token", "").strip()
    return bool(supplied) and hmac.compare_digest(supplied, expected)


# CORS
app.add_middleware(
    CORSMiddleware,
    allow_origins=sorted(trusted_browser_origins()),
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


# Path normalization middleware
class NormalizePathMiddleware(BaseHTTPMiddleware):
    PASSTHROUGH = {"/health", "/dashboard", "/docs", "/openapi.json", "/favicon.ico"}

    async def dispatch(self, request: Request, call_next):
        path = request.url.path

        if path.startswith("/api"):
            origin = request.headers.get("origin", "")
            if not is_trusted_browser_origin(origin):
                return JSONResponse({"ok": False, "error": "untrusted browser origin"}, status_code=403)

            auth_mode = (config.proxy_auth_mode or "optional").lower()
            if path in MANAGEMENT_API_PATHS and auth_mode == "required" and not valid_control_token(request):
                return JSONResponse({"ok": False, "error": "control authentication required"}, status_code=403)

            return await call_next(request)

        # Skip static files and dashboard. Management APIs are handled above.
        if path.startswith("/static") or path in self.PASSTHROUGH:
            return await call_next(request)

        token = config.proxy_auth_token.strip()
        auth_mode = (config.proxy_auth_mode or "optional").lower()
        if token:
            prefix = "/" + token
            if path == prefix or path.startswith(prefix + "/"):
                path = path[len(prefix):] or "/"
            elif auth_mode == "required":
                return JSONResponse(
                    {"type": "error", "error": {"type": "permission_error", "message": "forbidden"}},
                    status_code=403,
                    headers={"Connection": "close"},
                )

        while "/v1/v1/" in path:
            path = path.replace("/v1/v1/", "/v1/", 1)
        if not path.startswith("/v1/") and path not in self.PASSTHROUGH and not path.startswith("/docs"):
            path = "/v1" + path

        request.scope["path"] = path
        request.scope["raw_path"] = path.encode()
        return await call_next(request)


app.add_middleware(NormalizePathMiddleware)


@app.middleware("http")
async def access_log_middleware(request: Request, call_next):
    response = await call_next(request)
    log_local_event(request, response.status_code)
    return response

# Static files for dashboard
app.mount("/static", StaticFiles(directory=str(STATIC_DIR)), name="static")

# Shared HTTP client
_client: Optional[httpx.AsyncClient] = None
_client_proxy_url = None


def get_client() -> httpx.AsyncClient:
    global _client, _client_proxy_url
    outbound_proxy = (config.outbound_proxy_url or "").strip()
    if _client is None or _client_proxy_url != outbound_proxy:
        _client_proxy_url = outbound_proxy
        _client = httpx.AsyncClient(
            limits=httpx.Limits(max_keepalive_connections=20),
            **httpx_client_kwargs(timeout=120.0),
        )
    return _client


async def read_json_object(request: Request) -> tuple[Optional[dict], Optional[JSONResponse]]:
    try:
        body = await request.json()
    except json.JSONDecodeError:
        return None, JSONResponse(
            {"type": "error", "error": {"type": "invalid_request_error", "message": "Request body must be valid JSON."}},
            status_code=400,
        )
    except Exception as e:
        safe_msg = str(e).encode("ascii", errors="replace").decode("ascii")
        return None, JSONResponse(
            {"type": "error", "error": {"type": "invalid_request_error", "message": safe_msg}},
            status_code=400,
        )
    if not isinstance(body, dict):
        return None, JSONResponse(
            {"type": "error", "error": {"type": "invalid_request_error", "message": "Request body must be a JSON object."}},
            status_code=400,
        )
    return body, None


# ---------------------------------------------------------------------------
# Request/Response translation: Anthropic <-> OpenAI
# ---------------------------------------------------------------------------

TOOL_NAME_RE = re.compile(r"[^A-Za-z0-9_-]+")
DATA_IMAGE_RE = re.compile(r"^data:(image/[^;,]+);base64,(.*)$", re.DOTALL)
JSON_SCHEMA_TYPES = {"string", "number", "integer", "boolean", "object", "array"}
SCHEMA_COMBINATORS = ("anyOf", "oneOf", "allOf")
STREAM_HEARTBEAT_SECONDS = float(os.environ.get("STREAM_HEARTBEAT_SECONDS", "3"))
TOOL_CALLS_SECTION_BEGIN = "<|tool_calls_section_begin|>"
TOOL_CALLS_SECTION_END = "<|tool_calls_section_end|>"
TOOL_CALL_BEGIN = "<|tool_call_begin|>"
TOOL_CALL_END = "<|tool_call_end|>"
TOOL_CALL_ARGUMENT_BEGIN = "<|tool_call_argument_begin|>"
EMBEDDED_TOOL_MARKERS = (
    TOOL_CALLS_SECTION_BEGIN,
    TOOL_CALLS_SECTION_END,
    TOOL_CALL_BEGIN,
    TOOL_CALL_END,
    TOOL_CALL_ARGUMENT_BEGIN,
)
TRACE_LINE_RE = re.compile(
    r"^\s*(?:"
    r"The user (?:said|asked|wants|requested)\b|"
    r"The session (?:was|has been)\b|"
    r"Files on disk\b|"
    r"I have .{0,80}\binstalled\b|"
    r"I (?:need to|should|will|have to)\b|"
    r"Let me\b|"
    r"Now let me\b|"
    r"Now I (?:will|need to|should|have to)\b|"
    r"用户(?:要求|说|想要|让我)|"
    r"会话(?:已|被|恢复)|"
    r"我(?:需要|应该|先|将|会)|"
    r"现在让我|"
    r"让我\b"
    r")",
    re.IGNORECASE,
)
TRACE_NUMBERED_STEP_RE = re.compile(
    r"^\s*\d+[\.)]\s*(?:"
    r"(?:First\s+)?(?:check|run|create|load|inspect|verify|continue)\b|"
    r"(?:先|检查|运行|创建|加载|继续)"
    r")",
    re.IGNORECASE,
)
TRACE_CUE_RE = re.compile(
    r"(?:"
    r"The user (?:said|asked|wants|requested)|"
    r"session (?:was|has been) resumed|"
    r"Python kernel was reset|"
    r"Files on disk are intact|"
    r"I (?:need to|should|have to)|"
    r"Let me\b|"
    r"Let me (?:first )?(?:check|run|continue)|"
    r"Now I (?:will|need to|should|have to)|"
    r"用户(?:要求|说|想要|让我)|"
    r"会话已恢复|"
    r"内核已重置|"
    r"现在让我|"
    r"让我(?:先|检查|继续)"
    r")",
    re.IGNORECASE,
)
TRACE_PROBE_MIN_CHARS = 12


def normalize_tool_name(name, fallback: str) -> str:
    """OpenAI-compatible function names are alphanumeric plus _ and -."""
    cleaned = TOOL_NAME_RE.sub("_", str(name or fallback)).strip("_")
    return (cleaned or fallback)[:64]


def sse_event(event_type: str, data: dict) -> str:
    return f"event: {event_type}\ndata: {json.dumps(data)}\n\n"


async def stream_events_with_heartbeat(event_iter, interval: float = STREAM_HEARTBEAT_SECONDS):
    """Yield SSE events, adding ping heartbeats while an upstream stream is idle."""
    if interval <= 0:
        async for event in event_iter:
            yield event
        return

    agen = event_iter.__aiter__()
    task = asyncio.create_task(agen.__anext__())
    started = False
    try:
        while True:
            done, _ = await asyncio.wait({task}, timeout=interval)
            if not done:
                if started:
                    yield sse_event("ping", {"type": "ping"})
                continue
            try:
                event = task.result()
            except StopAsyncIteration:
                break
            started = True
            yield event
            task = asyncio.create_task(agen.__anext__())
    finally:
        if not task.done():
            task.cancel()


def build_tool_name_lookup(anthropic_body: dict) -> dict:
    """Map OpenAI-safe and provider-native tool names back to Claude names."""
    lookup = {}
    for idx, tool in enumerate(anthropic_body.get("tools") or []):
        if not isinstance(tool, dict):
            continue
        original = str(tool.get("name", "") or f"tool_{idx}")
        safe = normalize_tool_name(original, f"tool_{idx}")
        lookup[original] = original
        lookup[safe] = original
        lookup[f"functions.{original}"] = original
        lookup[f"functions.{safe}"] = original
        if original.startswith("functions."):
            short = original.split(".", 1)[1]
            lookup[short] = original
            lookup[normalize_tool_name(short, f"tool_{idx}")] = original
    return lookup


def _strip_provider_tool_prefix(name: str) -> str:
    cleaned = str(name or "").strip()
    if ":" in cleaned:
        maybe_name, maybe_index = cleaned.rsplit(":", 1)
        if maybe_index.strip().isdigit():
            cleaned = maybe_name.strip()
    if cleaned.startswith("functions."):
        cleaned = cleaned.split(".", 1)[1]
    return cleaned


def _resolve_response_tool_name(raw_name: str, fallback: str, tool_name_lookup: Optional[dict] = None) -> str:
    raw = str(raw_name or "").strip()
    stripped = _strip_provider_tool_prefix(raw)
    candidates = [
        raw,
        stripped,
        normalize_tool_name(raw, fallback),
        normalize_tool_name(stripped, fallback),
        f"functions.{stripped}" if stripped else "",
    ]
    for candidate in candidates:
        if candidate and tool_name_lookup and candidate in tool_name_lookup:
            return tool_name_lookup[candidate]
    return normalize_tool_name(stripped or raw, fallback)


def _decode_tool_arguments(raw_arguments: str) -> Optional[dict]:
    raw = (raw_arguments or "").strip()
    if not raw:
        return {}
    try:
        value, _ = json.JSONDecoder().raw_decode(raw)
    except json.JSONDecodeError:
        return None
    if isinstance(value, dict):
        return value
    return {"value": value}


def _find_first_embedded_tool_marker(text: str) -> int:
    positions = [text.find(marker) for marker in (TOOL_CALLS_SECTION_BEGIN, TOOL_CALL_BEGIN)]
    positions = [pos for pos in positions if pos >= 0]
    return min(positions) if positions else -1


def _next_marker_position(text: str, start: int) -> tuple[int, str]:
    found = []
    for marker in (TOOL_CALL_END, TOOL_CALLS_SECTION_END, TOOL_CALL_BEGIN):
        pos = text.find(marker, start)
        if pos >= 0:
            found.append((pos, marker))
    return min(found, default=(-1, ""))


def _skip_embedded_tool_noise(text: str, pos: int) -> int:
    while pos < len(text):
        advanced = False
        while pos < len(text) and text[pos].isspace():
            pos += 1
            advanced = True
        for marker in (TOOL_CALLS_SECTION_BEGIN, TOOL_CALL_END, TOOL_CALLS_SECTION_END):
            if text.startswith(marker, pos):
                pos += len(marker)
                advanced = True
                break
        if not advanced:
            return pos
    return pos


def extract_embedded_tool_calls(text: str, tool_name_lookup: Optional[dict] = None) -> tuple[str, list[dict]]:
    """Parse provider-native tool call markers leaked through message.content.

    Some OpenAI-compatible providers stream native text like
    `<|tool_call_begin|>functions.python:0<|tool_call_argument_begin|>{...}`.
    Claude Science expects Anthropic `tool_use` blocks instead, so convert
    complete JSON calls and remove the protocol markers from visible text.
    """
    if not isinstance(text, str) or not any(marker in text for marker in EMBEDDED_TOOL_MARKERS):
        return text or "", []

    first_marker = _find_first_embedded_tool_marker(text)
    if first_marker < 0:
        return text or "", []

    clean_parts = [text[:first_marker]]
    tool_calls = []
    pos = first_marker

    while pos < len(text):
        pos = _skip_embedded_tool_noise(text, pos)
        call_pos = text.find(TOOL_CALL_BEGIN, pos)
        if call_pos < 0:
            clean_parts.append(text[pos:])
            break
        clean_parts.append(text[pos:call_pos])

        header_start = call_pos + len(TOOL_CALL_BEGIN)
        args_marker = text.find(TOOL_CALL_ARGUMENT_BEGIN, header_start)
        if args_marker < 0:
            break

        raw_name = text[header_start:args_marker].strip()
        args_start = args_marker + len(TOOL_CALL_ARGUMENT_BEGIN)
        end_pos, end_marker = _next_marker_position(text, args_start)
        if end_pos < 0:
            end_pos, end_marker = len(text), ""

        raw_args = text[args_start:end_pos]
        arguments = _decode_tool_arguments(raw_args)
        if arguments is not None:
            idx = len(tool_calls)
            tool_calls.append({
                "id": f"toolu_{uuid.uuid4().hex[:12]}",
                "name": _resolve_response_tool_name(raw_name, f"tool_{idx}", tool_name_lookup),
                "input": arguments,
            })

        pos = end_pos + len(end_marker)

    return "".join(clean_parts).strip(), tool_calls


def _find_marker_start_in_buffer(text: str) -> int:
    return _find_first_embedded_tool_marker(text)


def _flushable_text_prefix(text: str) -> tuple[str, str]:
    """Keep possible marker prefixes buffered so split stream chunks are detected."""
    max_keep = 0
    for marker in (TOOL_CALLS_SECTION_BEGIN, TOOL_CALL_BEGIN):
        limit = min(len(marker) - 1, len(text))
        for size in range(1, limit + 1):
            if marker.startswith(text[-size:]):
                max_keep = max(max_keep, size)
    if max_keep:
        return text[:-max_keep], text[-max_keep:]
    return text, ""


def _looks_like_trace_line(line: str) -> bool:
    stripped = line.strip()
    if not stripped:
        return True
    return bool(TRACE_LINE_RE.search(stripped) or TRACE_NUMBERED_STEP_RE.search(stripped))


def strip_assistant_trace_text(text: str, *, aggressive: bool = False) -> str:
    """Remove provider-visible planning traces without touching normal answers.

    Some backends leak assistant scratchpad-style prose in `content`, e.g.
    "The user said ... Let me check files..." or the same pattern in Chinese.
    Those are not tool results or final answers, so hide them before returning
    an Anthropic response to Claude Science.
    """
    if not isinstance(text, str) or not text.strip():
        return text or ""

    cue_hits = len(TRACE_CUE_RE.findall(text[:1600]))
    if cue_hits == 0:
        return text.strip()

    lines = text.splitlines()
    kept = []
    dropping_leading_trace = True
    dropped = 0
    for line in lines:
        if dropping_leading_trace and _looks_like_trace_line(line):
            dropped += 1
            continue
        dropping_leading_trace = False
        kept.append(line)

    cleaned = "\n".join(kept).strip()

    # If a tool call follows, pre-tool narration is usually just scratchpad.
    if aggressive and cue_hits:
        return cleaned if cleaned and dropped == 0 else ""

    # If the whole message is a compact trace block, suppress it.
    meaningful_lines = [line for line in lines if line.strip()]
    if meaningful_lines and dropped >= len(meaningful_lines):
        return ""
    return cleaned or text.strip()


def _should_hold_visible_stream_text(text: str) -> bool:
    """Briefly buffer tool-enabled streams only while the prefix is ambiguous."""
    if not text:
        return True
    probe = text[:1600]
    if TRACE_CUE_RE.search(probe):
        return True
    if len(text) < TRACE_PROBE_MIN_CHARS and not re.search(r"[.!?。！？\n]", text):
        return True
    return False


def _pick_schema_type(value):
    if isinstance(value, str) and value in JSON_SCHEMA_TYPES:
        return value
    if isinstance(value, list):
        candidates = [v for v in value if isinstance(v, str) and v in JSON_SCHEMA_TYPES]
        if "object" in candidates:
            return "object"
        if "array" in candidates:
            return "array"
        if candidates:
            return candidates[0]
    return None


def _infer_schema_type(schema: dict):
    if "properties" in schema:
        return "object"
    if "items" in schema:
        return "array"
    enum_values = schema.get("enum")
    if isinstance(enum_values, list):
        for value in enum_values:
            if value is None:
                continue
            if isinstance(value, bool):
                return "boolean"
            if isinstance(value, int):
                return "integer"
            if isinstance(value, float):
                return "number"
            if isinstance(value, str):
                return "string"
    return None


def sanitize_tool_schema(schema, *, force_object: bool = False) -> dict:
    """Normalize Claude tool schemas for OpenAI-compatible providers.

    Claude Science can send tool schemas with a missing or null root type.
    DeepSeek rejects those for function parameters, so the root must always be
    an object schema. Nested schemas are kept permissive but never keep
    `type: null`.
    """
    if not isinstance(schema, dict):
        return {"type": "object", "properties": {}} if force_object else {}

    cleaned = {}
    schema_type = _pick_schema_type(schema.get("type")) or _infer_schema_type(schema)
    if force_object:
        schema_type = "object"
    if schema_type:
        cleaned["type"] = schema_type

    for key, value in schema.items():
        if key == "type" or value is None:
            continue
        if key == "properties":
            if isinstance(value, dict):
                cleaned["properties"] = {
                    str(prop_name): sanitize_tool_schema(prop_schema)
                    for prop_name, prop_schema in value.items()
                }
            continue
        if key == "items":
            if isinstance(value, dict):
                cleaned["items"] = sanitize_tool_schema(value)
            elif isinstance(value, list):
                cleaned["items"] = [sanitize_tool_schema(item) for item in value if isinstance(item, dict)]
            continue
        if key in SCHEMA_COMBINATORS:
            if isinstance(value, list):
                variants = [sanitize_tool_schema(item) for item in value if isinstance(item, dict)]
                if variants:
                    cleaned[key] = variants
            continue
        if key == "required":
            if isinstance(value, list):
                required = [item for item in value if isinstance(item, str)]
                if required:
                    cleaned["required"] = required
            continue
        if key == "enum":
            if isinstance(value, list):
                enum_values = [item for item in value if item is not None]
                if enum_values:
                    cleaned["enum"] = enum_values
            continue
        if key == "additionalProperties":
            if isinstance(value, bool):
                cleaned[key] = value
            elif isinstance(value, dict):
                cleaned[key] = sanitize_tool_schema(value)
            continue
        if key in {
            "description", "title", "format", "pattern", "minimum", "maximum",
            "exclusiveMinimum", "exclusiveMaximum", "minLength", "maxLength",
            "minItems", "maxItems", "default", "const",
        }:
            cleaned[key] = value

    if force_object:
        cleaned["type"] = "object"
        if not isinstance(cleaned.get("properties"), dict):
            cleaned["properties"] = {}
    return cleaned


def _is_inline_image_url(url: str) -> bool:
    return isinstance(url, str) and url.startswith("data:")


def _siliconflow_needs_jpeg_data_url(backend_name: str, backend_base_url: str) -> bool:
    return backend_name == "custom" and "siliconflow" in (backend_base_url or "").lower()


def _is_siliconflow_backend(backend_name: str, backend_base_url: str) -> bool:
    return backend_name == "custom" and "siliconflow" in (backend_base_url or "").lower()


def _convert_inline_image_to_jpeg_url(url: str, backend_name: str, backend_base_url: str) -> str:
    """Convert inline data images to JPEG for providers that reject PNG data URLs."""
    if not (_is_inline_image_url(url) and _siliconflow_needs_jpeg_data_url(backend_name, backend_base_url)):
        return url

    match = DATA_IMAGE_RE.match(url)
    if not match:
        return url

    mime_type = match.group(1).lower()
    if mime_type in {"image/jpeg", "image/jpg"}:
        return url
    try:
        raw = base64.b64decode(match.group(2), validate=False)
        from PIL import Image

        with Image.open(BytesIO(raw)) as image:
            image.seek(0)
            if image.mode not in {"RGB", "L"}:
                image = image.convert("RGB")
            out = BytesIO()
            image.save(out, format="JPEG", quality=92)
            encoded = base64.b64encode(out.getvalue()).decode("ascii")
        return f"data:image/jpeg;base64,{encoded}"
    except Exception:
        return url


def _openai_image_url_from_anthropic(block: dict) -> Optional[str]:
    if "image_url" in block:
        image_url = block["image_url"]
        if isinstance(image_url, dict):
            return image_url.get("url")
        if isinstance(image_url, str):
            return image_url
    if "source" in block:
        src = block["source"]
        if isinstance(src, dict):
            mt = src.get("media_type", "image/png")
            d = src.get("data", "")
            if d:
                return f"data:{mt};base64,{d}"
    return None


def _image_policy_for_backend(backend_name: str, backend_base_url: str) -> str:
    policy = (config.inline_image_policy or "auto").lower()
    if policy in {"preserve", "omit", "omit_inline"}:
        return policy
    if backend_name == "deepseek":
        return "omit"
    return "preserve"


def _convert_tool_choice(tool_choice, tool_name_map: dict, backend_name: str, backend_base_url: str):
    """Translate Anthropic tool_choice while avoiding provider-specific 400s."""
    if not tool_choice or backend_name == "deepseek":
        return None

    choice_type = tool_choice.get("type") if isinstance(tool_choice, dict) else tool_choice

    # SiliconFlow Kimi currently accepts only auto/none for tool_choice.
    if _is_siliconflow_backend(backend_name, backend_base_url):
        return "none" if choice_type == "none" else "auto"

    if isinstance(tool_choice, dict) and choice_type == "tool":
        choice_name = str(tool_choice.get("name", ""))
        return {
            "type": "function",
            "function": {"name": tool_name_map.get(choice_name, normalize_tool_name(choice_name, "tool_0"))},
        }
    if choice_type == "any":
        return "required"
    if choice_type == "auto":
        return "auto"
    if choice_type == "none":
        return "none"
    return None


def _normalized_model_family(model: str) -> str:
    """Return a provider-prefix-free lowercase model id for capability checks."""
    return str(model or "").strip().lower().rsplit("/", 1)[-1]


def _is_openai_o_series(model: str) -> bool:
    model_id = _normalized_model_family(model)
    return len(model_id) > 1 and model_id[0] == "o" and model_id[1].isdigit()


def _supports_openai_reasoning_effort(model: str) -> bool:
    model_id = _normalized_model_family(model)
    if _is_openai_o_series(model_id):
        return True
    match = re.match(r"^gpt-(\d+)", model_id)
    return bool(match and int(match.group(1)) >= 5)


def _requested_reasoning_effort(anthropic_body: dict) -> Optional[str]:
    """Translate only an explicit Anthropic reasoning request; never enable it implicitly."""
    output_config = anthropic_body.get("output_config")
    if isinstance(output_config, dict):
        effort = str(output_config.get("effort") or "").lower()
        if effort in {"low", "medium", "high"}:
            return effort
        if effort in {"max", "xhigh"}:
            return "xhigh"

    thinking = anthropic_body.get("thinking")
    if not isinstance(thinking, dict):
        return None
    thinking_type = str(thinking.get("type") or "").lower()
    if thinking_type in {"disabled", "none", "off"}:
        return None
    if thinking_type == "adaptive":
        return "xhigh"
    if thinking_type != "enabled":
        return None
    try:
        budget = int(thinking.get("budget_tokens"))
    except (TypeError, ValueError):
        return "high"
    if budget < 4000:
        return "low"
    if budget < 16000:
        return "medium"
    return "high"


def _requested_thinking_enabled(anthropic_body: dict) -> Optional[bool]:
    thinking = anthropic_body.get("thinking")
    output_config = anthropic_body.get("output_config")
    if isinstance(thinking, dict):
        thinking_type = str(thinking.get("type") or "").lower()
        if thinking_type in {"disabled", "none", "off"}:
            return False
        if thinking_type in {"enabled", "adaptive"}:
            return True
    if isinstance(output_config, dict) and output_config.get("effort"):
        return True
    return None


def _apply_reasoning_controls(
    openai_body: dict,
    anthropic_body: dict,
    backend_model: str,
    backend_name: str,
    backend_base_url: str,
) -> None:
    """Map explicit reasoning intent using platform-first, model-second capability rules."""
    enabled = _requested_thinking_enabled(anthropic_body)
    if enabled is None:
        return

    model_id = _normalized_model_family(backend_model)
    platform = f"{backend_name} {backend_base_url}".lower()
    effort = _requested_reasoning_effort(anthropic_body)

    # Aggregators define their own wire protocol even when the underlying model
    # belongs to another vendor, so platform rules must win over model names.
    if "openrouter" in platform:
        if enabled and effort:
            openai_body["reasoning"] = {"effort": effort}
        elif not enabled:
            openai_body["reasoning"] = {"effort": "none"}
        return
    if "siliconflow" in platform:
        openai_body["enable_thinking"] = enabled
        return

    if _supports_openai_reasoning_effort(model_id):
        if enabled and effort:
            openai_body["reasoning_effort"] = effort
        return
    if any(name in model_id for name in ("glm", "kimi", "moonshot", "deepseek", "mimo")):
        openai_body["thinking"] = {"type": "enabled" if enabled else "disabled"}
        return
    if "qwen" in model_id:
        openai_body["enable_thinking"] = enabled
        return
    if "minimax" in model_id:
        openai_body["reasoning_split"] = enabled


def _output_limit_from_error(error_text: str) -> Optional[int]:
    """Extract an advertised output ceiling without mistaking the requested value for it."""
    text = str(error_text or "").lower().replace(",", "")
    patterns = (
        r"(?:max_tokens|max_completion_tokens)[^\n]{0,120}?(?:less than or equal to|at most|maximum(?:\s+(?:is|of))?|<=)\s*[:=]?\s*(\d+)",
        r"(?:maximum|max)\s+(?:allowed\s+)?(?:output\s+)?tokens?[^\d]{0,30}(\d+)",
        r"(?:limit|ceiling)[^\d]{0,30}(\d+)\s*(?:output\s+)?tokens?",
    )
    for pattern in patterns:
        match = re.search(pattern, text)
        if not match:
            continue
        value = int(match.group(1))
        if 64 <= value <= 1_000_000:
            return value
    return None


def adapt_openai_body_after_error(body: dict, status_code: int, error_text: str) -> Optional[tuple[dict, str]]:
    """Return one conservative retry body for explicit parameter-compatibility errors."""
    if status_code not in {400, 422}:
        return None
    text = str(error_text or "").lower()
    adapted = dict(body)

    if "max_tokens" in text or "max_completion_tokens" in text or "output token" in text:
        if (
            "max_tokens" in adapted
            and "max_completion_tokens" in text
            and any(word in text for word in ("use", "instead", "unsupported", "not supported"))
        ):
            adapted["max_completion_tokens"] = adapted.pop("max_tokens")
            return adapted, "max_tokens->max_completion_tokens"

        limit = _output_limit_from_error(text)
        token_field = "max_completion_tokens" if "max_completion_tokens" in adapted else "max_tokens"
        if limit and token_field in adapted:
            try:
                requested = int(adapted[token_field])
            except (TypeError, ValueError):
                requested = limit
            adapted[token_field] = min(requested, limit)
            if adapted[token_field] != body.get(token_field):
                return adapted, f"{token_field}-clamped-to-provider-limit"

        # OpenAI-compatible upstreams define their own model default when the
        # optional output field is absent. This is safer than inventing a global cap.
        if token_field in adapted:
            adapted.pop(token_field)
            return adapted, f"{token_field}-omitted-for-provider-default"

    optional_fields = (
        "parallel_tool_calls",
        "reasoning_effort",
        "reasoning",
        "thinking",
        "enable_thinking",
        "reasoning_split",
        "temperature",
        "top_p",
    )
    for field in optional_fields:
        if field in adapted and field.lower() in text and any(
            marker in text for marker in ("unsupported", "not supported", "unknown", "unrecognized", "invalid")
        ):
            adapted.pop(field)
            return adapted, f"unsupported-{field}-removed"
    return None


def anthropic_to_openai(
    anthropic_body: dict,
    backend_model: str,
    backend_name: str = "",
    backend_base_url: str = "",
) -> dict:
    """Convert Anthropic Messages API request → OpenAI Chat Completions format."""
    openai_messages = []
    backend_name = backend_name.lower()
    image_policy = _image_policy_for_backend(backend_name, backend_base_url)

    # System prompt
    system = anthropic_body.get("system")
    if system:
        if isinstance(system, str):
            openai_messages.append({"role": "system", "content": system})
        elif isinstance(system, list):
            parts = [b["text"] for b in system if isinstance(b, dict) and b.get("type") == "text"]
            if parts:
                openai_messages.append({"role": "system", "content": "\n".join(parts)})

    # Messages
    for msg in anthropic_body.get("messages", []):
        role = msg.get("role", "user")
        content = msg.get("content")

        if role == "user":
            tool_messages = []
            if isinstance(content, str):
                openai_content = content
            elif isinstance(content, list):
                text_parts, image_parts, omitted_images = [], [], 0
                for block in content:
                    if not isinstance(block, dict):
                        continue
                    t = block.get("type", "")
                    if t == "tool_result":
                        tool_content = block.get("content", "")
                        if isinstance(tool_content, list):
                            result_parts = []
                            for item in tool_content:
                                if isinstance(item, dict) and item.get("type") == "text":
                                    result_parts.append(item.get("text", ""))
                                elif isinstance(item, str):
                                    result_parts.append(item)
                                else:
                                    result_parts.append(json.dumps(item, ensure_ascii=False))
                            tool_content = "\n".join(part for part in result_parts if part)
                        elif not isinstance(tool_content, str):
                            tool_content = json.dumps(tool_content, ensure_ascii=False)
                        tool_messages.append({
                            "role": "tool",
                            "tool_call_id": block.get("tool_use_id", ""),
                            "content": tool_content,
                        })
                    elif t == "text":
                        text_parts.append(block["text"])
                    elif t in ("image", "image_url"):
                        url = _openai_image_url_from_anthropic(block)
                        if not url:
                            omitted_images += 1
                            continue
                        if image_policy == "omit" or (image_policy == "omit_inline" and _is_inline_image_url(url)):
                            omitted_images += 1
                        else:
                            url = _convert_inline_image_to_jpeg_url(url, backend_name, backend_base_url)
                            image_parts.append({"type": "image_url", "image_url": {"url": url}})
                if image_parts:
                    openai_parts = list(image_parts)
                    if text_parts:
                        openai_parts.insert(0, {"type": "text", "text": " ".join(text_parts)})
                    if omitted_images:
                        openai_parts.append({
                            "type": "text",
                            "text": f"[{omitted_images} inline image attachment(s) omitted for backend compatibility.]",
                        })
                    openai_content = openai_parts
                elif omitted_images:
                    image_note = f"[{omitted_images} inline image attachment(s) omitted for backend compatibility.]"
                    openai_content = " ".join([*text_parts, image_note]).strip()
                else:
                    openai_content = " ".join(text_parts)
            else:
                openai_content = str(content)

            openai_messages.extend(tool_messages)
            if openai_content:
                openai_messages.append({"role": "user", "content": openai_content})

        elif role == "assistant":
            if isinstance(content, str):
                openai_messages.append({"role": "assistant", "content": content})
            elif isinstance(content, list):
                text_parts, tool_calls = [], []
                for block in content:
                    if block.get("type") == "text":
                        text_parts.append(block["text"])
                    elif block.get("type") == "tool_use":
                        tool_calls.append({
                            "id": block.get("id", ""),
                            "type": "function",
                            "function": {
                                "name": block.get("name", ""),
                                "arguments": json.dumps(block.get("input", {})),
                            },
                        })
                am = {"role": "assistant"}
                am["content"] = " ".join(text_parts) if text_parts else None
                if tool_calls:
                    am["tool_calls"] = tool_calls
                openai_messages.append(am)
            else:
                openai_messages.append({"role": "assistant", "content": str(content)})

    openai_body = {"model": backend_model, "messages": openai_messages}

    # Preserve the caller's output budget. If it is absent, omit the field so
    # an OpenAI-compatible upstream can apply its own model default instead of
    # receiving an arbitrary bridge-wide value.
    if "max_tokens" in anthropic_body and anthropic_body.get("max_tokens") is not None:
        max_tokens = clamp_max_tokens_for_model(anthropic_body["max_tokens"], backend_model)
        if _is_openai_o_series(backend_model):
            openai_body["max_completion_tokens"] = max_tokens
        else:
            openai_body["max_tokens"] = max_tokens

    if "temperature" in anthropic_body:
        openai_body["temperature"] = anthropic_body["temperature"]
    if "top_p" in anthropic_body:
        openai_body["top_p"] = anthropic_body["top_p"]

    stop_seq = anthropic_body.get("stop_sequences")
    if stop_seq:
        if isinstance(stop_seq, list) and len(stop_seq) == 1:
            openai_body["stop"] = stop_seq[0]
        elif isinstance(stop_seq, list):
            openai_body["stop"] = stop_seq

    openai_body["stream"] = anthropic_body.get("stream", False)

    # Tools
    tools = anthropic_body.get("tools")
    if tools:
        openai_tools = []
        tool_name_map = {}
        for idx, tool in enumerate(tools):
            if isinstance(tool, dict):
                original_name = str(tool.get("name", "") or f"tool_{idx}")
                safe_name = normalize_tool_name(original_name, f"tool_{idx}")
                tool_name_map[original_name] = safe_name
                parameters = sanitize_tool_schema(tool.get("input_schema", {}), force_object=True)
                openai_tools.append({
                    "type": "function",
                    "function": {
                        "name": safe_name,
                        "description": tool.get("description", ""),
                        "parameters": parameters,
                    },
                })
        if openai_tools:
            openai_body["tools"] = openai_tools
            tool_choice = anthropic_body.get("tool_choice")
            converted_choice = _convert_tool_choice(tool_choice, tool_name_map, backend_name, backend_base_url)
            if converted_choice:
                openai_body["tool_choice"] = converted_choice
            if isinstance(tool_choice, dict) and "disable_parallel_tool_use" in tool_choice:
                openai_body["parallel_tool_calls"] = not bool(tool_choice["disable_parallel_tool_use"])

    _apply_reasoning_controls(
        openai_body,
        anthropic_body,
        backend_model,
        backend_name,
        backend_base_url,
    )

    return openai_body


def openai_to_anthropic_response(
    openai_resp: dict,
    original_model: str,
    request_id: str,
    tool_name_lookup: Optional[dict] = None,
) -> dict:
    choice = openai_resp.get("choices", [{}])[0]
    message = choice.get("message", {})
    content_blocks = []

    normal_content = message.get("content", "") or ""
    reasoning_content = message.get("reasoning_content", "") or ""
    policy = config.reasoning_content_policy
    if policy == "always" and reasoning_content:
        text_content = reasoning_content + (f"\n\n{normal_content}" if normal_content else "")
    elif policy == "fallback":
        text_content = normal_content or reasoning_content
    else:
        text_content = normal_content
    raw_tool_calls = message.get("tool_calls") or []
    text_content, embedded_tool_calls = extract_embedded_tool_calls(text_content, tool_name_lookup)
    text_content = strip_assistant_trace_text(
        text_content,
        aggressive=bool(raw_tool_calls or embedded_tool_calls),
    )
    if text_content:
        content_blocks.append({"type": "text", "text": text_content})

    has_tool_use = False
    for tc in raw_tool_calls:
        func = tc.get("function", {})
        try:
            arguments = json.loads(func.get("arguments", "{}"))
        except json.JSONDecodeError:
            arguments = {"_raw": func.get("arguments", "{}")}
        if not isinstance(arguments, dict):
            arguments = {"value": arguments}
        has_tool_use = True
        content_blocks.append({
            "type": "tool_use",
            "id": tc.get("id", f"toolu_{uuid.uuid4().hex[:12]}"),
            "name": _resolve_response_tool_name(func.get("name", ""), "tool_0", tool_name_lookup),
            "input": arguments,
        })

    for embedded_call in embedded_tool_calls:
        has_tool_use = True
        content_blocks.append({
            "type": "tool_use",
            "id": embedded_call["id"],
            "name": embedded_call["name"],
            "input": embedded_call["input"],
        })

    usage = openai_resp.get("usage", {})
    return {
        "id": request_id,
        "type": "message",
        "role": "assistant",
        "content": content_blocks,
        "model": original_model,
        "stop_reason": "tool_use" if has_tool_use else _map_finish_reason(choice.get("finish_reason", "stop")),
        "stop_sequence": None,
        "usage": {"input_tokens": usage.get("prompt_tokens", 0), "output_tokens": usage.get("completion_tokens", 0)},
    }


def _map_finish_reason(r: str) -> str:
    m = {"stop": "end_turn", "length": "max_tokens", "tool_calls": "tool_use", "function_call": "tool_use", "content_filter": "end_turn"}
    return m.get(r, "end_turn")


# ---------------------------------------------------------------------------
# Streaming translation
# ---------------------------------------------------------------------------

async def translate_stream(
    openai_stream,
    original_model: str,
    request_id: str,
    tool_name_lookup: Optional[dict] = None,
):
    tool_calls_map: dict[int, dict] = {}
    finish_reason = None
    output_tokens = 0
    message_started = False
    content_block_started = False
    content_block_stopped = False
    content_block_index: Optional[int] = None
    next_block_index = 0
    pending_text = ""
    capturing_embedded_tools = False
    embedded_tool_text = ""
    hold_visible_text = bool(tool_name_lookup)

    def ev(t: str, d: dict) -> str:
        return f"event: {t}\ndata: {json.dumps(d)}\n\n"

    def message_start_event() -> str:
        return ev("message_start", {
            "type": "message_start",
            "message": {
                "id": request_id, "type": "message", "role": "assistant",
                "content": [], "model": original_model,
                "stop_reason": None, "stop_sequence": None,
                "usage": {"input_tokens": 0, "output_tokens": 0},
            },
        })

    def text_delta_events(text: str) -> list[str]:
        nonlocal content_block_started, content_block_stopped, content_block_index, next_block_index
        if not text:
            return []
        events = []
        if not content_block_started or content_block_stopped:
            content_block_started = True
            content_block_stopped = False
            content_block_index = next_block_index
            next_block_index += 1
            events.append(ev("content_block_start", {
                "type": "content_block_start",
                "index": content_block_index,
                "content_block": {"type": "text", "text": ""},
            }))
        events.append(ev("content_block_delta", {
            "type": "content_block_delta",
            "index": content_block_index,
            "delta": {"type": "text_delta", "text": text},
        }))
        return events

    def buffered_text_events(text_delta: str) -> list[str]:
        nonlocal pending_text, capturing_embedded_tools, embedded_tool_text, hold_visible_text
        if not text_delta:
            return []
        if capturing_embedded_tools:
            embedded_tool_text += text_delta
            return []

        pending_text += text_delta
        marker_pos = _find_marker_start_in_buffer(pending_text)
        if marker_pos >= 0:
            prefix = pending_text[:marker_pos]
            embedded_tool_text = pending_text[marker_pos:]
            pending_text = prefix
            capturing_embedded_tools = True
            if hold_visible_text:
                return []
            return text_delta_events(prefix)

        if hold_visible_text:
            if _should_hold_visible_stream_text(pending_text):
                return []
            hold_visible_text = False

        flush_text, pending_text = _flushable_text_prefix(pending_text)
        return text_delta_events(flush_text)

    def finalize_pending_text_events(*, aggressive: bool = False) -> tuple[list[str], list[dict]]:
        nonlocal pending_text, embedded_tool_text
        events = []
        embedded_calls = []
        if pending_text:
            clean_pending = strip_assistant_trace_text(pending_text, aggressive=aggressive)
            if clean_pending:
                events.extend(text_delta_events(clean_pending))
            pending_text = ""
        if embedded_tool_text:
            clean_text, embedded_calls = extract_embedded_tool_calls(embedded_tool_text, tool_name_lookup)
            clean_text = strip_assistant_trace_text(clean_text, aggressive=aggressive or bool(embedded_calls))
            embedded_tool_text = ""
            if clean_text:
                events.extend(text_delta_events(clean_text))
        return events, embedded_calls

    def start_tool_block_events(tool_call: dict, block_index: int) -> list[str]:
        return [ev("content_block_start", {
            "type": "content_block_start",
            "index": block_index,
            "content_block": {
                "type": "tool_use",
                "id": tool_call["id"],
                "name": tool_call["name"],
                "input": {},
            },
        })]

    def embedded_tool_events(tool_calls: list[dict]) -> list[str]:
        nonlocal next_block_index
        events = []
        for tool_call in tool_calls:
            block_index = next_block_index
            next_block_index += 1
            events.extend(start_tool_block_events(tool_call, block_index))
            arguments = json.dumps(tool_call.get("input", {}), ensure_ascii=False)
            if arguments:
                events.append(ev("content_block_delta", {
                    "type": "content_block_delta",
                    "index": block_index,
                    "delta": {"type": "input_json_delta", "partial_json": arguments},
                }))
            events.append(ev("content_block_stop", {"type": "content_block_stop", "index": block_index}))
        return events

    async for line in openai_stream.aiter_lines():
        if not line or not line.startswith("data: "):
            continue
        payload = line[6:]
        if payload.strip() == "[DONE]":
            break
        try:
            chunk = json.loads(payload)
        except json.JSONDecodeError:
            continue

        usage = chunk.get("usage") or {}
        if usage:
            output_tokens = usage.get("completion_tokens", output_tokens)

        choices = chunk.get("choices", [])
        if not choices:
            continue

        choice = choices[0]
        delta = choice.get("delta", {})
        finish_reason = choice.get("finish_reason") or finish_reason

        if not message_started:
            message_started = True
            yield message_start_event()

        text_delta = delta.get("content", "") or ""
        if not text_delta and config.reasoning_content_policy != "never":
            text_delta = delta.get("reasoning_content", "") or ""
        if text_delta:
            for event in buffered_text_events(text_delta):
                yield event

        for tc_delta in delta.get("tool_calls") or []:
            idx = tc_delta.get("index", 0)
            func_delta = tc_delta.get("function", {})
            if idx not in tool_calls_map:
                final_text_events, embedded_calls = finalize_pending_text_events(aggressive=True)
                for event in final_text_events:
                    yield event
                if content_block_started and not content_block_stopped:
                    yield ev("content_block_stop", {"type": "content_block_stop", "index": content_block_index})
                    content_block_stopped = True
                for event in embedded_tool_events(embedded_calls):
                    yield event
                tool_calls_map[idx] = {
                    "id": tc_delta.get("id", "") or f"toolu_{uuid.uuid4().hex[:12]}",
                    "name": _resolve_response_tool_name(func_delta.get("name", ""), f"tool_{idx}", tool_name_lookup),
                    "arguments": "",
                    "block_index": next_block_index,
                }
                next_block_index += 1
                start_events = start_tool_block_events(tool_calls_map[idx], tool_calls_map[idx]["block_index"])
                for event in start_events:
                    yield event
            if func_delta.get("name"):
                tool_calls_map[idx]["name"] = _resolve_response_tool_name(func_delta["name"], f"tool_{idx}", tool_name_lookup)
            if tc_delta.get("id"):
                tool_calls_map[idx]["id"] = tc_delta["id"]
            if func_delta.get("arguments"):
                tool_calls_map[idx]["arguments"] += func_delta["arguments"]
                yield ev("content_block_delta", {
                    "type": "content_block_delta",
                    "index": tool_calls_map[idx]["block_index"],
                    "delta": {"type": "input_json_delta", "partial_json": func_delta["arguments"]},
                })

        if finish_reason:
            final_text_events, embedded_calls = finalize_pending_text_events(aggressive=bool(tool_calls_map))
            for event in final_text_events:
                yield event
            if content_block_started and not content_block_stopped:
                yield ev("content_block_stop", {"type": "content_block_stop", "index": content_block_index})
                content_block_stopped = True
            for event in embedded_tool_events(embedded_calls):
                yield event
            for idx in sorted(tool_calls_map.keys()):
                block_index = tool_calls_map[idx]["block_index"]
                yield ev("content_block_delta", {"type": "content_block_delta", "index": block_index, "delta": {"type": "input_json_delta", "partial_json": ""}})
                yield ev("content_block_stop", {"type": "content_block_stop", "index": block_index})
            has_tool_use = bool(tool_calls_map) or bool(embedded_calls)
            stop_reason = "tool_use" if has_tool_use else _map_finish_reason(finish_reason)
            yield ev("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop_reason, "stop_sequence": None}, "usage": {"output_tokens": output_tokens}})
            yield ev("message_stop", {"type": "message_stop"})
            break

    if message_started and not finish_reason:
        final_text_events, embedded_calls = finalize_pending_text_events(aggressive=bool(tool_calls_map))
        for event in final_text_events:
            yield event
        if content_block_started and not content_block_stopped:
            yield ev("content_block_stop", {"type": "content_block_stop", "index": content_block_index})
            content_block_stopped = True
        for event in embedded_tool_events(embedded_calls):
            yield event
        for idx in sorted(tool_calls_map.keys()):
            block_index = tool_calls_map[idx]["block_index"]
            yield ev("content_block_delta", {"type": "content_block_delta", "index": block_index, "delta": {"type": "input_json_delta", "partial_json": ""}})
            yield ev("content_block_stop", {"type": "content_block_stop", "index": block_index})
        stop_reason = "tool_use" if tool_calls_map or embedded_calls else "end_turn"
        yield ev("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop_reason, "stop_sequence": None}, "usage": {"output_tokens": output_tokens}})
        yield ev("message_stop", {"type": "message_stop"})


# ---------------------------------------------------------------------------
# Anthropic API routes
# ---------------------------------------------------------------------------

@app.post("/v1/messages")
async def messages_api(request: Request):
    body, json_error = await read_json_object(request)
    if json_error:
        return json_error
    original_model = body.get("model", "claude-sonnet-4-5")

    try:
        backend = config.resolve_backend(original_model)
    except ValueError as e:
        return JSONResponse({"type": "error", "error": {"type": "api_error", "message": str(e)}}, status_code=400)

    stream = body.get("stream", False)
    request_id = f"msg_{uuid.uuid4().hex[:16]}"
    tool_name_lookup = build_tool_name_lookup(body)

    if backend["mode"] == "anthropic":
        native_body = build_anthropic_backend_body(body, backend["model"])
        headers = anthropic_backend_headers(backend["api_key"])
        client = get_client()
        url = f"{backend['base_url']}/messages"
        print(f"[proxy] → {backend['backend']} native Anthropic | model={backend['model']} | "
              f"stream={stream} | original_model={original_model}")

        if stream:
            async def native_stream_gen():
                try:
                    async with client.stream("POST", url, json=native_body, headers=headers) as backend_resp:
                        if backend_resp.status_code != 200:
                            try:
                                error_text = (await backend_resp.aread()).decode("utf-8", errors="replace")[:500]
                            except Exception:
                                error_text = "(unreadable response)"
                            print(f"[proxy] native backend error {backend_resp.status_code}: {error_text}", flush=True)
                            log_request(backend["backend"], backend["model"], True, f"error {backend_resp.status_code}")
                            err_msg = f"Backend error {backend_resp.status_code}: {error_text}"
                            safe_msg = err_msg.encode("ascii", errors="replace").decode("ascii")
                            yield f"event: error\ndata: {json.dumps({'type':'error','error':{'type':'api_error','message':safe_msg}})}\n\n"
                            return
                        log_request(backend["backend"], backend["model"], True, "success")
                        async for chunk in backend_resp.aiter_bytes():
                            if chunk:
                                yield chunk
                except Exception as e:
                    log_request(backend["backend"], backend["model"], True, "error")
                    safe_msg = str(e).encode("ascii", errors="replace").decode("ascii")
                    yield f"event: error\ndata: {json.dumps({'type':'error','error':{'type':'api_error','message':safe_msg}})}\n\n"

            return StreamingResponse(native_stream_gen(), media_type="text/event-stream",
                                     headers={"Cache-Control": "no-cache", "Connection": "keep-alive", "X-Accel-Buffering": "no"})

        try:
            resp = await client.post(url, json=native_body, headers=headers)
            if resp.status_code != 200:
                err_text = resp.text[:500] if resp.text else "(empty)"
                print(f"[proxy] native backend error {resp.status_code}: {err_text}", flush=True)
                log_request(backend["backend"], backend["model"], False, f"error {resp.status_code}")
                safe_msg = f"Backend returned {resp.status_code}: {err_text}".encode("ascii", errors="replace").decode("ascii")
                return JSONResponse({"type": "error", "error": {"type": "api_error", "message": safe_msg}}, status_code=resp.status_code)
            log_request(backend["backend"], backend["model"], False, "success")
            data = resp.json()
            if isinstance(data, dict) and data.get("type") == "message":
                data["model"] = original_model
            return JSONResponse(data)
        except Exception as e:
            log_request(backend["backend"], backend["model"], False, "error")
            safe_msg = str(e).encode("ascii", errors="replace").decode("ascii")
            return JSONResponse({"type": "error", "error": {"type": "api_error", "message": safe_msg}}, status_code=502)

    openai_body = anthropic_to_openai(body, backend["model"], backend["backend"], backend["base_url"])

    print(f"[proxy] → {backend['backend']} | model={backend['model']} | "
          f"stream={stream} | original_model={original_model}")

    headers = {"Authorization": f"Bearer {backend['api_key']}", "Content-Type": "application/json"}
    client = get_client()
    url = f"{backend['base_url']}/chat/completions"

    if stream:
        async def stream_gen():
            try:
                request_body = openai_body
                for attempt in range(2):
                    async with client.stream("POST", url, json=request_body, headers=headers) as backend_resp:
                        if backend_resp.status_code == 200:
                            log_request(backend["backend"], backend["model"], True, "success")
                            events = translate_stream(backend_resp, original_model, request_id, tool_name_lookup)
                            async for event in stream_events_with_heartbeat(events):
                                yield event
                            return
                        try:
                            error_text = (await backend_resp.aread()).decode("utf-8", errors="replace")[:500]
                        except Exception:
                            error_text = "(unreadable response)"
                        retry = adapt_openai_body_after_error(request_body, backend_resp.status_code, error_text)
                        if attempt == 0 and retry:
                            request_body, reason = retry
                            print(f"[proxy] retrying compatible request | reason={reason}", flush=True)
                            continue
                        print(f"[proxy] backend error {backend_resp.status_code}: {error_text}", flush=True)
                        log_request(backend["backend"], backend["model"], True, f"error {backend_resp.status_code}")
                        err_msg = f"Backend error {backend_resp.status_code}: {error_text}"
                        safe_msg = err_msg.encode("ascii", errors="replace").decode("ascii")
                        yield f"event: error\ndata: {json.dumps({'type':'error','error':{'type':'api_error','message':safe_msg}})}\n\n"
                        return
            except Exception as e:
                log_request(backend["backend"], backend["model"], True, "error")
                yield f"event: error\ndata: {json.dumps({'type':'error','error':{'type':'api_error','message':str(e)}})}\n\n"

        return StreamingResponse(stream_gen(), media_type="text/event-stream",
                                 headers={"Cache-Control": "no-cache", "Connection": "keep-alive", "X-Accel-Buffering": "no"})
    else:
        try:
            resp = await client.post(url, json=openai_body, headers=headers)
            if resp.status_code != 200:
                retry = adapt_openai_body_after_error(openai_body, resp.status_code, resp.text[:500])
                if retry:
                    retry_body, reason = retry
                    print(f"[proxy] retrying compatible request | reason={reason}", flush=True)
                    resp = await client.post(url, json=retry_body, headers=headers)
            if resp.status_code != 200:
                err_text = resp.text[:500] if resp.text else "(empty)"
                print(f"[proxy] backend error {resp.status_code}: {err_text}", flush=True)
                log_request(backend["backend"], backend["model"], False, f"error {resp.status_code}")
                safe_msg = f"Backend returned {resp.status_code}: {err_text}".encode("ascii", errors="replace").decode("ascii")
                return JSONResponse({"type": "error", "error": {"type": "api_error", "message": safe_msg}}, status_code=resp.status_code)
            log_request(backend["backend"], backend["model"], False, "success")
            return JSONResponse(openai_to_anthropic_response(resp.json(), original_model, request_id, tool_name_lookup))
        except Exception as e:
            log_request(backend["backend"], backend["model"], False, "error")
            safe_msg = str(e).encode("ascii", errors="replace").decode("ascii")
            return JSONResponse({"type": "error", "error": {"type": "api_error", "message": safe_msg}}, status_code=502)


@app.post("/v1/messages/count_tokens")
async def count_tokens(request: Request):
    body, json_error = await read_json_object(request)
    if json_error:
        return json_error
    total_chars = 0
    for msg in body.get("messages", []):
        content = msg.get("content", "")
        if isinstance(content, str):
            total_chars += len(content)
        elif isinstance(content, list):
            for block in content:
                if isinstance(block, dict):
                    total_chars += len(json.dumps(block))
    system = body.get("system", "")
    if isinstance(system, str):
        total_chars += len(system)
    elif isinstance(system, list):
        total_chars += len(json.dumps(system))
    return JSONResponse({"input_tokens": max(1, total_chars // 4)})


# ---------------------------------------------------------------------------
# OAuth mocks
# ---------------------------------------------------------------------------

FAKE_ACCOUNT_UUID = "byok-user-000000000000000000"
FAKE_ORG_UUID = "org_byok_000000000000"
FAKE_ACCESS_TOKEN = "fake-bearer-token-for-proxy"
FAKE_CLAUDE_AI_SCOPES = "user:inference user:file_upload user:profile user:mcp_servers user:plugins"


def fake_token_response() -> dict:
    return {
        "token_type": "bearer",
        "access_token": FAKE_ACCESS_TOKEN,
        "refresh_token": "fake-refresh-token",
        "expires_in": 999999999,
        "expires_at": "2099-12-31T23:59:59Z",
        "scope": FAKE_CLAUDE_AI_SCOPES,
        "scopes": FAKE_CLAUDE_AI_SCOPES,
        "provider": "claude_ai",
        "account": fake_account_response(),
        "organization": fake_org_response(),
    }


def fake_account_response() -> dict:
    return {
        "id": FAKE_ACCOUNT_UUID,
        "uuid": FAKE_ACCOUNT_UUID,
        "sub": FAKE_ACCOUNT_UUID,
        "email": "byok@localhost",
        "email_address": "byok@localhost",
        "email_verified": True,
        "name": "BYOK User",
        "display_name": "BYOK User",
    }


def fake_user_response() -> dict:
    account = fake_account_response()
    org = fake_org_response()
    return {
        **account,
        "id": FAKE_ACCOUNT_UUID,
        "uuid": FAKE_ACCOUNT_UUID,
        "sub": FAKE_ACCOUNT_UUID,
        "email": "byok@localhost",
        "email_address": "byok@localhost",
        "email_verified": True,
        "name": "BYOK User",
        "display_name": "BYOK User",
        "account": account,
        "user": account,
        "organization": fake_org_response(),
        "organizations": [org],
        "active_organization": org,
        "organization_uuid": FAKE_ORG_UUID,
        "org_uuid": FAKE_ORG_UUID,
        "enabled_plugins": [],
        "subscription_type": "max",
        "rate_limit_tier": "tier_5",
        "seat_tier": "enterprise_usage_based",
        "billing_type": "api",
        "has_extra_usage_enabled": True,
    }


def fake_org_response() -> dict:
    return {
        "id": FAKE_ORG_UUID,
        "uuid": FAKE_ORG_UUID,
        "name": "BYOK Organization",
        "type": "organization",
        "organization_type": "claude_max",
        "status": "active",
        "default_role": "admin",
        "subscription": {"type": "max", "status": "active"},
        "rate_limit_tier": "tier_5",
        "seat_tier": "enterprise_usage_based",
        "billing_type": "api",
        "has_extra_usage_enabled": True,
        "claude_ai_completion_feedback_enabled": False,
    }


def fake_org_list_response() -> dict:
    org = fake_org_response()
    return {
        **org,
        "data": [org],
        "organizations": [org],
        "has_more": False,
        "first_id": org["id"],
        "last_id": org["id"],
    }


@app.api_route("/v1/oauth/{path:path}", methods=["GET", "POST", "PUT", "DELETE"])
async def oauth_mock(request: Request, path: str):
    return JSONResponse(fake_token_response())


@app.get("/v1/userinfo")
@app.get("/v1/me")
@app.get("/v1/user")
@app.get("/v1/profile")
@app.get("/v1/account")
async def userinfo_mock(request: Request):
    return JSONResponse(fake_user_response())



@app.get("/v1/models")
async def list_models(request: Request):
    """Return compatible model list."""
    models = model_list_for_config(config)
    return JSONResponse({
        "data": models,
        "has_more": False,
        "first_id": models[0]["id"] if models else None,
        "last_id": models[-1]["id"] if models else None,
    })


# Add proper organization endpoint (not just catch-all)
@app.get("/v1/organizations")
async def orgs_mock(request: Request):
    """Mock organization list endpoint."""
    return JSONResponse(fake_org_list_response())


@app.get("/v1/organization")
@app.get("/v1/organizations/{org_id}")
async def org_mock(request: Request, org_id: str = FAKE_ORG_UUID):
    """Mock single organization endpoint."""
    return JSONResponse(fake_org_response())


@app.api_route("/v1/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH"])
async def catch_all(request: Request, path: str):
    lowered = path.lower()
    if "oauth" in lowered or "token" in lowered:
        return JSONResponse(fake_token_response())
    if "organization" in lowered or lowered.startswith("org"):
        return JSONResponse(fake_org_list_response())
    if any(k in lowered for k in ("userinfo", "profile", "account", "user", "me")):
        return JSONResponse(fake_user_response())
    return JSONResponse({"ok": True})


# ---------------------------------------------------------------------------
# Dashboard & Management API
# ---------------------------------------------------------------------------

@app.get("/dashboard")
async def dashboard():
    return FileResponse(str(STATIC_DIR / "dashboard.html"))


@app.get("/api/config")
async def api_get_config():
    return config.public_dict()


@app.post("/api/config")
async def api_update_config(request: Request):
    body, json_error = await read_json_object(request)
    if json_error:
        return json_error
    allowed_keys = {
        "deepseek_api_key", "openai_api_key", "custom_api_key",
        "deepseek_base_url", "openai_base_url", "custom_base_url",
        "default_backend", "force_model",
        "deepseek_model_map", "openai_model_map", "custom_model_map",
        "model_aliases", "model_list_mode",
        "model_token_caps", "default_max_tokens_cap",
        "deepseek_upstream_mode", "openai_upstream_mode", "custom_upstream_mode",
        "proxy_auth_token", "proxy_auth_mode", "outbound_proxy_url",
        "deepseek_model_pattern", "openai_model_pattern", "custom_model_pattern",
        "reasoning_content_policy", "inline_image_policy",
    }
    update_data = {k: v for k, v in body.items() if k in allowed_keys}
    # Reject masked API keys (bullet character U+2022)
    for key in ("deepseek_api_key", "openai_api_key", "custom_api_key"):
        if key in update_data and "•" in update_data[key]:
            del update_data[key]  # Skip masked placeholder
    if "proxy_auth_token" in update_data and "•" in str(update_data["proxy_auth_token"]):
        del update_data["proxy_auth_token"]
    if update_data:
        config.update(update_data)
        return {"ok": True}
    return {"ok": False, "error": "No valid config keys provided"}


@app.get("/api/provider-presets")
async def api_provider_presets():
    return {"presets": PROVIDER_PRESETS}


@app.post("/api/test-backend")
async def api_test_backend(request: Request):
    """Test connectivity to a backend provider."""
    body, json_error = await read_json_object(request)
    if json_error:
        return {"ok": False, "error": "Request body must be valid JSON."}
    provider = body.get("provider", "deepseek")
    api_key = body.get("api_key", "")
    base_url = body.get("base_url", "")
    upstream_mode = normalize_upstream_mode(body.get("upstream_mode", "openai"))

    if not api_key:
        return {"ok": False, "error": "API key is required"}

    if upstream_mode == "anthropic":
        if base_url:
            url = f"{normalize_anthropic_base_url(base_url)}/models"
        elif provider == "deepseek":
            url = "https://api.deepseek.com/anthropic/v1/models"
        else:
            return {"ok": False, "error": "Anthropic mode requires an API Base URL"}
        headers = anthropic_backend_headers(api_key)
    elif base_url:
        url = f"{normalize_openai_base_url(base_url)}/models"
        headers = {"Authorization": f"Bearer {api_key}"}
    elif provider == "deepseek":
        url = "https://api.deepseek.com/v1/models"
        headers = {"Authorization": f"Bearer {api_key}"}
    elif provider == "openai":
        url = "https://api.openai.com/v1/models"
        headers = {"Authorization": f"Bearer {api_key}"}
    else:
        return {"ok": False, "error": "Custom provider requires an API Base URL"}

    try:
        async with httpx.AsyncClient(**httpx_client_kwargs(timeout=10.0)) as c:
            resp = await c.get(url, headers=headers)
            if resp.status_code == 200:
                data = resp.json()
                models = [m.get("id", "") for m in data.get("data", [])[:10]]
                return {"ok": True, "models": models}
            else:
                return {"ok": False, "error": f"HTTP {resp.status_code}: {resp.text[:300]}"}
    except Exception as e:
        return {"ok": False, "error": str(e)}


@app.post("/api/setup-global-env")
async def api_setup_global_env():
    """Set ANTHROPIC_BASE_URL for future desktop-launched clients."""
    proxy_url = proxy_base_url()
    try:
        if os.name != "nt":
            return {"ok": False, "error": "Windows user environment setup is only supported on Windows."}
        ps = (
            "[Environment]::SetEnvironmentVariable("
            f"'ANTHROPIC_BASE_URL', {json.dumps(proxy_url)}, 'User')"
        )
        subprocess.run(
            ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", ps],
            capture_output=True, text=True, timeout=10, check=True,
        )
        os.environ["ANTHROPIC_BASE_URL"] = proxy_url
        return {"ok": True, "proxy_url": mask_proxy_url(proxy_url), "method": "windows-user-env"}
    except Exception as e:
        return {"ok": False, "error": str(e)}


@app.post("/api/install-service")
async def api_install_service():
    """Install the proxy as a per-user background service."""
    proxy_url = proxy_base_url()
    logs_dir = Path.home() / ".claude-science" / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)

    if os.name != "nt":
        return {"ok": False, "error": "Windows scheduled task install is only supported on Windows."}

    task_name = "ClaudeScienceByokProxy"
    runner_path = Path.home() / ".claude-science" / "run-proxy.ps1"

    def ps_quote(value) -> str:
        return str(value).replace("'", "''")

    runner_content = f"""$ErrorActionPreference = 'Stop'
$env:ANTHROPIC_BASE_URL = '{ps_quote(proxy_url)}'
$env:PROXY_HOST = '{ps_quote(config.proxy_host)}'
$env:PROXY_PORT = '{ps_quote(config.proxy_port)}'
Set-Location '{ps_quote(PROXY_DIR)}'
& '{ps_quote(sys.executable)}' '{ps_quote(Path(__file__).resolve())}' *>> '{ps_quote(logs_dir / "proxy.log")}'
"""
    runner_path.parent.mkdir(parents=True, exist_ok=True)
    runner_path.write_text(runner_content, encoding="utf-8")

    task_script = f"""
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File "{runner_path}"'
$trigger = New-ScheduledTaskTrigger -AtLogOn
Register-ScheduledTask -TaskName '{task_name}' -Action $action -Trigger $trigger -Description 'Claude Science BYOK proxy' -Force | Out-Null
Start-ScheduledTask -TaskName '{task_name}'
"""
    try:
        subprocess.run(
            ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", task_script],
            capture_output=True, text=True, timeout=20, check=True,
        )
        os.environ["ANTHROPIC_BASE_URL"] = proxy_url
        return {
            "ok": True,
            "task_name": task_name,
            "runner_path": str(runner_path),
            "proxy_url": mask_proxy_url(proxy_url),
        }
    except Exception as e:
        return {"ok": False, "error": str(e)}


@app.post("/api/refresh-token")
async def api_refresh_token():
    """Re-generate the fake OAuth token."""
    try:
        result = subprocess.run(
            [sys.executable, str(PROXY_DIR / "setup-token.py")],
            capture_output=True, text=True, timeout=10,
        )
        if result.returncode == 0:
            return {"ok": True, "output": result.stdout.strip().split("\n")[-3:]}
        return {"ok": False, "error": result.stderr or result.stdout}
    except Exception as e:
        return {"ok": False, "error": str(e)}


@app.get("/api/recent-requests")
async def api_recent_requests():
    return {"requests": list(reversed(request_log[-50:]))}


@app.delete("/api/recent-requests")
async def api_clear_requests():
    request_log.clear()
    return {"ok": True}


@app.api_route("/api/oauth/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH"])
async def api_oauth_mock(request: Request, path: str):
    lowered = path.lower()
    if any(k in lowered for k in ("profile", "account", "userinfo", "user", "me")):
        return JSONResponse(fake_user_response())
    if "organization" in lowered or lowered.startswith("org"):
        return JSONResponse(fake_org_list_response())
    if "usage" in lowered:
        return JSONResponse({
            "usage": {"used": 0, "limit": 999999999, "remaining": 999999999},
            "organization": fake_org_response(),
            "organizations": [fake_org_response()],
        })
    return JSONResponse(fake_token_response())


@app.api_route("/api/auth/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH"])
async def api_auth_mock(request: Request, path: str):
    lowered = path.lower()
    if "organization" in lowered or lowered.startswith("org"):
        return JSONResponse(fake_org_list_response())
    return JSONResponse(fake_user_response())


@app.get("/api/userinfo")
@app.get("/api/me")
@app.get("/api/user")
@app.get("/api/profile")
@app.get("/api/account")
async def api_userinfo_mock(request: Request):
    return JSONResponse(fake_user_response())


@app.get("/api/organizations")
async def api_orgs_mock(request: Request):
    return JSONResponse(fake_org_list_response())


@app.get("/api/organization")
@app.get("/api/organizations/{org_id}")
async def api_org_mock(request: Request, org_id: str = FAKE_ORG_UUID):
    return JSONResponse(fake_org_response())


@app.api_route("/api/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "PATCH"])
async def api_anthropic_catch_all(request: Request, path: str):
    lowered = path.lower()
    if "oauth" in lowered or "token" in lowered:
        return JSONResponse(fake_token_response())
    if "organization" in lowered or lowered.startswith("org"):
        return JSONResponse(fake_org_list_response())
    if any(k in lowered for k in ("userinfo", "profile", "account", "user", "me")):
        return JSONResponse(fake_user_response())
    return JSONResponse({"ok": True})


# ---------------------------------------------------------------------------
# Health check
# ---------------------------------------------------------------------------

@app.get("/health")
async def health():
    return {
        "status": "ok",
        "deepseek_configured": bool(config.deepseek_api_key),
        "openai_configured": bool(config.openai_api_key),
        "custom_configured": bool(config.custom_api_key and config.custom_base_url),
        "default_backend": config.default_backend,
        "force_model": config.force_model or "(none)",
        "model_list_mode": config.model_list_mode,
        "model_aliases": len(normalized_model_aliases(config.model_aliases)),
        "upstream_modes": {
            "deepseek": normalize_upstream_mode(config.deepseek_upstream_mode),
            "openai": normalize_upstream_mode(config.openai_upstream_mode),
            "custom": normalize_upstream_mode(config.custom_upstream_mode),
        },
        "proxy_auth_mode": config.proxy_auth_mode,
        "proxy_auth_configured": bool(config.proxy_auth_token),
        "outbound_proxy_configured": bool((config.outbound_proxy_url or "").strip()),
        "outbound_proxy_url": mask_url_credentials(config.outbound_proxy_url),
        "inline_image_policy": config.inline_image_policy,
        "proxy_dir": str(PROXY_DIR),
        "source_path": str(Path(__file__).resolve()),
        "config_revision": str(config.get("_csa_revision", "") or ""),
    }


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import threading
    import uvicorn

    HTTPS_PORT = config.proxy_port + 1  # 9877 by default
    CERT_DIR = PROXY_DIR / "certs"
    SSL_CERT = str(CERT_DIR / "server-cert.pem")
    SSL_KEY = str(CERT_DIR / "server-key.pem")

    have_ssl = os.path.exists(SSL_CERT) and os.path.exists(SSL_KEY)

    print(f"\n{'='*60}")
    print(f"  Claude Science BYOK Proxy v2.1")
    print(f"  Dashboard → http://{config.proxy_host}:{config.proxy_port}/dashboard")
    if have_ssl:
        print(f"  HTTPS     → https://{config.proxy_host}:{HTTPS_PORT}")
        print(f"  Cert CN   → api.anthropic.com")
    print(f"  Health    → http://{config.proxy_host}:{config.proxy_port}/health")
    print(f"{'='*60}\n")

    if have_ssl:
        # Start HTTPS server in a background thread
        def run_https():
            uvicorn.run(
                app, host=config.proxy_host, port=HTTPS_PORT,
                ssl_keyfile=SSL_KEY, ssl_certfile=SSL_CERT,
                log_level="warning",
            )

        t = threading.Thread(target=run_https, daemon=True)
        t.start()
        print(f"[proxy] HTTPS server started on port {HTTPS_PORT}")

    # Start HTTP server (main thread)
    uvicorn.run(app, host=config.proxy_host, port=config.proxy_port, log_level="warning")
