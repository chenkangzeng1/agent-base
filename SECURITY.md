# Security Policy

## Reporting a vulnerability

If you discover a security vulnerability in phi-agent, please **do not** open a public issue.

Instead, email the maintainer at phiagent@hibuka.com with:

- A description of the vulnerability
- Steps to reproduce
- Affected versions
- Any suggested fixes (if available)

You will receive a response within 48 hours. The issue will be addressed as quickly as possible, and a fix will be released via a patch version.

## Scope

Security concerns include but are not limited to:

- Command injection in `LocalShellTool`
- Path traversal in file operations
- Credential leakage (API keys, session tokens)
- CDP/browser session hijacking

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅ |

Only the latest released version receives security patches.
