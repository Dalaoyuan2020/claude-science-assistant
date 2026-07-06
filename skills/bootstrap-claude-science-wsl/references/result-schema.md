# Inspection result schema

Both inspectors emit a JSON object with `schema_version: 1`, `generated_at`, `mode: read-only`, component facts, runtime facts, and `secrets.values_included: false`.

Classify results as follows:

| Status | Meaning |
|---|---|
| `pass` | Requirement is present and verified. |
| `warning` | Product can run, but reliability or security is reduced. |
| `blocker` | Installation or launch must not continue. |

Overall states:

- `ready`: no blockers; required services pass.
- `repairable`: one or more blockers have an in-place repair that does not require reboot.
- `reboot-required`: Windows feature state cannot be verified until reboot.
- `unsupported`: OS, architecture, virtualization, or policy cannot support the product.

Treat a Windows Bridge plus a WSL Bridge as a blocker for product launch, even if both health endpoints answer.
