# Security Policy

## Reporting a vulnerability

Please report security vulnerabilities through
[GitHub private vulnerability reporting](../../security/advisories/new)
(the "Security" tab → "Report a vulnerability"). Do not open a public
issue for a vulnerability.

Include what an attacker can do, the request or input that does it, and
the version you tested. You get an acknowledgement within a few days; the
fix and the advisory are coordinated with you before disclosure.

## Scope

Sumidoku serves a web UI and API, executes third-party WASM source
packages, and fetches remote content on behalf of the user. In scope:
authentication bypass, path traversal outside the configured directories,
escaping the WASM sandbox, SSRF beyond what a configured source can
already do, and credential or database exposure. Out of scope: what a
malicious *source package* does with the network access sources are
expected to have — sources are user-installed trust, like browser
extensions.

Only the latest release is supported.
