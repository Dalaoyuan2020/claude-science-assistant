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

Loopback binding is not an authentication boundary. The management API rejects
untrusted browser origins, and required path/control-token mode is supported,
but the current local default can still be `optional`. Do not expose the Bridge
to a LAN or public interface. A future non-loopback mode must require a
high-entropy data token, strict Host/Origin checks, and rate limiting before it
can be considered supported.

Run only one writable Bridge instance. A Windows Bridge and a WSL Bridge must
not share the same configuration file or present separate dashboards for one
logical installation.

## Reporting Issues

When filing issues, redact API keys, tokens, private keys, proxy credentials, and prompt content.
