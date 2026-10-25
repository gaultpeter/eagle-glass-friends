# Signing and Windows trust

Two independent Microsoft systems decide whether `eglass.exe` runs quietly.
Fixing one does not fix the other.

| What the user sees | System | Fixed by |
| --- | --- | --- |
| "Windows protected your PC" / "Unknown publisher" | SmartScreen | A valid Authenticode signature (plus reputation) |
| "Operation did not complete successfully because the file contains a virus" | Defender antivirus | A false-positive submission, not a signature |
| "This app can't run on your PC" | Architecture / OS | Build for x64, target Windows 10+ |

A signature makes Defender *more* likely to trust the file and makes a false-positive
report taken seriously, but it never removes an active detection. That needs the
submission described below.

## What is already in this repository

These are done and need no action:

- `build.rs` embeds a version resource (`FileDescription`, `ProductName`,
  `CompanyName`, `LegalCopyright`, `OriginalFilename`), the application icon,
  and an `asInvoker` + `longPathAware` manifest into `eglass.exe`.
- The window itself gets the same icon at runtime, so the taskbar and Alt-Tab
  entry are not a blank generic window.
- `scripts/sign.ps1` signs and verifies, and no-ops cleanly when no
  credentials are configured.
- `scripts/verify-release.ps1` checks the metadata above, optionally requires a
  valid signature, and writes `SHA256SUMS.txt`.
- `.github/workflows/release.yml` builds on a tag, signs through whichever
  provider is configured, verifies, and publishes the exe plus its hash.
- `.github/workflows/ci.yml` builds and checks packaging on every push.

Anonymous, resource-less binaries are exactly what heuristics score badly.
Those steps are cheap and they are already done.

## Choosing a signer

| Option | Cost | Publisher shown | SmartScreen | Best for |
| --- | --- | --- | --- | --- |
| **SignPath Foundation** | Free | SignPath Foundation | Trusted immediately | Open-source projects with an OSI license — these two repos |
| **Azure Trusted Signing** | ~$10/month | Your verified identity | Trusted, builds reputation | A project you own and can verify an identity for |
| **OV certificate** | ~$200–400/year | Your organisation | Warns until reputation builds | A business with a track record |
| **EV certificate** | ~$300–600/year | Your organisation | Trusted immediately | A business that needs it on day one |
| **None** | Free | "Unknown publisher" | Always warns | Nothing |

Pick **one**. The workflow runs exactly one signing path, in the order
SignPath → Azure → PFX.

### Option A — SignPath Foundation (recommended here)

Free code signing for open-source projects, run by SignPath GmbH. The publisher
name on the signature is SignPath Foundation rather than your own, but the
signature is real, timestamped, and clears "Unknown publisher" — and it inherits
SignPath's existing reputation.

1. Apply at <https://signpath.org/> (Foundation programme). You will be asked for
   the repository URL, the OSI license (MIT here), and a short description.
2. Once approved, install the **SignPath GitHub App** on the repository.
3. In the SignPath portal create:
   - a **project** pointing at this repository,
   - an **artifact configuration** (XML) that selects `eglass.exe`,
   - a **signing policy** (typically "release" with manual approval).
4. Add these repository secrets (`Settings → Secrets and variables → Actions`):

   | Secret | Where it comes from |
   | --- | --- |
   | `SIGNPATH_ORGANIZATION_ID` | SignPath portal, organisation settings |
   | `SIGNPATH_API_TOKEN` | SignPath portal, user API tokens |
   | `SIGNPATH_PROJECT_SLUG` | The project you created |
   | `SIGNPATH_SIGNING_POLICY_SLUG` | The policy you created |
   | `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | The artifact configuration you created |
   | `SIGNPATH_CONNECTOR_URL` | `https://pipelineconnector.connectors.signpath.io/GitHubActions/GitHubCom` |

5. Push a tag. The workflow uploads the unsigned exe, submits the signing
   request, downloads the signed result back into `dist/`, and publishes it.

Nothing else needs to change.

### Option B — Azure Trusted Signing

Microsoft-run signing with no USB token, roughly $10/month.

Prerequisites, in order — this is where most attempts stall:

1. An Azure subscription.
2. **Identity validation** in the Trusted Signing resource. Individuals need a
   verifiable history (roughly three years); organisations need verifiable
   business records. This can take days.
3. A **Trusted Signing account** in a supported region.
4. A **certificate profile** of type *Public Trust* inside that account.
5. An app registration with the **Trusted Signing Certificate Profile Signer**
   role on the account, plus a client secret.

Then set these secrets:

| Secret | Value |
| --- | --- |
| `AZURE_TENANT_ID` | Directory (tenant) ID |
| `AZURE_CLIENT_ID` | Application (client) ID |
| `AZURE_CLIENT_SECRET` | Client secret value |
| `AZURE_TRUSTED_SIGNING_ENDPOINT` | The account endpoint, e.g. `https://weu.codesigning.azure.net` |
| `AZURE_TRUSTED_SIGNING_ACCOUNT` | Trusted Signing account name |
| `AZURE_TRUSTED_SIGNING_PROFILE` | Certificate profile name |

The workflow signs with `azure/trusted-signing-action` and timestamps through
`http://timestamp.acs.microsoft.com`.

### Option C — OV or EV certificate

Buy from a public CA (DigiCert, Sectigo, SSL.com, GlobalSign). Two facts matter:

- Since June 2023 the CA/Browser Forum requires private keys to live on
  FIPS 140-2 Level 2 hardware. You will get a USB token or a cloud HSM, **not**
  an exportable `.pfx`.
- EV gives immediate SmartScreen reputation. OV does not; it builds up over time
  and is tied to the certificate, so rotating it resets that history.

Because of the hardware requirement, CI signing normally goes through the CA's
own cloud service (DigiCert KeyLocker, SSL.com eSigner, Azure Key Vault) rather
than a PFX secret. Use the provider's GitHub Action or `signtool` plugin.

The `pfx` and `pfx-base64` modes in `scripts/sign.ps1` only work if your CA
still issued you an exportable software key. If yours did, set:

| Secret | Value |
| --- | --- |
| `WINDOWS_CERT_PFX_BASE64` | `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))` |
| `WINDOWS_CERT_PASSWORD` | PFX password |

## Signing locally

`scripts/sign.ps1` auto-detects the first configured method.

```powershell
# With a PFX
$env:WINDOWS_CERT_PFX          = 'C:\keys\eglass.pfx'
$env:WINDOWS_CERT_PASSWORD     = '...'
powershell -ExecutionPolicy Bypass -File scripts\sign.ps1 target\release\eglass.exe

# With a certificate installed in CurrentUser\My
$env:WINDOWS_CERT_THUMBPRINT   = 'ABCDEF...'
powershell -ExecutionPolicy Bypass -File scripts\sign.ps1 target\release\eglass.exe
```

It signs with SHA-256, timestamps with RFC3161 (falling back across three
timestamp servers), then verifies with both `Get-AuthenticodeSignature` and
`signtool verify /pa /all`. A signature that cannot be verified is treated as a
failure.

`signtool.exe` is located automatically from the Windows SDK
(`Windows Kits\10\bin\<version>\x64`), so no PATH setup is needed.

## Verifying any binary

```powershell
Get-FileHash .\eglass.exe -Algorithm SHA256
Get-AuthenticodeSignature .\eglass.exe | Format-List Status, StatusMessage, SignerCertificate
& "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe" verify /pa /all .\eglass.exe
```

`Status` must be `Valid`. `UnknownError` or `HashMismatch` means the file changed
after signing. `NotSigned` means no signature at all.

## Clearing a Defender detection

If Defender quarantines the exe, that is an antivirus detection, not SmartScreen.
Submit the exact file:

1. Go to <https://www.microsoft.com/en-us/wdsi/filesubmission>.
2. Sign in, choose **Software developer** (you are the author), and select
   **Incorrectly detected as malware**.
3. Upload the exe. If it exceeds the size limit, submit the SHA-256 and a
   download link from the GitHub release.
4. Include:
   - the detection name from Defender's history (e.g. `Trojan:Win32/Wacatac.B!ml`),
   - the SHA-256 from `SHA256SUMS.txt`,
   - the repository URL and a one-line description of what the program does,
   - confirmation that it is signed, if it is.

Typical turnaround is one to three days. Expect to resubmit after a detection
recurs; ML heuristics re-score new builds independently.

This program is a plausible false-positive target: it is a freshly compiled,
low-prevalence Rust binary that loads `libvlc.dll` at runtime, opens network
sockets, reads a credentials file, and in the friends build talks to a
Cloudflare Worker. None of that is malicious, and all of it is worth explaining
in the submission.

## SmartScreen reputation, precisely

- **Unsigned**: every download shows "Unknown publisher". There is no path to
  trust without a signature.
- **Signed with OV**: warnings stop only after enough clean downloads accumulate
  against that certificate. Small projects can wait a long time.
- **Signed with EV**: reputation is granted immediately.
- **SignPath Foundation / Azure Trusted Signing**: the signature chains to an
  identity with existing reputation, so warnings clear quickly.

Reputation is tied to the certificate and the file hash. Rotating the certificate
or shipping a new build starts the file-specific part over — which is normal and
expected, and why a stable signing identity across releases matters more than any
single build.

## Release checklist

1. Bump `version` in `Cargo.toml`.
2. `cargo build --release --locked` and run the app.
3. `powershell -File scripts\verify-release.ps1 -Path target\release\eglass.exe`
   — all metadata checks pass.
4. Commit, then `git tag vX.Y.Z && git push --tags`.
5. The release workflow builds, signs, verifies, hashes, and publishes.
6. Confirm the published asset reports `Status: Valid` and the hash matches.
7. If Defender flags it, submit that exact build for review.

## Gotchas

- **Sign last.** Any byte change after signing — re-stripping, patching
  resources, re-linking — invalidates the signature.
- **Always timestamp.** A signature without an RFC3161 timestamp stops validating
  when the certificate expires.
- **`strip = true` is fine.** Resources live in the PE `.rsrc` section, not in the
  symbol tables.
- **Builds are not bit-reproducible.** `rustc` output differs per build; publish
  the hash for each release instead of promising a stable hash.
- **Do not bundle VLC.** The app loads the system `libvlc.dll`; bundling a full
  player would multiply the size and the heuristic surface.
- **Antivirus on the build runner** can quarantine `dist/eglass.exe` mid-job.
  GitHub's `windows-latest` images do not, but self-hosted runners might.
- **One signer at a time.** Signing the same file with two different
  certificates produces a nested signature that some verifiers handle badly.
- **Optional hardening.** `video.rs` loads `libvlc.dll` by path. Adding
  `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)` and loading with
  an absolute path reduces DLL-search-order heuristics — but libVLC resolves its
  own plugin directory, so test playback thoroughly before adopting it.
