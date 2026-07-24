---
name: csa-external-agent
description: Submit a redacted, manually approved request from Claude Science to the local CSA Subagent Hub when sandbox limits block dataset downloads, environment diagnosis, VM/SSH/GPU checks, migration planning, or another host-side investigation. Use this skill to request an external Agent plan and later read its outbox result; never use it to execute host commands directly.
---

# CSA External Agent

Use this skill only when work is blocked by a capability outside the current
Claude Science sandbox. The first release is a request-and-approval bridge, not
remote shell access.

## Submit a request

1. Classify the request as `dataset`, `environment`, `vm`, `migration`, or
   `custom`.
2. Reduce the note to the goal, a redacted error summary, and checks already
   attempted. Never include API keys, tokens, passwords, cookies, private keys,
   complete `.env` files, or unrelated conversation history.
3. From the current project workspace, run:

```bash
~/.claude-science/skills/csa-external-agent/scripts/submit-request.sh \
  --project-root "$PWD" \
  --task-kind environment \
  --title "Diagnose dependency installation failure" \
  --note "Read-only diagnosis requested; error details are redacted."
```

4. Tell the user that the request is waiting for approval in CSA Subagent Hub.
   Do not claim that the external Agent has started.

Every request uses `approvalMode=manual`, `policyId=manual-only`, and one of the
non-executing actions `diagnose`, `plan`, or `review`.

## Read a result

After the user approves and completes the external Agent run, use the request
ID printed by the submit script:

```bash
~/.claude-science/skills/csa-external-agent/scripts/read-result.sh \
  --project-root "$PWD" \
  --request-id "req-YYYYMMDD-HHMMSS-xxxxxxxx"
```

Treat `running` as incomplete. For `completed`, read `summary` first and open
the relative `resultPath` only if more detail is needed. Do not follow paths
outside the current project.

## Safety boundary

- Do not invoke Claude Code or another host Agent directly from the sandbox.
- Do not turn a request into shell, install, download, migration, deletion, or
  credential-management commands.
- Migration requests are planning and diagnosis only.
- Do not poll continuously. Check after the user says the run is complete or
  when resuming the task.
- Read [references/safety-policy.md](references/safety-policy.md) before handling
  an ambiguous or potentially destructive request.
- Read [references/request-schema.md](references/request-schema.md) when
  debugging interoperability with the CSA panel.
