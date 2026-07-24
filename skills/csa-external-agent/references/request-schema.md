# Request and result schema

The submit script writes one UTF-8 JSON file to:

```text
reports/csa-agent-inbox/<requestId>.json
```

Required request fields:

```text
schemaVersion, source, taskKind, title, cwd, note,
requestedAction, approvalMode, policyId, createdAt
```

Allowed values:

```text
taskKind: dataset | environment | vm | migration | custom
requestedAction: diagnose | plan | review
approvalMode: manual
policyId: manual-only
```

CSA writes the stable result pointer to:

```text
reports/csa-agent-outbox/<requestId>.json
```

Result fields include `status`, `latestRunId`, `sessionId`, relative
`resultPath`, redacted `summary`, `nextAction`, and `updatedAt`.
