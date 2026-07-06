# Security Policy

## Local Secrets

Never commit:

- `config.json`
- `.env`
- `certs/`
- OAuth token files under `~\.claude-science\.oauth-tokens\`
- `~\.claude-science\encryption.key`
- logs that may contain backend error text

If `outbound_proxy_url` includes credentials, treat it as a secret.

## Network Safety

The default installation must not modify system DNS, Clash, v2rayN, sing-box, VPN, TUN, Windows system proxy, hosts, certificate trust, or port 443.

Use `outbound_proxy_url` for explicit backend egress through a local node.

## Local Control Plane

Loopback binding is not an authentication boundary. Before distributing this
project, the management API must require a control credential, reject untrusted
Origin and Host headers, and avoid wildcard CORS. The data API must use a
high-entropy token by default. Do not expose the current service to a LAN or
package it as a launcher-managed product until these controls and their tests
are in place.

Run only one writable Bridge instance. A Windows Bridge and a WSL Bridge must
not share the same configuration file or present separate dashboards for one
logical installation.

## Reporting Issues

When filing issues, redact API keys, tokens, private keys, proxy credentials, and prompt content.
