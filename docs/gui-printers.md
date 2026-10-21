# GUI: printers

Prusa Connect API-key sending is also available in My printers and the send dialog;
see [Prusa Connect](prusa-connect.md) for setup, queue options and the browser link.
Cloud connections do not join the LAN status monitors described below.

The printer workflow in the desktop GUI, on top of [printhost.md](printhost.md) (the
`mica-printhost` crate) and the earlier GUI rounds. Everything here is original Mica code; the
backend protocol logic stays in `mica-printhost` and is registered there.

## What it does

- **My printers** (`File › My printers…`, the `+` in the Machine view, "Add a printer…" in the
  Print dialog; `panels/printers.rs`): add / edit / remove physical printers with a name, type
  (PrusaLink, Klipper via Moonraker, Snapmaker U1), address, credentials (PrusaLink API key or
  user name + password; Moonraker optional API key) and the Mica profile the printer prints
  with. **Test connection** shows what the backend reports (model, firmware, state, tools, what
  it can do, loaded spools) or a plain-language error ("401: XL refused the credentials — check
  the API key in My printers against the printer's Settings → Network → PrusaLink → API key").
- **Secrets go to the macOS Keychain** (`printhost/keychain.rs`, `security-framework` 3.7.0):
  one generic-password item per printer under the service "Mica Slicer", the account being an
  opaque reference like `mica-printer-19c6…-3`. `printers.toml` (next to `mica.toml`) stores
  only that reference (`keychain = "…"`), never the key. On other platforms the store reports
  itself as "not supported yet" and the form says so instead of pretending to save. Tests and
  headless screenshots use an in-memory store, so they never touch the real Keychain or disk.
  The file is the CLI's: `mica print` still reads `api_key_env` / `password_env` entries, and the
  GUI reads them too; the CLI accepts the GUI's extra `profile` / `model` fields and says clearly
  that it cannot read the Keychain yet.
- **Profile link**: each physical printer may name a profile (`profile = "Prusa/Original Prusa
  XL 5T Input Shaper 0.4 nozzle"`); several printers can share one. Without a link the first
  printer of the same family whose reported model names the profile's model is used (an XL for
  the XL profiles, the U1 for the U1), else the first of the family (`config::ranked`).
- **Print…** (`panels/print.rs`, state machine in `printhost/job.rs`): the printer picker lists
  the ones fitting the current profile first ("For Original Prusa XL 5T…", then "Other
  printers"). Choosing one runs the preflight on a thread and shows: the printer's state (idle,
  or "Printing 3DBenchy.bgcode · 37 %"), its tools ("5 tools (T1–T5)"), the job's filament next
  to the loaded one per tool with colour swatches and a verdict (✓, "differs", "not reported",
  "no such tool"), the file format ("Binary G-code (.bgcode) · accepted"), and the warnings in
  plain words. For a **Snapmaker U1** the filament table becomes *Filament slots*: per used tool
  the project's swatch → a dropdown of the printer's four slots (color, type, slot number),
  proposed by type then nearest color; "type differs" is a warning, an empty slot an error that
  blocks the start. The U1 also gets *Bed leveling*, *Flow calibration* (with a per-tool choice
  when the job uses several tools) and *Timelapse*, defaulting to the printer's
  `print_task_config` and then to the last choice sent to that printer (`mica.toml`,
  `[u1_start."<name>"]`). Options: *Start printing after upload* (on by default) and the file name. Upload
  shows a progress bar ("Uploading 3DBenchy.bgcode · 1.2 MB of 2.4 MB"); success closes the
  dialog and shows a toast with an *Open Machine view* link. Errors are a headline plus what to
  do; "Try again" re-runs the check or the upload. Switching printers mid-check discards the
  stale answer (events carry a generation).
- **Machine view** (third header tab next to Prepare | Preview, `panels/machine.rs`): the
  printers on the left with a state dot, the chosen one on the right: name, model · firmware ·
  address, state chip, "updated 2 s ago"; the job card with name, progress bar, elapsed / left,
  layer and Z, Pause / Resume / Cancel (Cancel asks first, in a modal); per-tool temperatures
  with bars (XL 5, U1 4), bed and chamber; the camera snapshot where the backend reports one
  (refreshed every few seconds; PrusaLink has none and says so); the loaded spools with "Use
  these in the project". Live state comes from one monitor thread per printer
  (`printhost/monitor.rs`), polled at 2 s while printing or paused, 10 s idle, 15 s after a
  failure (`printhost/poll.rs`), and **not at all** while the view is hidden or the window is in
  the background; only the printer being looked at polls. Commands run even while hidden.
- **Filament sync** (`panels/sync.rs`): the Filament card gets a sync button when the profile
  has a linked printer; it sets each tool's colour (overlay `filament_colour`) and, when the
  printer reports a material the current preset is not, switches to a matching preset
  (`Generic PLA`-style lookup via `match_filament`). One undo step. Each tool row shows the
  printer's swatch next to the project's, with a dot only when they differ (amber: another
  filament type; accent: another color). Colors compare by CIEDE2000 (`mica_printhost::colour`,
  on the `u1_mapping` DeltaE00 port): below ΔE 6 they are the same spool color (`#FFFFFF`
  loaded for a `#F5F5F5` project is ΔE 2.0, `#000000` for `#1A1A1A` 5.5; yellow `#F8F81C` for
  gold `#F5D020` is 11.5 and is flagged). Hovering the printer swatch says what is loaded (hex
  codes only there); clicking it opens a popover ("Printer has yellow loaded in slot 1
  (project: gold)") with "Use printer's color" (or "Use printer's filament" for another type)
  and "Ignore" (until the printer reports another spool in that slot). One line above the list
  sums up ("2 colors differ from the printer · Sync all · Ignore"; muted for colors, amber for
  types). User-visible text names colors from a small curated palette (nearest by ΔE: white,
  off-white, gray, black, red, orange, yellow, gold, …) and never shows hex codes; the same
  applies to the print dialog's U1 slot dropdown ("Slot 1 · PLA Basic · yellow"), its filament
  rows and the Machine view's tool tiles ("PLA · yellow"). Loaded spools come from the monitor
  or the last test connection; the sync asks the printer first when nothing is known yet.
- **File header** (Moonraker and U1 printers, `printhost/file_header.rs`): see
  [gcode-config-identity.md](gcode-config-identity.md#per-printer-file-header).

## Mock printers

`crates/mica-printhost/examples/mock_printer.rs` serves realistic PrusaLink (XL 5T printing,
CORE One idle) and Moonraker (Snapmaker U1 printing, four slots, camera) responses on loopback,
with a fake job that advances with the clock and reacts to pause / resume / cancel / start:

```sh
cargo run -p mica-printhost --example mock_printer -- --xl 18081 --u1 18082 --coreone 18083 \
    [--api-key KEY] [--xl-idle] [--u1-idle] [--slow-upload] [--job-seconds N] \
    [--moonraker-legacy-slicers | --moonraker-modern] [--u1-slots PLA:F8F81C,PLA:FFFFFF,-,PETG:000000]
```

`/server/files/metadata` names the slicer of an uploaded file from its `; generated by` line:
`--moonraker-modern` (default) like upstream Moonraker since 4637d70 (any `NAME VERSION on`),
`--moonraker-legacy-slicers` like older forks such as the U1 firmware (a fixed slicer list,
otherwise `Unknown` without thumbnails, time or filament).

Screenshots and tests never contact real printers. GUI tests use an in-process fake
`PrinterHost` (`printhost::monitor::tests::Fake`).

## Screenshots (`out/work/gui13-*.png`)

```sh
cargo build --release -p mica-gui && cargo build --release -p mica-printhost --example mock_printer
# A printing XL + U1 + CORE One, and a second, idle XL for the dialog shots.
./target/release/examples/mock_printer --xl 18081 --u1 18082 --coreone 18083 --api-key MOCK-KEY --slow-upload &
./target/release/examples/mock_printer --xl 18091 --u1 off --coreone off --api-key MOCK-KEY --slow-upload --xl-idle &
B=./target/release/mica-gui; M=tests/models/3DBenchy.stl; P=--printer-host
XL='XL 5T=prusalink@127.0.0.1:18081#MOCK-KEY'; IDLE='XL 5T=prusalink@127.0.0.1:18091#MOCK-KEY'
U1='U1=u1@127.0.0.1:18082#MOCK-KEY'; C1='CORE One=prusalink@127.0.0.1:18083#WRONG'
$B --screenshot out/work/gui13-my-printers.png $M --xl $P "$XL" $P "$U1" $P "$C1" --my-printers --test-connection "XL 5T"
$B --screenshot out/work/gui13-print-dialog.png $M --xl $P "$IDLE" $P "$U1" --state sliced --print-dialog
# A bigger file than the loopback socket buffers, so the bar is caught mid-way (restart the idle XL first).
$B --screenshot out/work/gui13-upload.png $M --xl --set layer_height=0.05 --set fill_density=50% $P "$IDLE" --state sliced --print-dialog --upload --settle 1200
$B --screenshot out/work/gui13-uploaded-toast.png $M --xl $P "$C1" --state sliced --print-dialog --upload --settle 2500   # (with the right key)
$B --screenshot out/work/gui13-machine-xl.png $M --xl $P "$XL" $P "$U1" --machine-printer "XL 5T"
$B --screenshot out/work/gui13-machine-u1.png $M $P "$XL" $P "$U1" --machine-printer U1 --settle 4000
$B --screenshot out/work/gui13-filament-sync.png $M --xl $P "$XL" --test-connection "XL 5T" --crop 370
$B --screenshot out/work/gui13-filament-synced.png $M --xl $P "$XL" --sync-filaments --crop 370
```

New flags: `--printer-host NAME=prusalink|moonraker|u1@ADDRESS[#KEY]` (repeatable; in memory
only), `--my-printers`, `--test-connection NAME`, `--print-dialog`, `--upload`, `--machine`,
`--machine-printer NAME`, `--sync-filaments`, `--settle MS` (wall-clock time spread over twelve
passes so background answers land; printer flags default to 1500 ms).

## Tests

`cargo test -p mica-gui printhost`: the in-memory secret store and unique references; the
`printers.toml` round trip without secrets (inline secrets, duplicates and unknown keys refused
without echoing the line; the CLI's example parses); credentials resolved from the store, never
the file, with readable missing-secret errors; profile ranking (linked → model → family →
other); error explanations per backend; preflight rows and verdicts, busy state and format
mismatch, one-based tool names; polling intervals by state and visibility and the ticker;
the monitor thread polling only while an interval is set, running commands while hidden,
reporting and clearing failures, and an unconnectable printer; the dialog's stages (pick →
check → upload → done), readable failures and stale answers ignored. `mica-cli` accepts the
GUI's extra fields.

## Manual checks on real printers (not done here)

1. **My printers**: add the CORE One and XL with their real PrusaLink API keys (and, if used,
   user name + password) and the U1 with its Moonraker address (`host:7125`). Test each; confirm
   the reported model, firmware, tool count (XL: 5, U1: 4) and that "Can" lists upload / start /
   pause / resume / cancel. Check Keychain Access shows "Mica Slicer" items and that
   `~/Library/Application Support/Mica Slicer/printers.toml` holds only `keychain = "…"`
   references. Enter a wrong key once and confirm the 401 text.
2. **Print dialog**: with the printer idle, slice a small cube, open Print…, confirm the
   preflight reads idle, the right tool count, the loaded spools (XL slot data and U1
   `print_task_config` are the unconfirmed parts of the backend — compare the swatches with
   the physical spools) and the format (.bgcode on Prusa, .gcode on the U1). Upload with
   *Start printing after upload* off, confirm the file lands on the printer; then with it on.
   While a job runs, open Print… again and confirm the busy note and that a start is refused
   readably.
3. **Machine view**: watch a running job: progress, elapsed / remaining, layer, Z, the five XL
   / four U1 tool temperatures, bed and chamber; the U1 camera frame updating every few seconds.
   Pause, resume, cancel (confirm the modal). Switch to Prepare and check with the printer's
   own interface (or `tcpdump`) that polling stops; put Mica in the background and check the
   same; come back and confirm it resumes within a poll interval.
4. **Filament sync**: load spools the project does not match, confirm the chips, press sync,
   confirm colours (and presets where a Generic preset exists) follow the printer; undo once.

## Add printer and the My printers cards (addprinter)

The first iteration of the approved mockups (`Main`, `Gallery`, `Setup`, `MyPrinters`).
`Printer › Add printer…` (the File menu, the printer card's menu, My printers' `+ Add printer`)
opens one window with three screens (`panels/add_printer.rs`):

1. **Found on your network.** When the window opens (and on *Refresh*) Mica sends one mDNS /
   DNS-SD query (`mica_printhost::discovery`, own code over `UdpSocket`, no crate) for
   `_snapmaker._tcp` (the U1: TXT `machine_type`, `device_name`, `ip`; Moonraker is then asked on
   :7125, not the announced MQTT port 1884; the TXT `sn` / `userid` are dropped on arrival),
   `_moonraker._tcp` (Moonraker's zeroconf component), `_octoprint._tcp` (what Prusa-Link and
   PrusaSlicer's Bonjour browser use, TXT `model` / `version`), `_prusa-link._tcp` and
   `_http._tcp` (only answers whose name, host or TXT look like a printer, and only kept when a
   PrusaLink / Moonraker API answers: smart plugs named after printers are not). It is a legacy-unicast
   query from an ephemeral port, so Mica does not bind 5353 next to the system's responder;
   incomplete answers get one follow-up for SRV / TXT / A. Each answer is then asked, without
   credentials and with GETs only, what it is: PrusaLink `/api/version`, `/api/v1/info`,
   `/api/v1/status`; Moonraker `/server/info`, `/printer/info`, `/printer/objects/list` (a U1
   has `print_task_config` / `filament_detect`), `/printer/objects/query?…` and
   `?configfile=settings` for the nozzle. A 401 lists the printer as *Needs API key* (only when
   it was announced as a printer). Cards: product picture, name, model · tools · nozzle (matched
   to a profile by `printhost::catalog::Catalog::match_printer`), state, backend, address.
   *Add this printer* adds the matching profile and the connection in one click; a printer that
   wants its key opens Setup with the key field highlighted; one that matches no model opens the
   gallery and keeps its address for Setup. *Enter an address* looks one address up the same way
   (bare hosts also on :7125).
2. **Pick your printer.** One card per model (`Catalog::build` groups every system profile:
   tool counts `- 2T` / `5T` / `Dual`, MMU, input shaper, nozzle sizes and HF / HT types), with
   its picture, build volume and variant chips, a search field and brand chips. *All printer
   profiles…* opens the old full list (user and custom profiles).
3. **Set it up.** Picture and the three steps on the left; tools, input shaper (where the model
   comes both ways) and nozzle as segmented buttons, with only the combinations that have a
   profile enabled; the name; the optional connection (address, key, *Test connection* showing
   firmware, tools and loaded spools, or the plain-language error). Camera: Moonraker printers
   report theirs, which the test finds and previews; Prusa printers say "Camera via Prusa Connect
   (not supported yet)" and offer a camera address of the user's own (MJPEG / RTSP / snapshot)
   with *Test* and a preview. *Add <name>* writes what the old flow wrote: `my_printers` in
   mica.toml (and selects the profile), and with an address a `printers.toml` entry linked to the
   profile, its key in the Keychain.

**My printers** (`panels/printers.rs`) is now a grid of cards: picture (or, for a printer with a
camera that is online, a snapshot refreshed every 3 s while the window is open; clicking it opens
the Machine view), status badge (Ready / Printing 43 % / Paused / Offline / Slice only), progress
and time left while printing, loaded spools, temperatures, and *Print here* / *Watch* / *Retry* /
*Settings*. Profiles without a connection get *Connect…* / *Use* / *Remove*. *Settings* is the
former window (list, details, edit form, test, remove, `+ Add manually` for logins the new flow
does not cover). Status comes from the printers' existing monitors: the cards count as "on
screen" for `poll_printers`, and nothing polls once the window closes (unless a job runs, as
before).

Pictures: PrusaSlicer's `thumbnail` and Orca's `<model>_cover.png`, read at runtime from the
pinned upstream checkouts or installed slicers, never bundled (THIRD_PARTY.md).

Screenshots (mock printers with `--mdns`; temp `MICA_CONFIG_DIR`):

```sh
./target/fast/examples/mock_printer --xl 28081 --u1 28082 --coreone 28083 --rtsp 28554 --ipcam 28090 --mdns 25353 &
B=./target/fast/mica-gui; M=tests/models/cube20.stl
$B --screenshot out/work/gui-addprinter-found.png $M --add-printer found --mdns 127.0.0.1:25353
$B --screenshot out/work/gui-addprinter-gallery.png $M --add-printer gallery --add-printer-brand Prusa
$B --screenshot out/work/gui-addprinter-setup.png $M --add-printer setup --add-printer-model "Prusa XL" \
   --add-printer-tools 5 --add-printer-address 127.0.0.1:28081 --add-printer-test \
   --add-printer-camera http://127.0.0.1:28090/snapshot.jpg --settle 3000
$B --screenshot out/work/gui-addprinter-camera.png $M --add-printer setup --add-printer-model "Snapmaker U1" \
   --add-printer-address 127.0.0.1:28082 --add-printer-test --settle 3000
$B --screenshot out/work/gui-addprinter-myprinters.png $M --printer-host "XL 5T=prusalink@127.0.0.1:28081" \
   --printer-host "Snapmaker U1=u1@127.0.0.1:28082" --printer-host "Workshop CORE One=prusalink@127.0.0.1:28083" \
   --printer-host "Garage MK4S=prusalink@127.0.0.1:28099" --my-printers --settle 4000
```

Tests: `mica-printhost` `discovery_tests` (a compressed Avahi-style answer, truncations and a
pointer loop, `_http` filtering and merging, a sparse and a full fake responder on loopback, the
PrusaLink / 401 / U1 probes against the scripted HTTP mock: GETs only, no key sent);
`printhost::catalog` (grouping of XL, CORE One, MK4S, U1; matching found printers;
`base_name`); `printhost::discover` (no search without a loopback target in tests);
`add_printer_tests` (the screens and their buttons, Setup's prefs / printers.toml / Keychain
entries, one-click add and the key prompt, cards polling only while open).

Checked read-only against real printers on a LAN: the U1
announces only `_snapmaker._tcp` (fixture in `discovery_tests`), its Moonraker answers on :7125,
and its webcam list has an enabled WebRTC camera ("case", now played live over WebRTC, see below) and a
disabled one. None of `_moonraker`, `_octoprint`, `_prusa-link` was announced; the CORE One and
XL announced nothing (possibly switched off; other devices that announce `_http._tcp`
and are filtered out). For Prusa printers that do not announce, *Enter an address* is the way.

## Live WebRTC camera and faster snapshots (camera-streamer)

The U1's `case` camera is a `webrtc-camerastreamer` (stream `/webcam/webrtc`, snapshot
`/webcam/snapshot.jpg`); it has no MJPEG or RTSP. The Machine view now plays it live
(`mica-camera/src/webrtc/`, badge "LIVE · case · WebRTC"); dashboard thumbnails keep the
snapshot. Signalling is camera-streamer's own (its `html/webrtc.html`, also used by Mainsail):
three JSON POSTs to the stream URL,

```text
POST /webcam/webrtc  {"type":"request","keepAlive":true}
  → {"id":"rtc-…","type":"offer","sdp":"…","keepAlive":true,"iceServers":[…]}
POST /webcam/webrtc  {"type":"answer","id":"rtc-…","sdp":"<Mica's answer>"}
POST /webcam/webrtc  {"type":"remote_candidate","id":"rtc-…","candidates":[{"candidate":"candidate:… <LAN IP> <port> typ host","sdpMid":"…","sdpMLineIndex":0}]}
```

then ICE/DTLS/SRTP on one UDP port bound to the LAN address that routes to the camera. Only
host candidates on the LAN are contacted (the camera's own address when it offers it); no STUN or
TURN. Mica answers the `keepalive` channel's `ping` with `pong`, closes the session (DTLS
close_notify) when the picture is hidden, and decodes H.264 with the system decoder. Lost
packets are NACKed; a gap that stays skips frames until the next key frame (a PLI is sent;
camera-streamer itself ignores it and recovers at its next key frame). If WebRTC never delivers
a frame (404, no route, UDP blocked), the player shows the snapshots instead and says why ("WebRTC
did not connect (…); showing snapshots"). Other WebRTC services (go2rtc, MediaMTX, Janus) signal
differently and still show snapshots.

Snapshots (cameras without a stream, and the fallback): while the Machine view shows the
camera (also with the window in the background) they are fetched back to back, one request
at a time over a kept-alive connection, at most 4/s and at most the camera's `target_fps`;
pictures scrolled away or in a minimised window stop.

Mock (`--u1-camera webrtc` lists the real U1's three cameras; the `case` camera is a mock
camera-streamer encoding the simulated job live):

```sh
./target/fast/examples/mock_printer --xl off --coreone off --u1 28082 --rtsp off --ipcam off --u1-camera webrtc --camera-streamer 28091 &
MICA_CONFIG_DIR=$(mktemp -d) ./target/fast/mica-gui --screenshot out/work/gui-machine-webrtc.png tests/models/cube20.stl \
   --printer-host "Snapmaker U1=u1@127.0.0.1:28082" --machine-printer "Snapmaker U1" --settle 6000
# A player without the GUI (frames per second, for `ps` CPU figures); --serve needs --features mock,openh264.
./target/fast/examples/camera_probe http://127.0.0.1:28091/webcam/webrtc 15
```

Tests (`cargo test -p mica-camera`): end to end over UDP on 127.0.0.1 against the mock (the
three POST bodies, 320×240 frames decoded by VideoToolbox, 15 fps, pongs, the session ending
when the player goes), light loss repaired by NACKs, heavy loss recovering at key frames, the
fallback to snapshots on a 404, letting go while unwatched and reconnecting; candidate filtering
(LAN only, the camera's own address first, loopback only in test runs) and the crypto provider
(known vectors, fresh certificates); snapshot pacing (4/s cap, back-to-back for a 0.3 s camera,
one connection, pause when unwatched).

## Every brand in the gallery (allbrands)

The gallery lists every printer vendor of the OrcaSlicer `profiles` folder Mica uses
(`MICA_ORCA_PROFILES`, an installed OrcaSlicer, else the pinned checkout): RatRig, Voron, Bambu
Lab, Creality, Elegoo, Anycubic, Qidi, Sovol, Flashforge and ~55 more (Orca's own copy of the
Prusa profiles and its test printer are left out; Prusa comes from PrusaSlicer's bundle, whose
pinned settings repo has no other vendors). It stays fast:

- **Startup** loads only the PrusaSlicer bundle, Snapmaker (as before) and the Orca vendors
  that remembered printers name (My printers, the last selection, the default printer,
  `printers.toml` links: `Profiles::load_referenced`).
- **The gallery index** (`printhost::catalog::GalleryIndex`) reads each vendor's index and
  machine files only (`orca::Bundle::load_vendor_machines`, no process or filament files) on a
  thread when the Add printer window first opens, and caches the models in
  `cache/printer-gallery.json` next to `mica.toml`, keyed by the folder and each vendor
  index's size and time. Until it arrives the gallery shows the loaded brands and says
  "Reading every brand's printers…".
- **Picking a model** starts loading that vendor's full bundle on a thread
  (`Profiles::load_orca_vendor` / `install_vendor`); *Add* waits for it ("Loading RatRig
  profiles…").

Grouping: one card per model; Orca model names ending in a build size become the model's
**Size** choice (RatRig V-Core 3 / 4 and HYBRID 300 / 400 / 500, Voron 2.4 / Trident 250 / 300 /
350), an IDEX `COPY MODE` / `MIRROR MODE` a **Mode** choice (RatRig V-Core 4 IDEX), and presets
that would otherwise choose alike are told apart by their remaining words (Raise3D Pro3 Left /
Right / Dual). Brand chips: Prusa, Snapmaker, Bambu Lab, Creality, RatRig, Voron, then
alphabetical, as many as fit, and "+N more" for the rest. Search matches brand, vendor key
(`bbl`), model and sizes. Found Moonraker printers are matched against every brand (the U1
first). Bambu Lab printers can be added to slice; connecting them is not offered.

Orca-origin printers slice with OrcaSlicer rules (the mapping's default) and their own
`gcode_flavor` (Klipper for RatRig / Voron / Creality K). A machine without `default_bed_type`
starts on Orca's Cool Plate in the GUI as in the CLI; a numeric one (Elegoo, WonderMaker: `4`)
names Orca's `BedType` (Textured PEI Plate).

Profile import fixes found while loading every vendor (each as OrcaSlicer 2.4.2 reads it):
plain floats read their leading number (`"50%"` → 50, `"2mm"` → 2, `"0,0"` → 0:
`ConfigOptionFloat::deserialize`), `"0.5x0.5"` is a point, `handle_legacy`'s `zig-zag` →
rectilinear, `wall_direction` `auto` → ccw, negative `ironing_angle` → 0, and old percentage
speeds / `support_object_xy_distance` are dropped. Compatibility lists flatten and map the
printer / process once instead of per preset (Bambu Lab's 2 000 filaments: 127 s → 17 s for
all 48 printers). `cargo test -p mica-profiles --test orca_vendors -- --ignored --nocapture`
reports per vendor what still does not compose (option bounds Orca does not enforce on load,
BBL's multi-extruder-variant filaments, a few vendor gaps without a process).

Screenshots: `out/work/gui-addprinter-allbrands.png`, `gui-addprinter-ratrig.png`,
`gui-addprinter-ratrig-setup.png` (`--add-printer gallery [--add-printer-brand RatRig]`,
`--add-printer setup --add-printer-model "RatRig V-Core 4"`).

Tests: `printhost::catalog` (sizes / modes split, brands popular-first, RatRig / Voron /
Creality grouping, every variant resolving to itself, Moonraker matches across brands, the
index loading on a thread and its cache), `cli_parity_tests::gallery_printers_of_other_brands_…`
(RatRig V-Core 4, Voron 2.4, Creality K1 picked in the gallery, vendor loaded only then, added
slice-only, Orca rules, Klipper G-code byte-identical to `mica slice --vendor …`),
`mica-profiles` `orca_vendors` (every vendor's printers load and flatten machine-only).

## Left for later

- Prusa Connect (cloud) and the CORE One / XL camera through it; PrusaLink itself has no
  snapshot endpoint.
- Add printer: per-tool nozzles at setup, user + password (digest) logins (use `+ Add manually`
  under Settings), IPv6-only printers; slow vendors' print/filament lists (Bambu Lab: 2 000+ filaments) are composed on the UI thread when such a printer is selected.
- The CLI reading Keychain references (it says so clearly for now).
- Thumbnails of the job on the printer, file browsing and queues (the backend reserves them).
- Bed-type preflight for the U1 (the backend's `bed_type` is unconfirmed, so the dialog does not
  send one yet).
