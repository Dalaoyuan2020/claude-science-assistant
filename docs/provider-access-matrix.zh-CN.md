# Provider access matrix

Last checked: 2026-07-11.

This table records the access path used by the launcher presets. The bridge receives Anthropic-style requests from Claude Science; `openai` upstream mode converts them to OpenAI Chat Completions, while `anthropic` upstream mode sends Anthropic Messages-compatible requests.

| Provider preset | Upstream mode | Base URL stored by preset | Runtime endpoint | Initial model | Notes |
| --- | --- | --- | --- | --- | --- |
| GLM official | `openai` | `https://open.bigmodel.cn/api/paas/v4` | `/chat/completions`, `/models` | none | User enters a model or fetches the live list. GLM Coding Plan keys may require the coding endpoint from Z.ai docs. |
| LongCat official | `openai` | `https://api.longcat.chat/openai` | `/v1/chat/completions`, `/v1/models` | none | The bridge normalizes the stored base to `/openai/v1` before calling. |
| DeepSeek official | `anthropic` | `https://api.deepseek.com/anthropic` | `/v1/messages`; `/v1/models` may be unavailable | none | Live models are preferred. If no testable list is returned, CSA may try documented official candidates, but it saves a model only after a real message succeeds. |
| MiniMax China official | `anthropic` | `https://api.minimaxi.com/anthropic` | `/v1/messages`; `/v1/models` is capability-detected | none | `MiniMax-M3` and the supported M2.x IDs are documented, but CSA keeps the initial model empty. If `/models` is unavailable, the user explicitly enters an official model ID before testing. |
| OpenAI / GPT | `openai` | `https://api.openai.com/v1` | `/chat/completions`, `/models` | none | The bridge uses Chat Completions because it translates Claude Science's Anthropic payloads. |
| OpenCode Go | `openai` | `https://opencode.ai/zen/go/v1` | `/chat/completions`, `/models` | none | Live models are scored after discovery; the preset does not inject model IDs that the account did not return. |
| OpenRouter | `openai` | `https://openrouter.ai/api/v1` | `/chat/completions`, `/models` | none | Requires a tested or explicit model because the available model set changes. |
| Project-operated relay | `openai` | `https://10521052.xyz/v1` | `/chat/completions`, `/models` | none | Operated by the CSA project, not an official model-vendor API; explicit domain confirmation remains required. |
| Custom relay | `openai` | user supplied | depends on user URL | none | Requires explicit confirmation before sending an API key. |

## Live capability observations (2026-07-11)

These checks used the locally saved Provider entries. They sent short `OK` prompts and a forced
`add_numbers` function call. No API key or response body is recorded in this document.

| Saved Provider | `/models` | Text at 256 | `max_tokens=32768` accepted | Native tool call | Reasoning control | Interpretation |
| --- | --- | --- | --- | --- | --- | --- |
| OpenCode Go / `glm-5.2` | HTTP 200, 20 models, selected model listed | HTTP 200 with visible text | yes | yes | `thinking:{type:"enabled"}` accepted and returned reasoning content | The service is usable and is not currently capped below 32768 at request validation. This does not prove the exact output ceiling. |
| DeepSeek official / `deepseek-v4-pro` | no testable model returned by `/models` | HTTP 200 with visible text; streaming and non-streaming passed | not probed | not probed | Claude Science `thinking:{type:"auto"}` is normalized to native `adaptive`; request completed | The temporary key was valid. The earlier retry loop was a request-parameter compatibility bug, not an authentication failure. |
| MiniMax China official | capability-detected | short conversation confirmed usable | not probed | not probed | not fully probed | Basic connectivity is confirmed; do not claim output, tool, or reasoning limits until the explicit capability probe is run. |
| Saved custom relay / `claude-opus-4-8` | HTTP 200, 4 models, selected model listed | HTTP 200 with visible text | yes | yes | not probed because a Claude model behind an OpenAI-compatible relay does not reveal a safe universal reasoning field | Text and tools work. The relay's `/models` response does not advertise an output ceiling. |
| Built-in relay / `gpt-5.5` | HTTP 502 | HTTP 502 | no result | no result | no result | The upstream was unavailable during this check. A 502 is not evidence of a token or tool limit. |

`accepted` means the gateway accepted that request parameter and completed a deliberately short
answer. It does not mean the model generated 32768 tokens, and it must not be presented as the
provider's documented maximum.

None of the two reachable OpenAI-compatible `/models` responses exposed `max_tokens`,
`max_output_tokens`, or `context_window`. Exact limits therefore cannot be discovered reliably from
those endpoints. The bridge uses this order instead:

1. preserve the caller's output budget;
2. when no budget is supplied, omit the OpenAI field and let the upstream use its model default;
3. apply a per-model cap only when `model_token_caps` explicitly contains one;
4. after a precise HTTP 400/422 parameter error, retry once using the advertised ceiling, the
   provider default, or the required `max_completion_tokens` field;
5. never apply a bridge-wide hard-coded output limit.

Run `scripts/probe-provider-capabilities.ps1` manually when a saved Provider needs rechecking. The
script deduplicates identical entries, decrypts keys only in the current Windows user process, and
prints capability metadata rather than secrets or answer text. It makes real billable requests, so
it is intentionally not part of routine startup.

## Parameter adaptation rules

- Anthropic `max_tokens` is preserved for ordinary OpenAI-compatible chat models.
- OpenAI `o1` / `o3` / `o4` families receive `max_completion_tokens`.
- `disable_parallel_tool_use` is translated to OpenAI `parallel_tool_calls`.
- Thinking is never enabled just because a model name looks capable. Only an explicit caller request
  is translated.
- For native Anthropic-compatible upstreams, caller `thinking.type=auto` is normalized to
  `adaptive` without a fixed `budget_tokens`; explicit `enabled` and `disabled` are preserved.
- Platform rules take precedence over model names: OpenRouter uses `reasoning.effort`, SiliconFlow
  uses `enable_thinking`, and then model-family rules handle GPT/o-series, GLM/Kimi/DeepSeek/MiMo,
  Qwen, and MiniMax.
- Unsupported optional parameters are removed only after a specific 400/422 error, with at most one
  retry. Authentication, model-not-found, quota, 5xx, and network failures are not disguised as
  parameter adaptation.

Official docs checked:

- DeepSeek API docs: https://api-docs.deepseek.com/
- Z.ai / GLM coding configuration: https://zcode.z.ai/en/docs/configuration
- LongCat API docs: https://longcat.chat/platform/docs/
- MiniMax China Anthropic API docs: https://platform.minimaxi.com/docs/api-reference/text-anthropic-api
- OpenAI API docs: https://developers.openai.com/api/docs/
- OpenCode Go docs: https://opencode.ai/docs/go/
- OpenRouter docs: https://openrouter.ai/docs/
