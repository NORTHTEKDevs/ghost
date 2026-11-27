# Code signing policy

What gets signed, by whom, how, and what a user can check for themselves.
This is the policy SignPath Foundation asks an open-source project to publish;
it is written to be true whether or not that application succeeds.

## Who

Ghost is maintained by **Kristian Baer** (Northtek, FrostByte LLC, Alaska,
<info@northtek.io>), currently the sole maintainer.

| Role | Who | What they do |
| --- | --- | --- |
| Author | Kristian Baer | Writes and merges the code; creates the release tag. |
| Reviewer | Kristian Baer | Reviews the diff a release contains before tagging. Single-maintainer today; this becomes a second person the moment there is one, and the table is updated in the same commit. |
| Approver | Kristian Baer | Approves a signing request. |

Every account with write access to the repository, and to any signing service,
uses multi-factor authentication. There is exactly one such account.

## What gets signed

Only artifacts built by this repository's own release workflow, from a tag on
`main`:

- `ghost.exe` - the command-line tool
- `ghost-http.exe` - the local HTTP server
- `ghost-mcp.exe` - the MCP server

They are signed **before** packaging, so the `.zip` archive and the `.mcpb`
bundle both contain signed executables. Nothing built anywhere else is ever
signed, and no third party's binaries are ever signed.

Linux artifacts (`.tar.gz`, `.mcpb`) are not Authenticode-signed, because
Authenticode is a Windows format. They carry the same build provenance
attestation and SHA-256 checksums as the Windows artifacts.

## How a release is built and signed

1. A `v*` tag is pushed to `main`.
2. `.github/workflows/release.yml` builds on GitHub-hosted runners:
   `windows-latest` for the Windows binaries, `ubuntu-latest` for Linux. No
   self-hosted runners, no local builds, no uploaded artifacts.
3. If a signing certificate is configured, the three Windows executables are
   signed with SHA-256 and an RFC 3161 timestamp, so signatures outlive the
   certificate's own validity.
4. The workflow reads back `Get-AuthenticodeSignature` for each file and
   **fails the release** unless all three report `Valid`. A signing step that
   exits zero is not accepted as evidence that anything was signed.
5. Archives and MCP bundles are packed, SHA-256 checksums written, and build
   provenance attested (`actions/attest-build-provenance`).
6. The release is published with every artifact and its checksum.

The private key is never present in the repository, never in an environment
variable in plaintext beyond the runner's own memory, and never on a
maintainer's workstation for CI purposes.

## What a user can verify

Provenance, which needs no certificate and covers Linux too:

```bash
gh attestation verify ghost-windows-x64.mcpb --repo NORTHTEKDevs/ghost
```

A pass proves the file came out of this repository's release workflow at a
named commit. Checksums are published beside every artifact. On Windows, once
signing is enabled, `Get-AuthenticodeSignature` reports the publisher.

## Privacy: what leaves your machine

**By default, nothing.** Ghost is a local tool. It has no telemetry, no
analytics, no crash reporting, no update check, and no account. Nothing about
your machine, your screen or your usage is sent anywhere.

Two features make network requests, both only when you ask for them:

- **The optional vision tier.** If, and only if, you set a vision API key,
  `ghost_locate_by_description` and its siblings send a screenshot of the
  window being automated to the provider you configured - Anthropic, NVIDIA,
  OpenAI or any OpenAI-compatible endpoint you name. That screenshot may
  contain whatever is on that window. With no key set, this tier is off and no
  image is ever transmitted. Most callers never enable it: the model driving
  Ghost reads `ghost_see` itself.
- **Browser control.** `ghost_browser_*` talks to a browser's DevTools endpoint
  on `127.0.0.1`. That request never leaves the machine.

`ghost-http` binds to localhost and is off unless started.

Ghost reads a great deal *locally* - window contents, the screen, other
processes' command lines - because that is what a desktop automation tool is.
It is the agent you connect that decides what to do with it, and Ghost's job is
to make that visible: every response reports what it touched, and the
interference audit records anything that moved the foreground.

## Uninstalling

Ghost installs nothing and registers nothing: no service, no scheduled task, no
autostart entry, no registry keys beyond what Windows records for any executed
program. Delete the binaries, or remove the extension in your MCP client, and
it is gone. Hidden desktops it created are destroyed with the process that owns
them.

## Reporting a problem

Security issues and false-positive antivirus reports:
<https://github.com/NORTHTEKDevs/ghost/issues>, or <info@northtek.io> for
anything that should not be public first. See [`antivirus.md`](antivirus.md)
for why a scanner might flag Ghost and how to report it upstream.

## Attribution

*(Added to this file and to the README once the application below is approved -
not before, because claiming it early would be false.)*

> Free code signing provided by [SignPath.io](https://signpath.io), certificate
> by [SignPath Foundation](https://signpath.org).
