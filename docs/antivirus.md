# Antivirus and SmartScreen

Ghost does, on purpose, the things antivirus heuristics are built to notice:
it injects keyboard and mouse input, captures the screen, enumerates every
window, creates hidden desktops, and reads other processes' command lines. A
scanner that has never seen this binary has to decide from those facts alone
whether it is a computer-use tool or a remote access trojan. This page records
what Ghost does to make that decision easy, what you can check yourself, and
what to do if an engine still gets it wrong.

## What the binaries do to look like what they are

- **They say who built them.** Every Windows binary carries a version
  resource: company (`Northtek (FrostByte LLC)`), product (`Ghost`), a file
  description, the release version, and the repository URL. An executable with
  no metadata at all is the single most common trait of malware droppers and is
  scored accordingly; Rust binaries ship with none unless the build adds it, so
  the build adds it (`crates/*/build.rs`).
- **They carry an application manifest** that declares `asInvoker` (Ghost never
  asks for elevation), Windows 10/11 support, per-monitor DPI awareness, and
  long-path support. Manifest-less executables that touch input APIs are another
  well-known heuristic negative.
- **They never read another process's memory.** Command lines are read through
  `NtQueryInformationProcess(ProcessCommandLineInformation)` with
  `PROCESS_QUERY_LIMITED_INFORMATION`, the least privilege that works. The
  `ReadProcessMemory` + `PROCESS_VM_READ` route, which credential dumpers and
  injectors use and which used to be in Ghost's import table, is gone.
- **They are not packed, obfuscated, or self-modifying.** Plain MSVC-linked Rust
  release builds. Packers raise detection rates rather than lower them, and
  Ghost has nothing to hide.
- **Every release publishes SHA-256 checksums** for the archives and the MCP
  Bundles, and the MCP registry entry pins the bundle checksums, so you can
  verify that what you ran is what was released.
- **Code signing is wired and waiting on a signing identity**; see
  [`code-signing.md`](code-signing.md). A signature from a Microsoft-validated
  publisher is the one remaining step that changes a scanner's verdict from
  "unknown" to "known", and it needs a certificate, not code.

## What you can check

```powershell
# Publisher and product metadata (empty on a binary that lacks it)
(Get-Item .\ghost-mcp.exe).VersionInfo | Format-List CompanyName, ProductName, FileDescription, FileVersion

# The checksum against the one published on the release
(Get-FileHash .\ghost-windows-x64.zip -Algorithm SHA256).Hash.ToLower()
Get-Content .\ghost-windows-x64.zip.sha256
```

You can also upload the release archive to [VirusTotal](https://www.virustotal.com)
and compare the report against the checksum above; it is public software and
there is nothing to protect.

## If an engine still quarantines it

1. **Confirm it is the released file.** Checksum first. A binary that does not
   match the release is not Ghost's problem to explain.
2. **Report the false positive to the vendor.** For Microsoft Defender the
   route is the [Microsoft Security Intelligence submission portal](https://www.microsoft.com/en-us/wdsi/filesubmission):
   sign in, choose *Software developer*, upload the flagged file, and say it is
   a false positive on an open-source desktop automation tool. Microsoft
   typically answers within a business day, and a cleared file stops being
   flagged for everyone. Every major vendor has an equivalent form.
3. **Exclude the folder as a last resort**, only on a machine you control, and
   only after step 1:

   ```powershell
   Add-MpPreference -ExclusionPath "C:\path\to\ghost"
   ```

   An exclusion tells Defender to stop looking at that folder. That is the
   right call for a build directory you compile into; it is the wrong call for
   a download you have not verified.

## Why not just sign it now

Signing needs a certificate bound to a verified legal identity, which needs the
company's owner to complete an identity validation with Microsoft. The pipeline
already signs and verifies on every tag the moment that identity exists. Until
then the measures above are what a maintainer can do without a certificate,
and they are the same measures a signed binary still needs, because a signature
on a metadata-less executable that reads process memory does not look much
better than no signature at all.
