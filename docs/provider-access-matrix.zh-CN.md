# Provider access matrix

Last checked: 2026-07-06.

This table records the access path used by the launcher presets. The bridge receives Anthropic-style requests from Claude Science; `openai` upstream mode converts them to OpenAI Chat Completions, while `anthropic` upstream mode sends Anthropic Messages-compatible requests.

| Provider preset | Upstream mode | Base URL stored by preset | Runtime endpoint | Default model | Notes |
| --- | --- | --- | --- | --- | --- |
| GLM official | `openai` | `https://open.bigmodel.cn/api/paas/v4` | `/chat/completions`, `/models` | `glm-5.2` | General BigModel API path. GLM Coding Plan keys may require the coding endpoint from Z.ai docs instead of this general endpoint. |
| LongCat official | `openai` | `https://api.longcat.chat/openai` | `/v1/chat/completions`, `/v1/models` | `LongCat-2.0` | The bridge normalizes the stored base to `/openai/v1` before calling. |
| DeepSeek official | `anthropic` | `https://api.deepseek.com/anthropic` | `/v1/messages`, `/v1/models` | `deepseek-v4-pro` | Fast/Haiku aliases map to `deepseek-v4-flash`. Do not set the DeepSeek catalog default to `glm-5.2`. |
| MiniMax official | `anthropic` | `https://api.minimax.io/anthropic` | `/v1/messages`, `/v1/models` | `MiniMax-M3` | Fast/Haiku aliases map to `MiniMax-M2.7-highspeed` when the default model is selected. China users can change the base URL to `https://api.minimaxi.com/anthropic`. |
| OpenAI / GPT | `openai` | `https://api.openai.com/v1` | `/chat/completions`, `/models` | `gpt-5.5` | The bridge still uses Chat Completions because it translates Claude Science's Anthropic payloads. |
| OpenCode Go | `openai` | `https://opencode.ai/zen/go/v1` | `/chat/completions`, `/models` | `glm-5.2` | The API model ID sent to Go is the bare ID (`glm-5.2`), not the OpenCode config prefix (`opencode-go/glm-5.2`). The preset only auto-maps Go models documented for OpenAI-compatible chat completions: GLM, Kimi, DeepSeek, and MiMo. MiniMax/Qwen Go models require the Anthropic-compatible `/messages` path and are not auto-selected by this OpenAI-mode preset. |
| OpenRouter | `openai` | `https://openrouter.ai/api/v1` | `/chat/completions`, `/models` | none | Requires a tested or explicit model because the available model set changes. |
| Built-in relay | `openai` | `https://10521052.xyz/v1` | `/chat/completions`, `/models` | none | Third-party relay, not marked official or trusted. |
| Custom relay | `openai` | user supplied | depends on user URL | none | Requires explicit confirmation before sending an API key. |

Official docs checked:

- DeepSeek API docs: https://api-docs.deepseek.com/
- Z.ai / GLM coding configuration: https://zcode.z.ai/en/docs/configuration
- LongCat API docs: https://longcat.chat/platform/docs/
- MiniMax platform docs: https://platform.minimax.io/docs/
- OpenAI API docs: https://developers.openai.com/api/docs/
- OpenCode Go docs: https://opencode.ai/docs/go/
- OpenRouter docs: https://openrouter.ai/docs/
