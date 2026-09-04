# Security Policy

## Supported versions

Security fixes are applied on the default branch (`main`) for the latest published work.

## Reporting a vulnerability

Please **do not** open a public issue for security vulnerabilities.

Report them privately via [GitHub Security Advisories](https://github.com/snowopsdev/selara/security/advisories/new) for this repository.

Include:

- A description of the issue and its impact
- Steps to reproduce (or a proof of concept if available)
- Affected versions / commit if known

We will acknowledge the report and work on a fix. Please give us reasonable time to address the issue before any public disclosure.

## Secrets and credentials

Never commit API keys, tokens, private keys, or local config that contains secrets. Prefer environment variables such as `SELARA_API_KEY`. See [CONTRIBUTING.md](CONTRIBUTING.md).
