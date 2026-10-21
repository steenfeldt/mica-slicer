# Printer connections

`mica-printhost` is a blocking library with no GUI, persistence, background polling or real-printer
tests. The caller supplies a `PrinterConfig`, `Credentials` and `Timeouts`, then calls `connect` to
obtain a `Box<dyn PrinterHost>`. Concrete `PrusaLink`, `PrusaConnect`, `Moonraker` and `SnapmakerU1` constructors are
also public. Run calls on a worker thread in the future GUI.

`PrinterHost` exposes capabilities, information, status, upload, upload with a progress callback,
start, pause, resume, cancel, loaded filaments, camera snapshot URL and advisory preflight. Tool
indices are **zero based**, temperatures Celsius, durations seconds, progress **0–100 percent**.
Optional values remain `None`; unsupported/unknown loaded filaments return an empty vector. A
network failure is a typed error, not a fabricated idle status. Reported offline Klippy states map
to `Offline`; unrecognized printer states map conservatively to `Error`.

## Capabilities

| Capability | PrusaLink v1 (CORE One / XL / MK4) | Legacy PrusaLink | Moonraker | Snapmaker U1 |
|---|---|---|---|---|
| Upload / start / pause-resume / cancel | Yes | Not implemented | Yes | Moonraker |
| Status / info | Yes | Yes, reduced fields | Yes | Moonraker, U1 model label |
| Loaded material / colour | Optional slot fields | No | No | Optional `print_task_config` |
| Snapshot URL | No advertised Buddy endpoint | No | Optional webcam API | Moonraker webcam API |
| File browsing / queue | No | No | No | No |
| Formats (preferred first) | `.bgcode`, text G-code | Text G-code | Text G-code | Text G-code |

`files` and `queue` are reserved capability flags and are false: this crate implements sending jobs,
not file browsing or queue management. Camera and filament flags become true after those optional
features have actually been observed. Legacy detection disables v1 control capability flags. A new
PrusaLink instance initially advertises v1; call `info`/`status` to negotiate before presenting it.
Discovery is cached for a Moonraker connection; reconnect after changing Klipper's configured tools.

Prusa upload preserves the supplied bytes and filename. It does not convert text to binary or rename
an existing file. Mica's existing CORE One/XL slice defaults already produce `.bgcode`; pass that
output to `mica print`. Use `.gcode` for U1. Acknowledged upload is distinct from acknowledged start:
Moonraker's `print_started` response sets `Job.started`; it is never inferred from the request alone.
PrusaLink's successful `Print-After-Upload` request is treated as acceptance of that action.

## Transport and authentication

Addresses accept `host[:port]` or HTTP(S) URLs, including a reverse-proxy path prefix. URL credentials,
query strings and fragments are rejected. TLS certificate verification stays enabled. Redirects are
not followed and environment proxies are disabled so LAN keys cannot be forwarded elsewhere.
Credentials are redacted in `Debug`; diagnostics contain neither response bodies nor secrets.

PrusaLink API keys use `X-Api-Key`. Older firmware can use username/password digest authentication
(MD5 or MD5-sess, `qop=auth` or the older no-qop form). SHA-256 and auth-int challenges return a
protocol error. A challenge is answered once; bad credentials return `Unauthorised`. Upload obtains
a challenge with a read-only `GET /api/printer` before sending the file; verify this protected endpoint
on older firmware. No upload/control command is retried after a transport failure. Secrets are held
only by the connection and caller; the crate never saves them. Moonraker supports an optional
`X-Api-Key`, or no credentials; Prusa digest credentials are rejected for Moonraker.

Defaults: 5 seconds to connect, 10 seconds for an ordinary request, 60 seconds for the upload request
(including its response). All three can be changed through `Timeouts`; zero is rejected. Uploads
stream file bytes, use explicit Content-Length and report file bytes read to the network through
`UploadProgress { sent, total }`. Progress reaching 100% is **not** printer acknowledgement. The
callback runs synchronously on the calling thread and must not panic or block. Multipart overhead
is excluded from progress. A bounded preliminary scan chooses a delimiter absent from the file.
Do not modify the local file during upload. Printer JSON responses are limited to 2 MiB.

Errors have `ErrorKind::{Unauthorised, Unreachable, Busy, Protocol}` and an optional HTTP status.
401/403 are auth errors; 409/423/429 are busy; connection failures/timeouts are unreachable. Unknown
JSON fields are ignored; missing required state/result fields are protocol errors.

## Endpoints and evidence

This was implemented **offline**, without a printer or a locally available PrusaLink OpenAPI file.
[`upstream-map.md`](upstream-map.md) was the starting feature map.
Representative HTTP fixtures exercise the protocol; they are **not hardware verification**.

### PrusaLink

- `GET /api/v1/info`, `GET /api/v1/status`.
- `GET /api/v1/job` supplies the current id for controls, and filename if status omits it.
- `PUT /api/v1/files/usb/{encoded/path}` streams bytes with `Print-After-Upload: ?1` (or `?0`) and
  `Overwrite-File: ?1`. Existing same-name files are intentionally overwritten.
- `POST /api/v1/files/usb/{encoded/path}` with `{"command":"start"}` starts an uploaded file.
- `PUT /api/v1/job/{id}/pause`, `PUT /api/v1/job/{id}/resume`, `DELETE /api/v1/job/{id}`.
- 404/405/501 on v1 reads allow legacy `GET /api/version` and `GET /api/printer` fallback. 401,
  timeout and malformed JSON **do not** trigger a fallback. Legacy controls/uploads are left out.

The endpoint contract follows the documented Buddy API. Compare against
[Prusa-Link OpenAPI](https://github.com/prusa3d/Prusa-Link/blob/master/prusa_link/static/openapi.yaml)
and the exact installed **Buddy firmware** before relying on firmware-specific fields. The Linux
Prusa-Link server and embedded Buddy implementations are not guaranteed identical.

Status parses `printer.state`, `temp_nozzle`/`target_nozzle`, `temp_bed`/`target_bed`, optional
`temp_chamber`/`target_chamber`, `axis_z`; job progress, `time_printing`, `time_remaining`, optional
`current_layer`, filename aliases. Aggregate nozzle telemetry is exposed as T0 when no slot data
exists; it is **not** taken as proof of the complete tool inventory.

**Unconfirmed optional Buddy 6.x/7.x shapes:** `tools` or `slots` arrays/maps at the root, or
`printer.tools`; numeric map keys `1`…`5` are treated as one based, `tool0`… as zero based. Per-slot
`temp_nozzle`/`target_nozzle` or `temperature`/`target`, `material`/`filament_type`,
`colour`/`color`/`filament_colour` are tolerant aliases. Confirm the actual XL payload and numbering;
when these fields are absent the inventory/material/colour stays unknown. No fallback invents PLA,
a colour or five tools. Plate type is used only if `info.bed_type` explicitly exists (also unconfirmed).

### Moonraker / Klipper

- `GET /server/info`, `GET /printer/info` (Klippy readiness and firmware; hostname is used as the
  model label when generic Moonraker has no physical-model field).
- `GET /printer/objects/list`: exact `extruder`, `extruder1`, … names discover installed tools;
  `extruder_stepper` and similarly prefixed non-heater objects do not count.
- `GET /printer/objects/query?…`: only discovered objects are queried. Includes `print_stats`,
  `virtual_sdcard`, discovered extruders, `heater_bed`, `motion_report` and chamber-named
  `temperature_sensor`, `temperature_fan` or `heater_generic` objects when present.
- `POST /server/files/upload`: streamed multipart with `root=gcodes`, `path`, optional `print=true`,
  and the `file` part. Accepts direct upload-result JSON or a `result` envelope.
- `POST /printer/print/start` with JSON `filename`; `POST /printer/print/pause|resume|cancel`.
- Optional `GET /server/webcams/list` supplies the first enabled camera's `snapshot_url`. A leading
  slash resolves against the server origin; absolute HTTP(S) URLs are preserved. No camera port,
  path or streaming protocol is guessed. A webcam service may require its own authentication.

These are conventional Moonraker HTTP APIs; no online specification was fetched. Status maps
`print_stats.state`, `filename`, `print_duration`, `info.current_layer`, `virtual_sdcard.progress`,
per-heater `temperature`/`target`, and `motion_report.live_position[2]`. Remaining time is an
**estimate** from elapsed time and file progress, which can be inaccurate while heating or early in
a print. Raw chamber sensors have no invented target. Arbitrarily named chamber sensors, file
metadata time estimates, websockets and object subscriptions are left out.

### Snapmaker U1

The U1 delegates upload, job controls, status, tool discovery and camera discovery to Moonraker.
The following is confirmed **in the pinned source**, not against a real printer's firmware:

`orca@8500fcdcca src/slic3r/Utils/SnapmakerPrinterAgent.cpp::fetch_filament_info` uses
`GET /printer/objects/query?print_task_config&filament_detect`, reads parallel
`filament_exist`, `filament_type`, `filament_sub_type`, `filament_color_rgba`, `filament_vendor` arrays,
and optionally NFC `filament_detect.info`. Mica queries these objects only when discovered, reads
present-slot material/RGBA values, and keeps missing/short-array values unknown. It does not parse
NFC temperature recommendations, concatenate subtypes, match presets, infer absent slots, or use
proprietary control endpoints. Missing extension objects/fields or 404/405/501 degrade to an empty
filament result; auth/connection failures remain visible. Without `filament_exist` a slot with a
type counts as loaded. The camera uses the standard optional webcam API; U1 camera support has not
been confirmed. No RFID data is synthesized.

**Starting with a slot map** (`SnapmakerU1::start_with_mapping`, `src/u1.rs`): the G-code names
T0…T3; the user (or the proposal, `src/u1_mapping.rs`, ported from Snapmaker Orca's
`compute_color_match`: same type, then nearest CIEDE2000 color) picks the slot for each used tool.
Mica uploads with `print` unset, reads the firmware version (`GET /machine/system_info` →
`system_info.product_info.firmware_version`, else a plain dotted version in `/printer/info`), then:

- **1.3.0 and later:** `server.files.start_local_print` over the Moonraker WebSocket
  (`ws://<address>/websocket`; Snapmaker's Moonraker does not offer it over HTTP), params
  `{type:"gcode", path, print_plate:1, options:{bed_level, flow_calibrate, time_lapse_camera,
  map_table:[[tool,slot],…], flow_calibrate_extruders:[slot,…]}}`, flags as 0/1.
- **Below 1.3.0 or unknown:** one `POST /printer/gcode/script` with `SET_PRINT_EXTRUDER_MAP
  CONFIG_EXTRUDER=<tool> MAP_EXTRUDER=<slot>` per tool, `SET_PRINT_USED_EXTRUDERS EXTRUDERS=<slots>`
  and `SET_PRINT_PREFERENCES BED_LEVEL=0|1 FLOW_CALIBRATE=0|1 TIME_LAPSE_CAMERA=0|1
  [FLOW_CALIBRATE_EXTRUDERS=<slots>]`, then `POST /printer/print/start`.
- **Fallback:** a `start_local_print` answering `state: "failure"` (as Snapmaker's app does), a
  "method not found" (-32601) or no WebSocket at all uses the macro route. A refusal
  (`state: "error"`, the printer's message shown) or an unanswered request is reported and never
  retried, so a job cannot start twice. Every error after the upload says the file is on the printer.

## Preflight

`host.preflight(&JobRequirements { tools, format, bed_type, needs_chamber_sensor }) -> Vec<Issue>`
performs only reads. A job describes zero-based tools and optional per-tool material/colour. Checks
cover reachability/auth, idle/finished state, known tool inventory, supported file format, reported
plate type, known material/colour and an optional chamber-sensor requirement. Unknown tools, plates
or requested filament attributes produce explicit warnings. Material comparisons ignore case;
opaque RRGGBBAA colours match RRGGBB, with or without `#`. Parsed colours compare within Mica's
CIEDE2000 tolerance; other colour strings use normalized text.

Issues are advisory; they never block by themselves. `mica print` prints them and attempts the
requested upload/start, so the printer remains authoritative about busy state. There is necessarily
a race between preflight and upload. No automatic pause, cancel, tool change, heat or motion command
runs during preflight. These are original Mica requirement checks: T0 must appear in the reported
inventory just like every other tool; a chamber requirement needs an observed temperature.
No tool macros are required. Read failures retain earlier warnings and stop later reads.

## CLI configuration

On macOS: `~/Library/Application Support/Mica Slicer/printers.toml`. Other platforms use Mica's
existing config directory. `MICA_CONFIG_DIR` overrides the directory for testing or a separate setup.

```toml
[[printers]]
name = "coreone"
kind = "PrusaLink"
address = "coreone.local"
api_key_env = "MICA_COREONE_KEY"

[[printers]]
name = "xl"
kind = "PrusaLink"
address = "http://xl.local"
username = "maker"
password_env = "MICA_XL_PASSWORD"

[[printers]]
name = "u1"
kind = "SnapmakerU1"
address = "http://u1.local:7125"
# api_key_env = "MICA_U1_KEY" # only when Moonraker requires it
```

`kind` is exactly `PrusaLink`, `PrusaConnect`, `Moonraker` or `SnapmakerU1`. Inline secret fields, unknown fields,
duplicate names, mixed credential mechanisms and missing/empty secret environment variables are
errors. The GUI writes the same file (see [gui-printers.md](gui-printers.md)): its entries carry
`keychain = "reference"` (the macOS Keychain item holding the key or password), optionally
`profile = "Vendor/Preset"` and `model = "…"`. The CLI accepts those fields but cannot read the
Keychain yet and says so; add `api_key_env` / `password_env` for command-line use. No config or
secret is written by these commands.

```sh
mica printers status coreone
mica print cube.bgcode --printer-host coreone
mica print cube.gcode --printer-host u1 --upload-only
```

`print` requests printing after upload unless `--upload-only` is present.
Prusa Connect also supports `--queue`; acceptance never confirms that printing started.
See [Prusa Connect](prusa-connect.md) for its API-key setup, exact requests, warning
handling and browser link. Connect has no status polling or native printer controls. Remote name is the local basename.
The CLI scans text G-code or checked, bounded binary G-code blocks for T selections, optional
filament-type/colour metadata and bed type. Absent selections default to T0; startup special T1000
and unload T-1 are ignored. It does not infer materials from motion. Some generated Mica text files
have no filament metadata, so only their observed tools/format can be checked. `needs_chamber_sensor`
is available to API callers; the CLI does not infer it from arbitrary custom G-code.

## Automated and manual verification

`cargo test --offline -p mica-printhost` uses original realistic responses and a local std
`TcpListener` server with exact request-body/header assertions. If the operating system
denies a loopback bind, the **same assertions** run through a test-only in-process ureq byte
transport. It exercises the actual HTTP encoder/decoder, digest, streaming and configured timeout
budgets, but cannot verify OS TCP behavior. No socket test is silently skipped, and production
builds contain no mock transport.

Coverage includes PrusaLink byte-exact binary/text upload, multipart text upload, start/pause/resume/
cancel, API key/no-auth/digest and rejected credentials, legacy fallback, five XL tools, four U1
heaters, optional U1 filament data, missing extensions/camera, status/progress/error states,
malformed JSON/API errors, busy/auth/timeout/unreachable errors, input rejection, upload progress,
and read-only preflight matches/mismatches/unknowns. Digest math also has RFC known-answer vectors.

Before relying on a printer connection, check each printer with a small known-good job:

1. **CORE One:** read info/status with the actual API key; record firmware and sanitized response
   shapes. While idle, upload `cube.bgcode` with `--upload-only`; confirm binary recognition and file
   bytes/name on USB. Upload/start the cube, confirm progress and temperatures; pause/resume and
   cancel it. Try an invalid key and verify there is no job mutation. Verify the filename lookup
   and v1 start JSON/method against the firmware. If using digest, test its challenge endpoint too.
2. **XL 5T:** confirm discovered slot numbering and all five heaters on the actual Buddy version.
   Load known materials/colours and compare `loaded_filaments`, including an empty/unknown tool.
   Check a job using T0 and T4, then one naming a nonexistent tool; only the latter should warn
   missing-tool. Check material/colour mismatches and a plate-type warning without modifying the
   printer. Upload/start a known multi-tool `.bgcode`, then pause/resume/cancel. Never infer all
   five tools from aggregate nozzle telemetry alone.
3. **U1:** read objects/info/status on its actual Moonraker address. Confirm `extruder`, `extruder1`…`extruder3` naming,
   `print_task_config` array-index mapping, RFID and non-RFID spools, absent/empty slots, and raw RGBA
   values. Verify webcam discovery or explicitly record it unavailable. Upload a text cube first
   without starting, then start it and verify Moonraker acknowledges `print_started`; exercise
   pause/resume/cancel. Confirm `.bgcode` produces a preflight warning and a local upload-format
   error. Do not use Orca multi-tool CLI oracle runs without explicit filament vectors.
4. **All three:** simulate unreachable address/timeouts and busy state; ensure errors/warnings are
   readable and contain no secret. Review overwriting a same-name file and preflight's advisory
   behavior. Confirm no printer config/secrets were created on disk.

Hardware response captures and physical printing remain manual checks. The parts most likely to
differ between firmware versions: the optional Buddy slot schema, the digest handshake on old
firmware, the U1 slot mapping and the camera service.

## Results for this change

- CORE One and XL 5T cube20 and 3DBenchy **text G-code** is byte-identical before and after this
  change; the binary exports match after normalizing only the `Produced on` timestamp and its
  metadata block's CRC.
- `mica validate` reports zero errors on all eight exports (text and binary).
- `cargo xtask corpus --quick`: 11 applicable cases passed with zero validation errors.
- No real printer was contacted by the tests.

## Implementation references and polling policy

The Moonraker adapter was reimplemented using the public
[object list/query API](https://moonraker.readthedocs.io/en/latest/external_api/printer/#printer-status-requests),
[file upload and metadata API](https://moonraker.readthedocs.io/en/latest/external_api/file_manager/),
and [webcam list API](https://moonraker.readthedocs.io/en/latest/external_api/webcams/).
Pinned OrcaSlicer `Moonraker::upload`, `MoonrakerPrinterAgent::query_printer_status` /
`fetch_object_list`, and PrusaSlicer `PrusaLink::post_inner` provide the attributed slicer
references. Snapmaker Orca's `Moonraker::get_machine_info` and upload path were reviewed for
U1 compatibility; their preprint dialog, MQTT and command execution are not used here.

Mica retains GET queries of discovered objects in sorted order, the first chamber sensor with
an available reading in discovery order, fractional progress clamped to 0–100%, and an estimate
of remaining seconds `elapsed * (100 - progress) / progress` only between 0 and 100%.
Missing hotend readings remain unknown temperatures on the discovered tool; absent bed/chamber
readings remain absent. Uploads stream to the `gcodes` root with a directory field and a filename;
only a requested start sends `print=true`. A returned `print_started=true` confirms the start.
Mica accepts both a direct upload receipt and a `result` envelope and does not issue a second
start request. The public Rust API, diagnostics and warning order stay unchanged.

Polling in `mica-gui/src/printhost/poll.rs` is original Mica scheduling policy. A visible
printing or paused job refreshes every 2 seconds so the Machine view feels responsive;
an idle card refreshes every 10 seconds to reduce unnecessary requests. A hidden running job
keeps a 10-second watch for completion/error notifications; other hidden printers do not poll.
Failures wait 15, 30, then at most 60 seconds. Minimising clears the ticker and suspends status
requests; re-enabling it polls immediately. Changing state alone keeps the pending deadline;
the new interval applies after the next poll. Camera frame budgets are independent and unchanged.
