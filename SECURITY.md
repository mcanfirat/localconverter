# Security policy

## Reporting a vulnerability

Please report security issues privately via GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository. **Do not open a public issue.**

Include: what you found, how to reproduce it, which version and platform, and
what an attacker could achieve. A proof-of-concept file is welcome — but please
describe it rather than attaching anything genuinely malicious.

We aim to acknowledge within 3 working days and to ship a fix or a documented
mitigation within 90 days, crediting you unless you prefer otherwise.

## In scope

- Escaping the temporary workspace, or cleanup deleting files outside it
- Path traversal, symlink escape, or archive extraction outside the destination
- Modifying or destroying a user's source file
- Any network transmission of file contents, names, paths or hashes
- Command injection into a conversion engine
- Bypassing the IPC boundary to reach the filesystem, a shell or a process
- Accepting a bundled binary whose checksum does not match
- An invalid or corrupt output being reported as a successful conversion

That last one is a security issue here, not merely a bug: the product's claim is
that a verified output can be trusted.

## Out of scope

- Crashes on deliberately malformed input that cannot escalate beyond a crash
  (please still file these as ordinary bugs)
- Vulnerabilities in an upstream engine with no LocalConvert-specific amplifier —
  report those upstream, and tell us so we can pin a fixed version
- Missing code signing on locally built artifacts
- Anything requiring an attacker to already have code execution as the user

## Supported versions

Pre-1.0. Only the latest release receives fixes.

## Threat model

[docs/SECURITY.md](docs/SECURITY.md) documents the full threat model, including
which mitigations exist today and which belong to phases that have not shipped.
