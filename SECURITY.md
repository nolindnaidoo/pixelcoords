# Security policy

## Supported versions

The latest release and the current `main` branch.

## Reporting a vulnerability

Report privately via GitHub Security Advisories: the repository's
Security tab, then "Report a vulnerability". Please do not open a public
issue for security reports. You can expect an acknowledgment within a few
days.

## Scope notes

pixelcoords is an offline tool: it makes no network calls, and captures
are written only to the local output directory. The areas most worth
scrutiny are file writes (session output, crop cleanup), the session JSON
schema consumed by third parties, and the screen-capture permission
handling on macOS.
