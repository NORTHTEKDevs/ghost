# Code signing the Windows binaries

## Why

The Windows binaries are not code-signed. Windows SmartScreen therefore shows
"Windows protected your PC" on the first run of `ghost.exe`, `ghost-http.exe`
and `ghost-mcp.exe`, and the user has to click *More info -> Run anyway*. For
a tool people install from a README that is the single largest piece of
first-run friction, and no amount of code fixes it: the fix is a certificate
tied to a verified legal identity.

## What is already in place

`.github/workflows/release.yml` signs all three binaries with
[Azure Artifact Signing](https://learn.microsoft.com/azure/trusted-signing/)
(the service formerly named Trusted Signing) **as soon as the secrets below
exist**, before the archive and the MCP Bundle are packed, so every shipped
copy carries the signature. It then verifies each file reports a `Valid`
Authenticode status and fails the release if not. With no secrets set, the
step is skipped and the run prints a notice; nothing else changes.

## What only the account owner can do

Azure Artifact Signing needs a verified identity for FrostByte LLC (the entity
behind Northtek). This is a one-time setup, roughly a day of elapsed time,
almost all of it waiting on Microsoft's identity validation.

1. **Create the signing account.** In the Azure portal, create an *Artifact
   Signing* (Trusted Signing) account in a supported region (East US is
   typical). Pricing is a low flat monthly fee (about $10/month at the time of
   writing, "Basic" tier), far below a traditional OV/EV certificate.
2. **Identity validation.** Under the account, start an *Identity validation*
   of type Organization for FrostByte LLC. Microsoft checks the legal entity
   (state registration, EIN, address); expect a request for documents and
   1 to 3 business days. It must pass before a certificate profile can be
   created.
3. **Certificate profile.** Create a *Public Trust* certificate profile bound
   to that validated identity. Note its name.
4. **App registration for CI.** In Entra ID, create an App Registration,
   create a client secret for it, and grant that application the
   *Trusted Signing Certificate Profile Signer* role on the signing account.
5. **Repository secrets** (Settings -> Secrets and variables -> Actions):

   | Secret | Value |
   |---|---|
   | `AZURE_TENANT_ID` | the Entra tenant ID |
   | `AZURE_CLIENT_ID` | the app registration's application (client) ID |
   | `AZURE_CLIENT_SECRET` | the client secret you created |
   | `SIGNING_ENDPOINT` | the account's endpoint, e.g. `https://eus.codesigning.azure.net/` |
   | `SIGNING_ACCOUNT` | the signing account name |
   | `SIGNING_PROFILE` | the certificate profile name |

6. **Tag a release.** The next `v*` tag signs. Check the *Verify the
   signatures* step in the Release run: it prints the status and signer
   subject for each binary.

## What to expect afterwards

- A signed binary still gets a SmartScreen prompt until the certificate has
  accumulated reputation with Microsoft. With Artifact Signing that happens
  quickly because the identity is already Microsoft-validated; with a
  traditional OV certificate it can take weeks of downloads. There is no way
  to buy instant reputation short of an EV certificate on a hardware token,
  which costs several hundred dollars a year and does not fit an automated
  release pipeline well.
- The signature is RFC 3161 timestamped, so binaries stay valid after the
  short-lived Artifact Signing certificate rotates.
- Linux binaries are not signed; distributions verify by checksum, which the
  release already publishes.

## Alternatives considered

- **OV / EV certificate from a CA** (Sectigo, DigiCert, SSL.com): $200 to
  $600 per year, EV requires a hardware token or cloud HSM, and the CI story
  is worse. Only worth it if instant SmartScreen reputation matters more than
  cost.
- **Self-signed:** worthless for SmartScreen; do not bother.
- **Do nothing:** the current state. The README says the binaries are
  unsigned and tells users how to click through.
