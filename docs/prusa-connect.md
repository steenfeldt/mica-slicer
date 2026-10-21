<!--
SPDX-License-Identifier: AGPL-3.0-only
Provenance: original
-->

# Send via Prusa Connect (per-printer API key)

Mica supports the API-key uploader from pinned PrusaSlicer 2.9.6,
`prusaslicer@b028299c77`. The relevant class is **PrusaConnect in
`src/slic3r/Utils/OctoPrint.hpp/.cpp`**, not `PrusaConnectNew` in
`PrusaConnect.hpp/.cpp` (which uses Prusa Account OAuth).

## API and entry points

- `mica_printhost::PrusaConnect::new(PrinterConfig, Timeouts)` and
  `PrinterKind::PrusaConnect` through the existing `connect` factory.
- `CONNECT_URL = "https://connect.prusa3d.com"`; an empty address defaults to it.
  Other origins are refused, except numeric loopback addresses for mock testing.
- `PrinterHost::upload_action_with_progress(..., UploadAction, ...)` returns
  `UploadOutcome { job, message, warning }`. Actions are `UploadOnly`, `Print`,
  and `Queue`. Existing hosts delegate to their unchanged upload/start behavior;
  requesting queue on an unsupported host fails locally.
- The existing boolean `upload` / `upload_with_progress` methods remain usable:
  false means upload only, true means print requested. Use the extended method to
  receive acceptance messages and warnings. `Job::started` is always **false** for
  Connect: cloud acceptance cannot confirm physical printing.
- GUI: My printers → Settings → Add manually → Type → **Prusa Connect (API key)**.
  Create the key in Connect → select printer → Settings → API key. The key is
  saved through the same `SecretStore` / macOS Keychain service and reference as
  PrusaLink. No secret is serialized. The optional **Connect ID** is the ID from
  `/printer/ID/dashboard` in the printer's browser URL; it enables **Open in Prusa
  Connect**, including for a PrusaLink entry. It is not inferred from a secret or
  fetched through another API.
- Send dialog: **Send via Connect**, with upload only / upload and print / add to
  queue. Readiness, bed, tools and material remain explicitly unknown. HTTP 202
  produces a persistent warning notification, including the acceptance message.
  Other accepted responses produce an information notification.

CLI, in the existing isolated/configured `printers.toml`:

```toml
[[printers]]
name = "cloud-xl"
kind = "PrusaConnect"
address = "https://connect.prusa3d.com"
api_key_env = "MICA_CONNECT_KEY"
connect_printer_id = "printer-id-from-browser-url" # optional, not a credential
```

```sh
mica print job.bgcode --printer-host cloud-xl                 # print requested
mica print job.bgcode --printer-host cloud-xl --upload-only
mica print job.bgcode --printer-host cloud-xl --queue
```

The two optional flags are mutually exclusive. The CLI keeps its existing
credential policy: environment-variable references only; a Keychain-backed entry
reports that CLI Keychain access is not implemented. It does not read the Keychain.
`mica printers status` is unsupported for Connect and makes no request.

## Exact outbound requests for review

All requests use the configured origin, normally `https://connect.prusa3d.com`.
TLS verification is enabled; redirects and proxies are disabled by the existing
transport. There is no automatic job retry, status polling, account discovery,
camera query, scraping, upload registration or follow-up start request.

Every send first makes exactly one credentialed version request:

```http
GET /api/version HTTP/1.1
Host: connect.prusa3d.com
X-Api-Key: <per-printer Connect key>
Accept: */*
User-Agent: ureq/3.4.2
Content-Length: 0
```

The body has zero bytes. As upstream does, Mica requires the JSON `api` field (a string) and
`text` beginning with `PrusaLink` or `OctoPrint`. An absent/false
`capabilities.upload-by-put` selects POST. Invalid responses or non-2xx statuses
stop the send before uploading.

The normal POST path is identical for all three actions:

```http
POST /api/files/local HTTP/1.1
Host: connect.prusa3d.com
X-Api-Key: <per-printer Connect key>
Accept: */*
User-Agent: ureq/3.4.2
Accept-Language: en
Content-Type: multipart/form-data; boundary=<B>
Content-Length: <exact framing bytes plus file byte length>
```

`B` is Mica's existing Moonraker multipart boundary generator (`mica-<pid hex>-<sequence
hex>`), checked against file contents to avoid collisions, with bounded memory.
Each line ending below is CRLF except the file bytes, which are streamed unchanged.
For remote name `folder/test name.bgcode` the complete multipart body is:

```text
--<B>\r\n
Content-Disposition: form-data; name="<action-field>"\r\n
\r\n
True\r\n
--<B>\r\n
Content-Disposition: form-data; name="path"\r\n
\r\n
folder\r\n
--<B>\r\n
Content-Disposition: form-data; name="file"; filename="test name.bgcode"\r\n
Content-Type: application/octet-stream\r\n
\r\n
<exact file bytes>\r\n
--<B>--\r\n
```

| Option | Action field | Other fields |
|---|---|---|
| Upload only | Entire first action part omitted | `path`, `file` |
| Upload and print | `to_print` with case-sensitive value `True` | `path`, `file` |
| Add to queue | `to_queue` with case-sensitive value `True` | `path`, `file` |

For file length `N`, UTF-8 directory length `P`, basename length `F`, and boundary
length `b`, the exact POST body size is `N + P + F + 3b + 165` bytes for upload only,
or `N + P + F + 4b + 226` bytes for either action. For the test fixture above
(`N=10`, `P=6`, `F=16`), that is `197 + 3b` or `258 + 4b` bytes.

Header names are case-insensitive and may be serialized in lowercase by ureq. No
Authorization, Cookie or Accept-Encoding header is added.

No `print`, `select`, `to_print=False`, or `to_queue=False` fields are sent.
A root-level remote filename sends `path` as an empty string; the file part always
uses the basename. `Content-Length` includes every boundary, header and CRLF;
progress counts only file bytes. The mock tests compare the whole body byte-for-byte
and assert that recorded body length equals Content-Length for every action.

If the version response explicitly advertises PUT, upstream's alternate request is:

```http
PUT /api/v1/files/local/folder/test%20name.bgcode HTTP/1.1
Host: connect.prusa3d.com
X-Api-Key: <per-printer Connect key>
Accept: */*
User-Agent: ureq/3.4.2
Content-Type: text/x.gcode
Overwrite: ?1
Content-Length: <file byte length>
Print-After-Upload: ?1
```

The body is just the file bytes. `Print-After-Upload` is sent **only** for print
requested; upload only omits it. No Accept-Language or multipart fields are sent
on PUT. **Queue with PUT is refused before sending file bytes**: upstream's PUT
implementation silently ignores QueuePrint, so copying that behavior would
misrepresent the action. The user sees an explicit explanation.

Manual **Test connection** makes only the version GET. Selecting/viewing a cloud
printer and opening the send dialog make **zero** requests. Opening the printer
page delegates a credential-free URL to the external browser; no key is appended.

## Responses and simplifications

Ported/adapted: `PrusaConnect::set_http_post_header_args`, action choices from
`OctoPrint.hpp::PrusaConnect`, `PrusaLink::test_with_method_check`, `post_inner`,
and `put_inner`, including empty parent paths, filename versus URL path encoding,
202 warning distinction and PUT's header-presence semantics.

Mica uses its existing streamed Rust HTTP transport, timeout/error types, progress
callback, Keychain store, configuration and GUI worker threads. Successful
response text (including plain text, not only JSON) is shown after stripping control
characters, redacting the key, and limiting display to 2,048 characters; the
response read is bounded at 2 MiB. Empty 202 bodies get a clear fallback warning.
401/403 are credential failures, 409/423/429 are busy/conflict, and network failure
uses the existing unreachable/timeout error. Error bodies are not echoed, preserving
Mica's existing secret-free diagnostics. After an ambiguous failure, the UI advises
checking Connect before retrying to avoid duplicate jobs.

Simplified/left out: translated Accept-Language (fixed English), custom CA and Windows
resolved-IP workarounds, OAuth/account login, hosted picker, team/storage selection,
remote status/controls/preflight, cameras, fleet/queue management and delayed scheduling.
Those are browser functions.

## Where the code is

- `mica-printhost/src/connect.rs`: protocol and fixed origin; `lib.rs`: action/outcome
  API and host factory; `http.rs`: status-preserving upload response and shared bounded
  body reader; `moonraker.rs`: visibility-only reuse of multipart boundary helper;
  `discovery.rs`: cloud transport is never probed during LAN discovery.
- `mica-gui/src/printhost/{config,mod,job,explain}.rs`: type, non-secret Connect ID,
  existing credential store, explicit cloud stage, warning propagation, no watcher.
- `mica-gui/src/panels/{printers,print,machine,dashboard,add_printer}.rs`: settings,
  send choices, browser link and honest cloud status labels.
- `mica-cli/src/{printer_cmd,lib}.rs`: kind, secret references, action flags and help.
- `mica-printhost/src/{tests,connect_tests}.rs`: recording mock HTTP server/transport;
  GUI and CLI unit tests cover credential references, cloud state and warnings.
- `PROVENANCE.md`: registers for the upstream-derived uploader and the printer-page
  URL pattern in the GUI config (the only use of `PrusaConnectNew::upload`).

Points to check against the live service: version validation, multipart action spelling/casing
and the empty path, the rare PUT negotiation (and explicit queue refusal), 202 acceptance versus
actual printing, response text redaction, and the per-printer key and Connect ID pairing.
The HTTP mock fixture falls back to an in-memory HTTP transport where binding loopback sockets
is not allowed. All test credentials are synthetic;
credential-store tests use MemoryStore. No real service or printer was contacted.

## Verification

- `cargo test --offline -p mica-printhost`: 43 passed, including five Connect request tests;
  the focused GUI Connect tests and the CLI Connect config/flags test pass.
- `cargo xtask corpus --quick`: 14 cases pass, zero validation errors.
- cube20 and 3DBenchy for CORE One, XL and U1 are byte-identical before and after the
  implementation; `mica validate --json` reports zero errors for all six.
- No real service, printer or Keychain is accessed by the tests.
