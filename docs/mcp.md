# Mica MCP (`mica mcp`): color painting, in a file or in the open window

Mica MCP (`mica mcp`) is a local [Model Context Protocol](https://modelcontextprotocol.io)
server in the `mica` command-line binary, Mica's interface for AI assistants (Claude Code, Claude
Desktop, any MCP client). It will grow beyond painting; its first capability is color painting:
the assistant can open a model, look at rendered pictures of it, and color-paint it for a multi-tool printer by
pointing at pixels: "paint the hull red, the cabin blue and the chimney black" becomes a few
`render_view` → `paint_at` → `render_view` round trips. The result is a normal Mica project 3MF
with Mica's (PrusaSlicer/Orca compatible) facet painting.

Two modes. **Live**: when a Mica window is open, the tools work on the window's
project: the user watches every paint stroke, loaded model and slice appear, and undoes AI edits
with Edit › Undo like their own. **Headless**: one scene of the server's own, opened
from an STL or 3MF and saved to a new 3MF. By default `mica mcp` is live whenever a window answers
and headless otherwise; see [Live mode](#live-mode-the-open-mica-window).

## Setting it up

Build or install the `mica` binary (`cargo build --release -p mica-cli` gives
`target/release/mica`), then register it with the client. The server talks MCP over stdin and
stdout; it opens no network port.

**Claude Code**

```sh
claude mcp add mica -- /path/to/mica mcp
# for all projects instead of the current one:
claude mcp add --scope user mica -- /path/to/mica mcp
```

**Claude Desktop**: add an entry to `claude_desktop_config.json` (macOS:
`~/Library/Application Support/Claude/claude_desktop_config.json`; Windows:
`%APPDATA%\Claude\claude_desktop_config.json`) and restart Claude Desktop:

```json
{
  "mcpServers": {
    "mica": {
      "command": "/path/to/mica",
      "args": ["mcp"]
    }
  }
}
```

A model can be opened at start, with the same profile options as `mica slice`:
`mica mcp ~/Models/boat.stl --vendor Snapmaker --printer 'Snapmaker U1 (0.4 nozzle)'`.
Normally the AI calls `open` itself. `mica mcp --help` prints the usage.

Then ask, for example: *"Open ~/Models/3DBenchy.stl for the U1 with white, red, blue and black
loaded. Paint the hull red, the cabin blue and the chimney black, check it from a few sides,
slice it and save it as ~/Models/benchy-painted.3mf."*

## Tools

Tools are numbered from 1 like the printer's slots; tool 0 erases paint back to the part's own
tool. Mixed colors have virtual tool numbers after the physical slots and work with every
paint and assignment tool. `scene_info` lists them separately under `mixes`. Objects and parts are 1-based as
`scene_info` lists them; `object` may be omitted when the scene has one. Coordinates are bed millimetres, z up from the bed, −y is the front. Every tool
that changes the scene is one undo step; a call that changed nothing leaves none.

| Tool | Arguments | What it does |
|---|---|---|
| `open` | `path`, `printer?` (`"U1"`, `"XL"`, `"CORE One"` or a preset name), `vendor?`, `print?`, `filament?`, `bed?`, `filament_colors?` (names or `#RRGGBB` per slot) | Loads an STL/3MF as `mica slice --export-3mf` does (STLs centred on the bed; 3MFs keep their printer, settings and painting). Replaces the current scene; returns `scene_info`. |
| `scene_info` | – | Printer, tool count, filament per slot (`color` name plus `hex`, preset, type); per object and part: name, kind, size, bounding box, triangles, own tool, painted area per tool (mm² and %); undo steps; notes (single-tool printer, slots sharing one color). |
| `render_view` | `view?` (front, back, left, right, top, bottom, iso, iso_front_left, iso_back_left, iso_back_right), `azimuth?`, `elevation?`, `size?` (128–1024, default 768), `object?`, `zoom?`, `center?` [x,y,z], `highlight_paint?`, `grid?` | A PNG (MCP image content) and a `view_id`, plus per visible part its pixel count, `bbox_px` and `center_px`, the share of pixels per tool, mm per pixel and render time. |
| `paint_at` | `view_id`, `x`, `y`, `tool`, `mode?` (`smart_fill` default, `connected`, `triangle`, `sphere`), `angle?` (smart fill, default 30°), `radius?` (sphere, default 3 mm), `max_distance?` (smart fill and connected: mm along the surface from the point; default 0 = no limit), `allow_large?` | Paints the surface under a pixel. Reports the object/part/facet hit, the 3D point, triangles and mm² painted and the object's area per tool. A background pixel within 6 px of the model snaps to it. A fill (tool 1 or more) that would cover more than 40% of the part is refused unless `allow_large: true`: the reply gives the share and area it would cover and the `max_distance` values that cover about 10, 20 and 30% (suggesting the 20% one). |
| `pick` | `view_id`, `x`, `y` | What is under a pixel (object, part, facet, point, normal, tools on the facet), nothing painted. |
| `paint_height_band` | `object?`, `z_min`, `z_max`, `tool` | A horizontal band all around (split facets give straight band edges). |
| `paint_by_direction` | `object?`, `part?`, `direction` (up, down, side, front, back, left, right or `"x,y,z"`), `max_angle?` (default 30°), `tool`, `z_min?`, `z_max?` | Every whole facet facing that way. |
| `set_part_tool` | `object?`, `part?`, `tool` | The tool of the unpainted surface of an object or a part (the `extruder` setting); paint stays on top. |
| `set_filament_colors` | `colors` (per slot; `""` keeps one) | Which colors are loaded; used for renders and saved in the project. |
| `create_mix` | `target` (color name or RGB hex), `tools?` (allowed physical tool numbers, default all loaded), `max_components?` (2 default, maximum 3), `pattern?` (`layers` default, `outer_wall_first`, `gradient`), `name?` | Finds a ColorMix recipe for the target from loaded filaments. Returns its virtual `tool`, components and ratios, predicted color name plus machine-readable hex, CIEDE2000 `delta_e`, and `quality`. Use the tool number with any paint tool or `set_part_tool`. Requires ColorMix in Settings › Features. Identical recipes reuse the existing tool without an undo step. Live: one undo step `Mica MCP: create mix`. |
| `clear_paint` | `object?`, `part?` | Removes color paint (default: everything). |
| `add_shape` | `shape` (`benchy`, `cube`, `cylinder`, `cone`, `sphere`, `torus`, `pyramid`, `wedge`, `tube`, `disc`), `size?` (one number: the largest side; or `[x, y, z]` mm), `position?` `[x, y]` (new objects), `kind?` (`object` default, `part`, `modifier`, `negative_volume`, `support_enforcer`, `support_blocker`), `object?` | A built-in model (the original #3DBenchy or a generated shape, see "Add shape" in the app) as a new object (at `position`, else the bed's middle or beside the other objects) or as a part or volume of `object` (centred in it; a part stands beside it; a modifier starts at 100% infill). Saved as a plain mesh object named after the shape. Headless without an open model it starts a scene with the shape on the default printer. Live: on the window's current plate (a part joins the numbered object, else the selected one), one undo step `Mica MCP: add shape`. |
| `undo` | – | Undoes the last change (up to 64 steps). |
| `slice_summary` | – | Slices a temporary copy of the project with its settings, exactly as `mica slice project.3mf` would: layers, estimated time, filament per tool (m, g), total filament, tool changes, purge and tower filament (`purge_and_tower_filament_m/g`), warnings and the G-code check's findings. When the project uses several tools it also slices the same model in one color (no paint, everything on tool 1): `single_color` (time, filament), `color_cost` (extra time and %, extra filament, tool changes) and an `advice` line telling the AI to report the cost and offer cheaper alternatives. Lists `mixes_used` and notes their tool changes per layer and time cost (also in `color_cost`). Nothing is sent to a printer. |
| `save_project` | `path?`, `flavour?` (`orca` / `prusa`; default prusa for Prusa printers, orca otherwise), `overwrite?` | Writes a new 3MF with the painting, part tools, printer settings and slot colors. Without a path: a new name (`<model>-painted.3mf`, `-2`, …) on the Desktop (Documents without one, else the home folder). `~/` is the home folder; a relative path is taken from the server's working folder. Live: a copy of the window's project; its open file only with `overwrite` and `replace_open_project`. |
| `status` | – | The mode (live or headless, and the requested one), the socket, and in live mode the window's project, unsaved state, printer, tools, plates, objects and last undo step. |

The server's `instructions` and each tool's description teach the workflow: open → `scene_info`
(→ `set_filament_colors`) → `render_view` → look → `paint_at` (or bands, directions, part tools) →
`render_view` again to verify → `undo` mistakes → `slice_summary` → `save_project`. Errors are
tool results with a message that says what to do (no model open, unknown view, background pixel,
tool out of range, single-tool printer, existing file, …); nothing in a tool call panics the
server, and a panic in Mica code would come back as an "internal error" result with the session
kept.

## How it works

- **Scene**: the project the CLI's `--export-3mf` builds (`project_export::build`), with the
  profiles resolved as `mica slice` resolves them. Each model part gets the engine's
  `painting::selector::TriangleSelector` (the GUI painting tools' selector) in the bed space of
  the object's first instance; after every edit the selector is serialised into the part's
  `painting.material`, the same `FacetStates` the 3MF writer and the slicer read. Smart fill is
  the GUI's Fill brush (`TriangleSelector::fill`), `connected` is the same fill at 180°,
  `triangle` sets one facet, `sphere` is the GUI's sphere brush (with facet splitting), the band
  is the GUI's height range. Undo keeps snapshots of the painting streams, part/object settings,
  slot colors and mixed colors.
- **Rendering** (`crates/mica-cli/src/mcp/render.rs`): a CPU z-buffer rasterizer, so it needs no
  GPU, window or display (it works over SSH and in tests) and its output is deterministic. An
  orthographic orbit camera frames the objects; every painted leaf triangle is drawn in its tool's
  filament color with the thumbnail renderer's two-light shading, 2x supersampled. Next to the
  picture it keeps, per pixel, the original facet under it (a facet-ID buffer), which is how a
  pixel the AI names maps back to (object, part, facet) exactly; the 3D point comes from the
  viewing ray and that facet's plane. Lines mark silhouettes and depth steps, softer lines mark
  creases sharper than 40° (where smart fill stops), and `highlight_paint` washes out unpainted
  surfaces and outlines borders between tools. A faint grid every 100 px with labels helps the AI
  read positions. The GUI's wgpu renderer was not reused: it needs a window and eframe, and the
  CLI would have pulled in the whole GUI stack; the CPU path also gives an exact ID buffer for
  free. The last 8 views are kept for pointing (4 bytes per pixel each).
- **Speed** (fast profile, Apple silicon): Benchy (225,102 triangles) renders in about 70 ms at
  768 px and 100 ms at 1024 px; a 768 px PNG is 75–110 KB (100–150 KB as base64). Painting
  calls take a few milliseconds (a fill refused by the 40% guard a few more: it measures the
  region first); `slice_summary` slices twice when the project uses several tools (the
  one-color comparison), about 4–6 s for Benchy on the U1. Text results are a few hundred bytes to 2 KB; no per-triangle data is ever
  sent.
- **Slice summary**: the project is saved to a temporary folder, sliced through the CLI's own
  `slice_job` (so the G-code is what `mica slice` makes of the saved project), read back with
  the G-code validator for filament per tool and tool changes, and the folder is removed.

## Safety

- Local only: stdin/stdout, no listener, no network calls. `slice_summary` never contacts a
  printer; there is no print or upload tool.
- Fills on smooth models: smart fill stops only at edges sharper than its angle, so on a Benchy
  hull it runs over the whole hull. `max_distance` limits it to a geodesic distance from the
  clicked point (the engine's `TriangleSelector::fill_within`: Dijkstra over the fill region's
  vertices with a triangle-unfolding step, exact on flat and unrolled patches, facets on the
  limit split like a brush edge to 1/15 of the distance, 0.1–1 mm), and the 40% guard stops a
  runaway fill before it is applied. The GUI's Fill brush has the same "Max spread" setting and
  its hover preview shows what a click paints.
- Cost: the server instructions and tool descriptions tell the AI to keep color changes
  reasonable (whole parts, height bands, few fills), to report `slice_summary`'s `color_cost`
  and to offer cheaper alternatives.
- Saving: projects never land in Mica's source checkout (a folder up the path with this
  workspace's Cargo.toml and a `.git`, or the checkout the binary was built from); such a path
  is refused with a message, and without a path the project goes to the Desktop.
- The opened file is only read. `save_project` refuses the input's path, needs a `.3mf` path in an
  existing folder and `overwrite: true` to replace a file. Slicing works on a temporary copy that
  is deleted afterwards.
- Mica's settings folder is read as the CLI reads it (user profiles); the server writes nothing
  there.
- Large models stay responsive: images are capped at 1024 px, results carry summaries only, and
  slow work runs off the protocol thread.

## Tests

`crates/mica-cli/tests/mcp.rs` drives the server through rmcp's own client over an in-process
duplex pipe:

- `ai_paints_the_benchy_for_the_u1`: opens `tests/models/3DBenchy.stl` (CC0) for the U1 with
  white/red/blue/black, renders the iso view, finds hull, cabin-wall and chimney pixels from the
  facet buffer of the same (deterministic) view, checks that the unlimited hull fill is refused
  (48% of the part), paints them with `paint_at` smart fill (tools 2, 3, 4; the hull with
  `allow_large`), checks every tool's area (about 33/48/17/2 %), undo and repaint, error
  messages, `slice_summary` (four tools, 240 layers, about 340 tool changes, no G-code errors; one
  color 0h 40m against 1h 54m painted, 12 g purge and tower), `save_project` (and that the input is
  untouched, and that a path in the source checkout is refused), then reopens the 3MF and checks printer, slot
  colors and the area per tool. `MICA_MCP_RENDER_DIR=<dir>` keeps the before/after renders.
- `fill_spread_limit_on_the_benchy_hull`: a hull-side click: refused without a limit (with a
  suggested `max_distance`), about 430 mm² with `max_distance: 12`, the whole hull (4,512 mm²)
  with `allow_large` (renders `benchy-hull-fill-limited-12mm.png`, `benchy-hull-fill-unlimited.png`).
- Unit tests: the geodesic limit on a sphere (cap areas) and a coarse 24-sided cylinder (a disc
  on the unrolled side), the GUI's Fill preview and stroke with a spread limit
  (`paint::fill_preview_honours_the_spread_limit`), the save folder and source-checkout rules.
- `bands_directions_tools_and_clearing`: a cube on the U1: a height band (its area), the top by
  direction, the object's tool, sides, clear and undo, and the single-tool printer message.
- Live mode (`crates/mica-gui/src/mcp_live_tests.rs`: a headless window serving the socket in a
  temporary folder, `MicaMcp::call` on a client thread while the test pumps frames): live and
  headless renders and `paint_at` results are identical on the same model, an AI edit is one
  `Mica MCP: paint` undo step (Edit › Undo removes it; the `undo` tool undoes AI steps only), part
  tools and slot colors reach the window, a user edit after a render invalidates the view and
  is never overwritten, `open` is refused with unsaved changes and `save_project` with the open
  file, a painted PrusaSlicer project maps both ways, the live slice summary equals the headless
  one on the saved copy (and the window slices too), objects on two plates map back to their
  plate, and without a window auto mode is headless while `--live` explains (also with a stale
  socket). `live.rs` unit tests: protocol round trip, socket mode 0600, stale socket replaced,
  a live one kept and another protocol version refused.

Renders from the test (`benchy-before-iso.png`, `benchy-after-iso.png`,
`benchy-after-back-highlight.png`, `benchy-reopened-iso.png`) show the red hull, blue cabin and
black chimney; the reopened project renders identically.

## Live mode: the open Mica window

Live mode lets the AI paint in the running Mica window, with every change visible at once and
undoable there.

**Trying it**

1. Start Mica Slicer (the Mica MCP feature is on unless Settings › Features turned it off).
   The window listens on `mcp.sock` in Mica's settings folder.
2. Register `mica mcp` with the AI client as above (no extra flag: the default mode uses the
   window when one answers).
3. Open a model in Mica (or let the AI call `open`), then ask, e.g. *"Paint the top of the object
   in Mica blue."* The AI calls `status` / `scene_info`, `render_view`, `paint_at`; the paint
   appears in the window, and the object list and 3D view update as for the user's own strokes.
   The 3D view's lower left corner shows **Mica MCP connected** while the client is attached.
4. Edit › Undo (Cmd+Z) takes back an AI edit: each tool call that changed something is one undo
   step named `Mica MCP: paint` (`part tool`, `filament colors`, `clear paint`, `create mix`).

**Modes** (`mica mcp --help`): the default is *auto*: live when a window answers, else headless; a
headless scene the AI opened is kept until its next `open`. `--live` always uses the window and
fails clearly while none answers; `--headless` never uses it. The `status` tool reports the mode,
the socket, and in live mode the window's project, unsaved state, printer and last undo step; the
server instructions name the mode at start.

**Transport and protocol** (`crates/mica-cli/src/mcp/live.rs`): a Unix domain socket in Mica's
settings folder (`$MICA_CONFIG_DIR/mcp.sock` when set), mode 0600, created by the window at start
and removed when it closes. A socket file nobody answers on (a crashed window) is replaced; one
another window answers on is left alone (that window says so). No TCP, nothing reachable from the
network. One JSON object per line each way, versioned (`"v": 1`):
`{"v":1,"id":7,"tool":"paint_at","args":{…}}` → `{"v":1,"id":7,"ok":true,"content":[{"type":"text","text":"…"},{"type":"image","data":"<base64>","mime":"image/png"}]}`
or `{"v":1,"id":7,"ok":false,"error":"…"}`. `hello` (protocol version, app version, pid) is
answered by the connection itself; a request of another protocol version gets an error. On
Windows live mode is not supported yet: `mica mcp` stays headless (`--live` says why) and the
window opens no endpoint.

**Same tools, same results** (`crates/mica-cli/src/mcp/tools.rs`, `crates/mica-gui/src/mcp_live.rs`):
the painting, rendering and inspection tools are one set of functions over JSON arguments that
both the headless server and the window call. The window runs them on a scene opened from its
project exactly as File › Save writes it (`App::build_project`, written to a temporary 3MF and
opened as `mica mcp` opens a 3MF), on a worker thread, so a live call returns what a headless call
returns on that saved project (the tests compare render and paint results on the same model). The
scene is kept while the window's project is unchanged, so view ids stay valid; when the user
edits, the next call syncs again and older view ids are refused (ids never repeat). Requests run
on the UI thread between frames, one at a time; nothing waits on the UI thread.

**Applying edits**: a call that changed the scene is applied to the plates as one undo step:
the color painting (each model part's stream joined like the object's mesh, checked to map back
exactly when the scene is synced; an object where it does not is refused rather than repainted
wrongly), object and part tools, slot colors (as project colors, like the color picker), and
mixed colors.
If the user changed the project while the call ran, nothing is applied and the AI is told to
render again and retry.

**Window-only tools**:

- `open` opens the file as a new project only when the window has no unsaved changes (a model
  imported and never saved counts as unsaved); otherwise it is refused with an explanation. A
  printer shortcut or preset name switches the window's printer first; `print` and `filament`
  must be presets of that printer; `bed` is refused (choose it in the window).
- `save_project` writes a copy (the window's document and its unsaved state are unchanged), with
  the headless guards (`.3mf`, existing folder, `overwrite` for an existing file, never inside
  the source checkout). The project file open in the window is only replaced with
  `overwrite: true` and `replace_open_project: true`, then as a normal save.
- `slice_summary` runs the headless summary slice on the worker and also starts the window's
  own slice of the current plate, so the user sees the preview.
- `undo` undoes the window's last step only when it is a Mica MCP step; the user's own steps are
  never undone (the AI is told to ask).

**Errors** are tool errors for the AI and also show next to the indicator (hover for the whole
text) for a few seconds; startup problems (another window serves Mica MCP, the folder is not
writable) appear as a notice.

**Limitations**: the scene covers all plates (objects on other plates sit at their plate's grid
offset, so `render_view` without `object` frames them all); `open` with a 3MF uses the window's
project-settings choice, not necessarily the project's own printer; part tools of multi-part
objects go into the part's settings in the window; Windows has no live mode yet; the feature
switch is read when the window starts.

## Creating mixed colors

For example, with red and yellow loaded, call `create_mix` with `target: "orange"`, then
assign or paint with the returned `tool` number. The server searches physical tool combinations
and positive shares in 5% steps, plus exact equal thirds. Each candidate is snapped through ColorMix's exact printable
layer cycle (the same snapping as the GUI editor), then scored using the existing Prusa
filament color predictor and CIEDE2000. The smallest difference wins; this is a prediction,
and actual filament, layer height and viewing direction affect the printed color.

Quality is `very close` below ΔE 3, `close` below 6, `rough` below 12, otherwise
`can't match well — load a filament closer to <color name>`. Describe colors to the user by
name; the hex field is machine-readable data. A reused recipe keeps its existing name.
The engine's maximum virtual tool number is 255; a full project can still reuse a recipe.

`gradient` matches the target at the bottom and reverses the component shares at the top,
following the GUI's initial gradient endpoints. Its returned `predicted_color` and `delta_e`
refer to the bottom (`prediction_scope`); `end_ratios` describes the top. The view uses the
same midpoint swatch as the GUI. With one target, this convention can produce an unchanged
gradient when the best shares are symmetric.

Mixed colors alternate physical tools per layer. `slice_summary` lists the recipes used
in `mixes_used` and includes a time-cost note; when it compares with one color, `color_cost`
contains the same recipe list and note. Tell the user the measured time, filament and tool
changes before suggesting a mix for a large area.

Mix definitions use the GUI's ColorMix 3MF archive entry and the shared slicing engine.
Headless undo restores the list as well as paint and assignments. Live creation applies the
list to the window as one undo step; a duplicate leaves the window's undo stack unchanged.
ColorMix off rejects creation with the Settings › Features instruction and keeps existing
project data, as the GUI does.

Tests cover red/yellow to orange on two physical tools, duplicate reuse, invalid inputs,
the feature switch, all patterns, virtual tool painting and assignment, undo, the engine's
ID limit, and save/reopen. Live tests compare the saved ColorMix section and sliced G-code
against the GUI Draft/Add path for all three patterns (byte equality with a fixed clock).

For environments that deny Unix sockets, the new live create-mix test can use the same window
request queue, worker and undo/apply path in process:
`MICA_MCP_TEST_IN_PROCESS=1 SOURCE_DATE_EPOCH=0 cargo test --offline -p mica-gui live_create_mix`.
Without that environment variable it uses the real socket transport.
