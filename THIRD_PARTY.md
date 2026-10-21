# Third-party components

Libraries, data and assets that Mica Slicer depends on or ships. Logic ported from other slicers is
tracked in [`PROVENANCE.md`](PROVENANCE.md) instead.

The app's About › Credits tab credits everything below (and every upstream of PROVENANCE.md) from
`crates/mica-gui/src/credits.rs`, with a short list of key libraries. The full Rust crate list is in
the app's About › Open-source licenses view and in [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).
It is generated, not hand-written: `cargo xtask gen-credits` reads `cargo tree` (mica-gui + mica-cli,
every platform) into `crates/mica-gui/assets/credits/crates.tsv` with each crate's copyright lines,
checks that every license it names has its full text in `crates/mica-gui/assets/credits/licenses/`,
and rewrites `THIRD_PARTY_LICENSES.md`. Run it after changing dependencies (a mica-gui test fails
when the list no longer matches `Cargo.lock` or the file is stale). The website (a separate repository) hosts no
credits or license pages, only a short Thanks section that points here and to the app.

Before adding a dependency, check that its license is compatible with AGPL-3.0-only
(MIT, Apache-2.0, BSD, BSL-1.0, Zlib, MPL-2.0 and LGPL are fine; proprietary or "non-commercial" licenses are not).

| Component | Version | License | Used by | Purpose |
|---|---|---|---|---|
| PrusaSlicer-generated normalized cube layer commands | PrusaSlicer 2.9.6 (`b028299c77`), generated 2026-10-16 | Generated output from Mica's cube20 model; PrusaSlicer generator AGPL-3.0 | `mica-gcode` tests | `tests/fixtures/coreone_cube20_layer_moves.gcode`, 100 layers / 6,256 commands, generated with `scripts/gcode-exact-diff.py`; no embedded profile bundle or startup/shutdown templates |
| `eframe`, `egui-wgpu` | 0.37.0 | MIT OR Apache-2.0 | `mica-gui` | Native app shell, egui controls and wgpu callbacks; wgpu backend only |
| `glam` | 0.34.1 | MIT OR Apache-2.0 | `mica-gui` | Camera, transforms and CPU picking |
| `bytemuck` | 1.25.2 | Zlib OR Apache-2.0 OR MIT | `mica-gui` | Safe GPU and preview record byte casts (derive) |
| `rfd` | 0.17.2 | MIT | `mica-gui` | Native open/save dialogs |
| `serde` (derive), `toml` | 1.0, 0.9 | MIT / Apache-2.0 | `mica-gui` | Preferences file (`mica.toml`); both crates were already in the dependency tree |
| `wgpu`, `winit` (transitive through eframe) | 30.0.1, 0.30.13 | MIT OR Apache-2.0 | `mica-gui` | Metal/Vulkan/DX12 rendering and native windows |
| `epaint_default_fonts` | 0.37.0 | Crate manifest: (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0 | `mica-gui` via eframe | Fonts already embedded by egui's default-fonts feature; individual notices below |
| Ubuntu Light | supplied in `epaint_default_fonts` 0.37.0 | Ubuntu Font Licence 1.0 | `mica-gui` | Default proportional UI font; [notice](docs/licenses/egui-fonts/UFL.txt) |
| Hack Regular (DejaVu / Bitstream Vera ancestry) | supplied in `epaint_default_fonts` 0.37.0 | MIT; DejaVu public domain; Bitstream Vera license | `mica-gui` | Default monospace font; [complete inherited notices](docs/licenses/egui-fonts/Hack-Regular.txt) |
| egui-icons / emoji-icon-font | supplied in `epaint_default_fonts` 0.37.0 | MIT | `mica-gui` | egui's embedded icon subset; [notice](docs/licenses/egui-fonts/emoji-icon-font-mit-license.txt) |
| Noto Emoji (optional monochrome feature, not enabled by this GUI) | supplied in `epaint_default_fonts` 0.37.0 | OFL-1.1 | egui optional asset | [Font package notice](docs/licenses/egui-fonts/OFL.txt) retained for completeness |
| IBM Plex Sans (Regular, Medium, SemiBold) | `@ibm/plex-sans` 1.1.0 release | SIL OFL 1.1 | `mica-gui` | UI text, and the Text tool's always-available default font (OFL 1.1 allows bundling with software and puts no condition on documents made with the font, so embossed geometry is free); TTFs embedded from `crates/mica-gui/assets/fonts/`; [licence](docs/licenses/ibm-plex/OFL.txt) (Copyright © 2017 IBM Corp., Reserved Font Name "Plex") |
| IBM Plex Mono (Regular, Medium) | `@ibm/plex-mono` 2.5.0 release | SIL OFL 1.1 | `mica-gui` | Numbers and read-outs; same licence file |
| `resvg`, `usvg` | 0.45.1 | Apache-2.0 OR MIT | `mica-gui` | Rasterising the printer's bed texture SVG once (no `text`, `system-fonts` or raster-image features) |
| `tiny-skia`, `tiny-skia-path` | 0.11.4 | BSD-3-Clause | `resvg` | 2D rasteriser (Skia port) |
| `usvg` (direct dependency, no default features) | 0.45.1 | Apache-2.0 OR MIT | `mica-gui` | Text / SVG tool: parsing the imported SVG into paths with resolved transforms (no `text` or `system-fonts`); already in the tree through `resvg`, no new download |
| `skrifa`, `read-fonts`, `font-types` | 0.44.0, 0.41.0, 0.12.7 | MIT OR Apache-2.0 | `mica-gui` | Text / SVG tool: font names, weights, outlines and metrics of the bundled and installed fonts (pure Rust; already in the tree through egui's `epaint`, no new download) |
| `harfrust` | 0.12.0 | MIT | `mica-gui` | Text / SVG tool: text shaping (kerning, ligatures, marks), a pure-Rust HarfBuzz port (already in the tree through `epaint`, no new download) |
| Installed system fonts | whatever the computer has | each font's own licence | `mica-gui` at run time | The Text tool lists and reads the fonts installed on the user's computer in place (macOS, Linux and Windows font folders); Mica ships none of them and a 3MF stores only the font's name and file name |
| `roxmltree`, `simplecss`, `svgtypes`, `kurbo`, `data-url`, `siphasher`, `base64`, `arrayvec` | 0.20.0, 0.2.2, 0.15.3, 0.11.3, 0.3.2, 1.0.4, 0.22.1, 0.7.8 | MIT OR Apache-2.0 | `resvg` / `usvg` | XML, CSS, SVG value and path parsing |
| `strict-num`, `xmlwriter`, `imagesize`, `pico-args`, `float-cmp` | 0.1.1, 0.1.0, 0.13.0, 0.5.0, 0.9.0 | MIT | `resvg` / `usvg` | Helpers |
| `arrayref` | 0.3.9 | BSD-2-Clause | `tiny-skia` | Array slicing |
| `security-framework` | 3.7.0 (pinned exactly; released 2026-02-20) | MIT OR Apache-2.0 | `mica-gui` (macOS only) | Printer API keys and passwords as generic-password items in the macOS Keychain (service "Mica Slicer"); `printers.toml` only stores the item reference |
| `muda` (no default features) | 0.19.3 (pinned exactly; released 2026-06-17) | Apache-2.0 OR MIT | `mica-gui` (macOS only) | The native macOS menu bar (application, File, Edit, View, Calibrate, Window and Help menus) built from Mica's one menu description; falls back to the in-window menu if it cannot be set up |
| `keyboard-types`, `crossbeam-channel` | 0.7.0, 0.5.17 | MIT OR Apache-2.0 | `muda` | Accelerator key codes and the menu event channel (`objc2`, `objc2-app-kit`, `objc2-foundation`, `dpi`, `png`, `once_cell`, `thiserror` were already in the tree) |
| `arboard` (direct dependency, no default features) | 3.7.0 | MIT OR Apache-2.0 | `mica-gui` (macOS only) | Reads the clipboard text when the native Edit > Paste item pastes into a text field; already in the tree through `egui-winit`, no new download |
| `security-framework-sys`, `core-foundation` 0.10, `core-foundation-sys`, `bitflags` 2 | 2.17.0, 0.10.1, 0.8.7, 2.13.2 | MIT OR Apache-2.0 | `security-framework` | Bindings and Core Foundation types for the Keychain calls |
| `image` (features `jpeg`, `png` only) | 0.25.10 | MIT OR Apache-2.0 | `mica-gui` | Decoding camera snapshots (JPEG / PNG) for the Machine view, and texture images imported into the texture gallery; already in the dependency tree through `eframe` |
| `png` | 0.18.1 | MIT OR Apache-2.0 | `mica-core` | Reading and writing the grey PNG height maps of surface textures (bundled maps, custom images in projects); already in the tree through `image` |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | `mica-core` | Content hashes naming custom texture images in projects; already in the tree through `mica-camera` |
| `zune-jpeg`, `zune-core` | 0.5.15, 0.5.x | MIT OR Apache-2.0 OR Zlib | `image` | JPEG decoder enabled for camera snapshots |
| `retina` (no default features: H.265 off) | 0.4.21 (pinned exactly) | MIT OR Apache-2.0 | `mica-camera` | Pure-Rust RTSP client: DESCRIBE/SETUP/PLAY over TCP-interleaved RTP, H.264 depacketisation (Printers dashboard RTSP cameras) |
| `openh264` (optional, development feature `openh264` only; **not in packaged or default builds**) | 0.9.9 (pinned exactly) | BSD-2-Clause | `mica-videodec`, `mica-camera` | Development alternative to the operating system's H.264 decoder, and live encoding of the mock printer's RTSP picture. Off by default |
| `openh264-sys2` (feature `source`) with Cisco OpenH264 source (same feature only) | 0.9.9; OpenH264 `1bca62311e` | BSD-2-Clause (Copyright © 2013 Cisco Systems) | `openh264` | Compiles OpenH264's C/C++ source with the system C++ compiler. Cisco's patent licence covers only Cisco's own binaries, not builds from source, so shipped builds never enable it (see "H.264 decoding" below) |
| `tokio` (features `rt`, `net`, `time`), `tokio-macros`, `mio`, `socket2` | 1.53.3, 2.7.2, 1.2.4, 0.5.10 / 0.6.5 | MIT (`socket2`: MIT OR Apache-2.0) | `mica-camera`, `retina` | Single-threaded async runtime that lives on an RTSP player's thread |
| `futures`, `futures-util`, `futures-core`, `futures-channel`, `futures-executor`, `futures-io`, `futures-macro`, `futures-sink`, `futures-task` | 0.3.34 | MIT OR Apache-2.0 | `mica-camera`, `retina` | Stream adapters for the RTSP session |
| `url` | 2.5.8 (already in the tree) | MIT OR Apache-2.0 | `mica-camera`, `retina` | RTSP URLs |
| `rmcp`, `rmcp-macros` (features `server`, `macros`, `transport-io`, `base64`; tests add `client`) | 3.5.1 (pinned exactly; released 2026-10-05) | Apache-2.0 | `mica-cli` (`mica mcp`) | The official Rust SDK of the Model Context Protocol (github.com/modelcontextprotocol/rust-sdk): JSON-RPC over stdin/stdout and the `#[tool]` router for the AI painting server (docs/mcp.md). No HTTP, OAuth or child-process transports are enabled |
| `schemars`, `schemars_derive`, `serde_derive_internals` | 1.2.3, 1.2.3, 0.30.0 | MIT (`serde_derive_internals`: MIT OR Apache-2.0) | `mica-cli`, `rmcp` | JSON Schemas of the MCP tools' arguments |
| `tokio` (features `rt`, `rt-multi-thread`, `io-std`, `macros` for `mica-cli`), `tokio-util`, `tokio-stream` | 1.53.3, 0.7.20, 0.1.19 | MIT | `mica-cli`, `rmcp` | The MCP server's async runtime and stdio framing (tokio was already in the tree) |
| `chrono`, `iana-time-zone`, `iana-time-zone-haiku`, `dyn-clone`, `ref-cast`, `ref-cast-impl` | 0.4.45, 0.1.65, 0.1.2, 1.0.20, 1.0.27, 1.0.27 | MIT OR Apache-2.0 | `rmcp`, `schemars` | Protocol timestamps, schema helpers |
| `darling`, `darling_core`, `darling_macro`, `ident_case`, `strsim` | 0.24.1, 0.24.1, 0.24.1, 1.0.1, 0.11.1 | MIT (`ident_case`: MIT/Apache-2.0) | `rmcp-macros` | Attribute parsing of the `#[tool]` macros (build time) |
| `bytes` | 1.12.1 (already in the tree) | MIT | `retina` | Packet buffers |
| `h264-reader`, `bitstream-io`, `sdp-types`, `rfc6381-codec`, `mp4ra-rust`, `mpeg4-audio-const`, `four-cc`, `fallible-iterator`, `http-auth`, `hex-slice`, `pretty-hex` | 0.8.0, 2.6.0, 0.1.8, 0.2.0, 0.3.0, 0.2.0, 0.4.0, 0.3.0, 0.1.10, 0.1.4, 0.4.2 | MIT OR Apache-2.0 (`sdp-types`, `hex-slice`, `pretty-hex`: MIT) | `retina` | H.264 parameter parsing, SDP, codec strings, RTSP Basic/Digest authentication, hex dumps in errors |
| `md-5`, `sha2`, `digest`, `block-buffer`, `crypto-common`, `generic-array`, `typenum`, `cpufeatures` | 0.10.6, 0.10.9, 0.10.7, 0.10.4, 0.1.7, 0.14.7, 1.20.1, 0.2.17 | MIT OR Apache-2.0 (`generic-array`: MIT) | `retina` (through `http-auth`) | RTSP Digest authentication hashes |
| `derive_more`, `derive_more-impl`, `convert_case`, `unicode-xid` | 2.1.1, 2.1.1, 0.10.0, 0.2.6 | MIT (`unicode-xid`: MIT OR Apache-2.0) | `retina` | Derive macros (build time) |
| `jiff`, `jiff-core`, `jiff-static`, `jiff-tzdb`, `jiff-tzdb-platform` | 0.2.38, 0.1.1, 0.2.38, 0.1.9, 0.1.3 | Unlicense OR MIT | `retina`, `mica-gcode` | Wall-clock timestamps of received packets; the local export time in the G-code identity line |
| `rand`, `rand_chacha`, `rand_core` | 0.8.8, 0.3.1, 0.6.4 | MIT OR Apache-2.0 | `retina` | RTSP client nonces / SSRC (a second, older copy next to `rand` 0.9 above) |
| `bstr` | 1.13.1 | MIT OR Apache-2.0 | `sdp-types` (through `retina`) | Byte strings in SDP parsing |
| `defmt`, `defmt-macros`, `defmt-parser` | 1.2.0, 1.2.0, 1.1.0 | MIT OR Apache-2.0 | listed in Cargo.lock through `retina`'s dependency graph | Not compiled for Mica's targets (embedded logging); recorded because the lock file names it |
| `minisign-verify` | 0.3.0 (pinned exactly) | MIT | `mica-update` | Verifies the minisign (Ed25519) signature of a downloaded update against the public key compiled into Mica; no dependencies |
| `semver` | 1.0.28 (already in the tree) | MIT OR Apache-2.0 | `mica-update` | Comparing Mica's version with the update server's |
| `base64` | 0.22.1 (already in the tree) | MIT OR Apache-2.0 | `mica-update` | Decoding the base64-wrapped key and signatures (cargo-packager's format) |
| `minisign` and its `scrypt`, `salsa20`, `pbkdf2`, `hmac`, `cipher`, `inout`, `ct-codecs`, `rpassword`, `rtoolbox` | 0.10.0; 0.11.0, 0.10.2, 0.12.2, 0.12.1, 0.4.4, 0.1.4, 1.1.8, 7.5.4, 0.0.6 | MIT (`minisign`, `ct-codecs`); MIT OR Apache-2.0; Apache-2.0 (`rpassword`, `rtoolbox`) | `mica-update` tests only (dev-dependency) | Signs test updates with a throwaway key made in memory; never compiled into Mica |
| `cargo-packager` (build tool, not linked) | 0.11.8 | Apache-2.0 OR MIT | `scripts/release.sh` | Makes the `.app` bundle and the NSIS installer, and the update signatures (`cargo packager signer`) |
| NSIS installer stub and `nsis_tauri_utils` plugin (in the Windows installer) | makensis 3.13; plugin as downloaded by cargo-packager 0.11.8 | zlib/libpng (NSIS); MIT OR Apache-2.0 (plugin) | Windows installer | The installer program around Mica's files |
| AppImage runtime (in the Linux AppImage) and `appimagetool` (build tool) | appimagetool 1.9.0 | MIT | Linux AppImage | The self-mounting runtime at the front of the `.AppImage` file |
| `wide`, `safe_arch` | 1.7.1, 1.2.0 | Zlib OR Apache-2.0 OR MIT | `openh264` (development feature only) | SIMD YUV → RGBA conversion |
| `nasm-rs` | 0.3.2 | MIT OR Apache-2.0 | `openh264-sys2` (build time, development feature only) | Assembles OpenH264's optional x86 assembly when NASM is installed |
| `objc2-video-toolbox`, `objc2-core-media`, `objc2-core-video` (no default features: only the decompression session, block / sample buffer, format description and pixel buffer APIs) | 0.3.2 | Zlib OR Apache-2.0 OR MIT | `mica-videodec` (macOS only) | Bindings to Apple's VideoToolbox, CoreMedia and CoreVideo frameworks for H.264 camera decoding (`objc2`, `objc2-core-foundation`, `block2` 0.6 were already in the tree) |
| `objc2-core-audio`, `objc2-core-audio-types` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | listed in Cargo.lock through `objc2-core-media`'s optional features | Not compiled (the CoreAudio features are off); recorded because the lock file names them |
| `windows` (features `Win32_Foundation`, `Win32_Media_MediaFoundation`, `Win32_System_Com`, `Win32_System_Variant`), `windows-core` | 0.62.2 (already in the tree) | MIT OR Apache-2.0 | `mica-videodec` (Windows only) | Bindings to Media Foundation (the H.264 decoder MFT) and COM |
| `gstreamer`, `gstreamer-app`, `gstreamer-video`, `gstreamer-base` | 0.25.4, 0.25.2, 0.25.4, 0.25.4 | MIT OR Apache-2.0 | `mica-videodec` (Linux and other Unix only) | Safe Rust bindings to the system's GStreamer: `appsrc ! h264parse ! decodebin ! videoconvert ! appsink` |
| `gstreamer-sys`, `gstreamer-app-sys`, `gstreamer-base-sys`, `gstreamer-video-sys`, `glib`, `glib-sys`, `glib-macros`, `gobject-sys`, `gio-sys` | 0.25.4 (`glib`: 0.22.10, `-sys` / macros 0.22.9) | MIT | `gstreamer` (Linux only) | C bindings to the system's GStreamer / GLib libraries (linked dynamically; nothing bundled) |
| `system-deps`, `cfg-expr`, `heck`, `version-compare`, `target-lexicon`, `toml` 1.1 | 9.0.0, 0.20.10, 0.5.0, 0.2.1, 0.13.5, 1.1.8 | MIT OR Apache-2.0 (`version-compare`: MIT; `target-lexicon`: Apache-2.0 WITH LLVM-exception) | `*-sys` above (build time, Linux only) | Finds the system GStreamer libraries through pkg-config |
| `atomic_refcell`, `kstring`, `muldiv`, `num-integer`, `num-rational`, `option-operations`, `pastey` | 0.1.14, 2.0.5, 1.0.1, 0.1.47, 0.4.2, 0.6.1, 0.2.3 | MIT OR Apache-2.0 (`muldiv`: MIT) | `gstreamer`, `glib` (Linux only) | Helpers (clock arithmetic, interned strings, macros) |
| H.264 test clip `crates/mica-videodec/testdata/test-pattern-320x240.h264` | made 2026-10-19 | Mica's own test picture (AGPL-3.0-only); encoder output, no encoder code | `mica-videodec` tests, `mica-camera` tests and mock RTSP camera | 20 frames (3 KB) of `mica_camera::mock::test_pattern`, encoded with Apple's VideoToolbox H.264 encoder through ffmpeg (`testdata/test-pattern.py` has the command); also streamed by the mock camera-streamer (WebRTC) |
| `str0m` (no default features: no bundled crypto backend) | 0.24.1 (pinned exactly; released 2026-10-03) | MIT OR Apache-2.0 | `mica-camera` | Sans-IO WebRTC: ICE, DTLS, SRTP, RTCP (NACK, PLI), SCTP data channels and H.264 depacketisation (single NAL, STAP-A, FU-A) for camera-streamer's WebRTC (the U1's case camera), receive only; `#![forbid(unsafe_code)]` |
| `str0m-proto`, `is`, `sctp-proto` | 0.7.0, 0.11.1, 0.10.5 (locked to a release from 2026-09-18) | MIT OR Apache-2.0 | `str0m` | Crypto-provider traits, the ICE agent (no unsafe code), SCTP for the `keepalive` data channel (`#![forbid(unsafe_code)]`) |
| `dimpl` (features `rust-crypto` only) | 0.7.5 (pinned exactly; released 2026-10-03) | MIT OR Apache-2.0 | `mica-camera`, `str0m` | DTLS 1.2/1.3 in pure Rust on RustCrypto (`#![forbid(unsafe_code)]`). Its `rcgen` and `aws-lc-rs` features stay off, so AWS-LC (a second, C crypto library) is not built |
| `rcgen` (feature `ring` only) | 0.14.10 (pinned exactly) | MIT OR Apache-2.0 | `mica-camera` | The self-signed ECDSA P-256 DTLS certificate of each WebRTC connection, on `ring` (already in the tree through rustls) |
| `aes`, `aes-gcm`, `ctr`, `hmac`, `sha1`, `sha2` (direct) and `aead`, `ghash`, `polyval`, `universal-hash`, `opaque-debug`, `ccm`, `hkdf`, `chacha20`, `chacha20poly1305`, `poly1305` | 0.8.4, 0.10.3, 0.9.2, 0.12.1, 0.10.7, 0.10.9; 0.5.2, 0.5.1, 0.6.2, 0.5.1, 0.3.1, 0.5.0, 0.12.4, 0.9.1, 0.10.1, 0.8.0 | MIT OR Apache-2.0 | `mica-camera`, `dimpl` | RustCrypto ciphers and hashes: SRTP (AES-128 counter mode with HMAC-SHA1, AES-GCM), STUN message integrity, certificate fingerprints, DTLS record protection |
| `p256`, `p384`, `ecdsa`, `elliptic-curve`, `primeorder`, `rfc6979`, `crypto-bigint`, `ff`, `group`, `sec1`, `base16ct`, `signature`, `x25519-dalek`, `curve25519-dalek` | 0.13.2, 0.13.1, 0.16.9, 0.13.8, 0.13.6, 0.4.0, 0.5.5, 0.13.1, 0.13.0, 0.7.3, 0.2.0, 2.2.0, 2.0.1, 4.1.3 | MIT OR Apache-2.0 (`x25519-dalek`, `curve25519-dalek`: BSD-3-Clause) | `dimpl` | DTLS key exchange and signatures |
| `der`, `der_derive`, `pkcs8`, `spki`, `pem-rfc7468`, `const-oid`, `x509-cert`, `flagset`, `base64ct`, `yasna`, `zeroize_derive`, `crc`, `crc-catalog` | 0.7.10, 0.7.3, 0.10.2, 0.7.3, 0.7.0, 0.9.6, 0.2.5, 0.4.7, 1.8.3, 0.6.0, 1.5.0, 3.4.0, 2.5.0 | MIT OR Apache-2.0 (`flagset`: Apache-2.0) | `dimpl`, `rcgen`, `sctp-proto`, `is` | Certificate and key encoding, CRC-32c for SCTP and STUN |
| `asn1-rs`, `asn1-rs-derive`, `asn1-rs-impl`, `der-parser`, `oid-registry`, `x509-parser`, `rusticata-macros`, `nom` 7, `minimal-lexical`, `num-bigint`, `lazy_static`, `data-encoding`, `synstructure` 0.13.2, `fiat-crypto`, `curve25519-dalek-derive` | as in Cargo.lock | MIT OR Apache-2.0 (`nom`, `data-encoding`, `synstructure`: MIT; `fiat-crypto`: MIT OR Apache-2.0 OR BSD-1-Clause) | listed in Cargo.lock through optional features of `rcgen` / `curve25519-dalek` | Not compiled; recorded because the lock file names them |
| str0m-rust-crypto (shape of the provider glue only) | 0.6.0 | MIT OR Apache-2.0 (Copyright Martin Algesten and the str0m contributors) | `mica-camera/src/webrtc/crypto.rs` | Mica's crypto provider for str0m follows the structure of str0m's own pure-Rust backend (not a dependency: it would turn on AWS-LC for certificate generation); written for Mica, with the DTLS certificate made by rcgen on ring |
| camera-streamer WebRTC signalling (protocol only, no code) | ayufan/camera-streamer `e17a86e4f9` (2026-03-02) | GPL-3.0 (protocol not copied code) | `mica-camera/src/webrtc/` | The JSON POST exchange (`request` → offer, `answer`, `remote_candidate`) and the `keepalive` ping/pong, read from `output/webrtc/webrtc.cc` and `html/webrtc.html` and checked against Mainsail's `WebrtcCameraStreamer.vue` (GPL-3.0); Mica's client and mock are written from the behaviour, no code is taken |

| Orca material adhesion parameters | `orca@8500fcdcca`, `MaterialType.cpp` | AGPL-3.0 | `mica-core` | Adhesion coefficients and thermal lengths used by auto brim; logic recorded in PROVENANCE.md |
| `flate2` | 1.1.10 | MIT / Apache-2.0 | `mica-gcode` | Zlib-wrapped Deflate metadata, G-code blocks and PNG images |
| `quick-xml` | 0.37.5 | MIT | `mica-core` | Streaming 3MF XML reader/writer |
| `zip` | 2.4.2 | MIT | `mica-core` | Streaming 3MF project reader/writer and generated test archives (Deflate only) |
| `zopfli`, `bumpalo`, `log` | 0.8.4, 3.20.3, 0.4.34 | Apache-2.0 (`bumpalo`, `log`: MIT / Apache-2.0) | `zip` | Deflate feature dependencies |
| `displaydoc`, `indexmap`, `hashbrown`, `equivalent` | 0.2.7, 2.14.2, 0.17.1, 1.0.3 | MIT / Apache-2.0 | `zip` | Error messages and ZIP directory indexing |
| `miniz_oxide` | 0.9.1 | MIT / Zlib / Apache-2.0 | `flate2` | Default Rust compression backend |
| `adler2` | 2.0.1 | 0BSD / MIT / Apache-2.0 | `miniz_oxide` | Adler-32 checksums |
| `simd-adler32` | 0.3.10 | MIT | `miniz_oxide` | Vectorized Adler-32 |
| `crc32fast` | 1.5.2 | MIT / Apache-2.0 | `flate2` | Compression-library CRC32 (bgcode block CRC is ported separately) |
| `cfg-if` | 1.0.5 | MIT / Apache-2.0 | `crc32fast` | Conditional compilation |
| `regex` | 1.13.1 | MIT / Apache-2.0 | `mica-template` | Regex matching (`=~`) in G-code templates |
| `regex-automata`, `regex-syntax` | 0.4.18, 0.8.11 | MIT / Apache-2.0 | `regex` | Regex engine |
| `aho-corasick`, `memchr` | 1.1.5, 2.8.3 | Unlicense / MIT | `regex` | String search |
| `rayon` (+ `rayon-core`, `crossbeam-*`, `either`) | 1.12 | MIT / Apache-2.0 | `mica-core`, `mica-gui` | Parallel layer processing |
| `clipper2-rust` (vendored fork, `crates/clipper2`) | 1.2.0 + Mica changes (`32519f102e`) | BSL-1.0 ([licence](crates/clipper2/LICENSE)) | `mica-core` | Polygon clipping and offsetting (pure-Rust port of Clipper2 1.5.4, `using_z`). Vendored in performance round 5: branch-free `nearbyint`, no path clone / write-only vertex lists in `add_path`, reserved arenas, output paths moved into the tree, opt-in input capture (feature `capture`, off by default); SVG/file/timer utilities, demo binary and examples removed; one clippy fix. Results are bit-identical to 1.2.0 on every captured input; every file carries a `Source:`/`Changes:` header and a PROVENANCE.md row |
| `num-traits`, `autocfg` | 0.2, 1.x | MIT / Apache-2.0 | `clipper2-rust` | Numeric traits |
| `boostvoronoi` (vendored fork, `crates/boostvoronoi`) | 0.12.1 + Mica changes (`4060736209`) | BSL-1.0 ([licence](crates/boostvoronoi/LICENSE)) | `mica-core` | Segment Voronoi diagram for the medial axis, Arachne variable-width walls and painting segmentation; pure-Rust port of Boost.Polygon's Voronoi, which PrusaSlicer uses. Vendored in performance round 4: fixed-capacity big integers (`ArrayVec<u32, 64>`, like Boost's `extended_int<64>`), optional geo/glam/cgmath/mint/nalgebra/serde integrations and file/visual utilities removed, clippy fixes; every file carries a `Source:`/`Changes:` header and a PROVENANCE.md row |
| `arrayvec` | 0.7.8 | MIT OR Apache-2.0 | `boostvoronoi` (also `resvg`) | Fixed-capacity word storage of the extended-precision integer |
| `regex` (dev-dependency only) | 1.13 | MIT / Apache-2.0 | `boostvoronoi` unit tests | Parsing recorded beach-line test cases |
| `cpp_map` | 0.2.0 | MIT / Apache-2.0 | `boostvoronoi` | Ordered map for the beach line (skip list; random levels don't affect results) |
| `rand`, `rand_core`, `rand_chacha`, `ppv-lite86`, `getrandom`, `libc`, `zerocopy` (+ `r-efi`, `wasip2`, `wit-bindgen` on other targets) | 0.9, 0.3, 0.2, 0.8 | MIT / Apache-2.0 (`zerocopy` also BSD-2-Clause) | `cpp_map` | Skip-list level randomisation |
| `approx` | 0.5.1 | Apache-2.0 | `boostvoronoi` | Float comparisons |
| `ordered-float` | 5.5.0 | MIT | `boostvoronoi` | Ordered floats in event queues |
| `libm` | 0.2.16 | MIT | `boostvoronoi` | Portable float math |
| `rustc-hash`, `smallvec`, `vob`, `thiserror` | 2.1, 1.16, 3.0, 2.0 | MIT / Apache-2.0 | `boostvoronoi` | Hashing, small vectors (edge iterators), bit vectors, error types |
| `toml` (+ `serde`, `toml_parser`, `toml_writer`, `winnow`, …) | 0.9 | MIT / Apache-2.0 | `mica-profiles` | User profile files |
| `serde_json` | 1.0 | MIT / Apache-2.0 | `mica-profiles` | Orca JSON profiles |
| QOI format description | Prusa bundled `qoi.h` at `b028299c77` | MIT | `mica-gcode` | Independent Rust encoder and test decoder; no vendored C; notice below |
| prusa-fdm-mixer color model | github.com/prusa3d/prusa-fdm-mixer `0124582` (same code as PrusaSlicer's bundled `prusa_fdm_mixer` at `b028299c77`) | MIT | `mica-core` (`fdm_mixer.rs`), `mica-gui` swatches | Rust port of the C++ model predicting the color of a layer-mixed filament blend, and its 33 tests; no C++ or measured data vendored; notice below |
| heatshrink algorithm | v0.4.1 | ISC | `mica-gcode` | Rust adaptation, 11/4 and 12/4; see notice below |
| MeatPack | libbgcode `d4da9073616d70a43c151e8c1d7fbff879d2e08a` adaptation | BSD-3-Clause | `mica-gcode` | Scott Mudge's G-code packing, ported through libbgcode; see notice below |
| ArcWelder regression cases | PrusaSlicer `b028299c77` tests | AGPL-3.0 | `mica-core` tests | Parameterized Rust adaptation of arc fitting/discretization/clipping cases; ArcWelder carries Brad Hochgesang’s attribution, retained in the geometry source |
| PrusaSlicer cube reference exports | 2.9.6 | AGPL-3.0 (embedded Prusa profile data) | `mica-gcode` tests | User-supplied `.gcode` / `.bgcode` regression fixtures, reused for timing/M73; generated cube/Benchy timing expectations in tests (full new reference outputs remain temporary); hashes and differences in `crates/mica-gcode/tests/fixtures/README.md` |
| OrcaSlicer U1 timing oracle | 2.4.1 | AGPL-3.0 | `mica-profiles` tests | Generated cube timing expectation only; installed profiles and full G-code used at runtime in the system temp directory |

## H.264 decoding (operating system decoders)

Mica contains no H.264 decoder code in shipped builds. RTSP and WebRTC camera video is decoded by the decoder
of the operating system, through `mica-videodec`: VideoToolbox on macOS, the Media Foundation H.264
decoder on Windows, and on Linux whatever H.264 decoder GStreamer finds (VA-API, `gst-libav`,
the OpenH264 plugin, …). Those decoders are licensed to the user by the operating system or
distribution vendor (Apple, Microsoft, the distribution's GStreamer plugin packages). Windows "N"
editions need Microsoft's Media Feature Pack; on Linux a missing decoder plugin is reported in the
camera view ("H.264 camera streams need GStreamer with an H.264 decoder (gst-plugins-bad/libav)").
Linux packages must depend on the GStreamer core libraries (`libgstreamer1.0-0`,
`libgstreamer-plugins-base1.0-0`), which Mica links dynamically; the decoder plugins can stay
optional. OpenH264 compiled from source is only a development option (cargo feature `openh264`,
default off) and must never be enabled for packaged builds.

## Printer bed models and textures (loaded at runtime, not distributed)

The 3D view shows each printer's real bed sheet: PrusaSlicer's `coreone_bed.stl` + `coreone.svg`,
`xl_bed.stl` + `xl.svg` and OrcaSlicer's `Snapmaker U1_bed.stl` + `Snapmaker U1_texture.svg`, as
named by the printer-model sections of the vendor profiles (`bed_model`, `bed_texture`). These files
are **AGPL-3.0 resources of PrusaSlicer and OrcaSlicer**; Mica never copies or bundles them. They are
looked up at runtime (`crates/mica-gui/src/scene/bed_assets.rs`) in the pinned upstream checkouts
(`upstream/prusaslicer/resources/profiles/<Vendor>/`, `upstream/orca/resources/profiles/<Vendor>/`,
ignored by version control) and in the user's installed applications (`/Applications/Original Prusa
Drivers/PrusaSlicer.app`, `/Applications/PrusaSlicer.app`, `/Applications/OrcaSlicer.app`,
`/Applications/Snapmaker Orca.app`, PrusaSlicer's downloaded vendor directory, the Linux and Flatpak
equivalents; `MICA_BED_ASSETS` overrides). Without them the plate falls back to the drawn grid.
The logos in those textures are trademarks of Prusa Research a.s. and Snapmaker; they are shown only
on their own printers' beds, as the vendors' slicers show them, and imply no affiliation.

## Printer product pictures (loaded at runtime, not distributed)

The Add printer window and the My printers cards show each printer model's picture: the
`thumbnail` PNG a PrusaSlicer `[printer_model]` section names (`COREONE_thumbnail.png`,
`XL5IS_thumbnail.png`, …) and OrcaSlicer's `<model name>_cover.png` (`Snapmaker U1_cover.png`).
Like the bed files above these are **AGPL-3.0 resources of PrusaSlicer and OrcaSlicer**, looked up
at runtime in the same places (`crates/mica-gui/src/printhost/catalog.rs` → `scene::bed_assets::
asset_roots`) and never copied or bundled; a model without one gets a neutral drawn silhouette.
The printers pictured and their names and logos are trademarks of Prusa Research a.s. and
Snapmaker; Mica shows them only to identify the printer a profile is for and implies no
affiliation.

## Printer discovery on the local network

"Found on your network" uses Mica's own mDNS / DNS-SD query code (`crates/mica-printhost/src/
discovery.rs`, plain `std::net::UdpSocket`, RFC 6762 / RFC 6763); no mDNS crate was added.

## Ideas adopted without their code

| Feature | Idea from | License of their code | What Mica took |
|---|---|---|---|
| Brick layers (`crates/mica-core/src/brick_layers.rs`) | [Brick Layers](https://github.com/GeekDetour/BrickLayers) by GeekDetour (Everson Siqueira); strength tests by CNC Kitchen | GPL-3.0-or-later | The idea (every other inner wall half a layer higher, raised walls last); re-implemented in the engine, no code translated (`PROVENANCE.md`, `bricklayers`) |

## libc++ `std::sort` element order (Apache-2.0 WITH LLVM-exception)

`crates/mica-core/src/cxx_sort.rs` re-implements the algorithm of libc++'s `std::sort` (LLVM project,
`__algorithm/sort.h`, `sift_down.h`, `pop_heap.h`, `push_heap.h` in the macOS SDK) so that sorts with equal
keys order their elements like the PrusaSlicer reference binary. No libc++ source is shipped; the module
is Mica code in Rust following that algorithm. LLVM's license (Apache-2.0 WITH LLVM-exception) is
compatible with AGPL-3.0-only. Copyright (c) the LLVM Project contributors.

## Built-in models (shipped in the app)

"Add shape" in the object list's "+ Add" menu and the 3D view toolbar's Add menu, and Mica MCP's
`add_shape`, put these on the plate (`crates/mica-core/src/builtin_models.rs`). The basic shapes
(cube, cylinder, cone, sphere, torus, pyramid, wedge, tube, disc) are generated by Mica's own code
(provenance original); no mesh is taken from another slicer. The Credits tab lists the 3DBenchy
under Models.

| File | Source | License | Notes |
|---|---|---|---|
| `crates/mica-core/assets/models/3DBenchy.stl.gz` | `Single-part/3DBenchy.stl` of the official repository [CreativeTools/3DBenchy](https://github.com/CreativeTools/3DBenchy) (git blob `5a51bb5f3f`), downloaded 2026-10-20, gzip-compressed (`gzip -9 -n`) | CC0 1.0 (public domain) | #3DBenchy by Creative Tools (NTI Group), designed by Daniel Norée and Paulo Kiefe. Decompressed it is the official file byte for byte: 11,285,384 bytes, sha256 `6ab57f1c3f8e86bc3cbd302c6fa6270acf06277c6335454e922419c25d42e97e` (a test checks it). The official license page, [3dbenchy.com/license](https://www.3dbenchy.com/license/) (checked 2026-10-20), says the file "is now in the Public Domain" and "licensed under a Creative Commons CC0 1.0 Universal license"; it was CC BY-ND 4.0 until the change in February 2025 ([all3dp.com](https://all3dp.com/4/3dbenchy-is-public-domain-now/), [Fabbaloo](https://fabbaloo.com/?p=228528)). Shown as "Benchy" in the gallery, saved in projects as an object named "3DBenchy". |

## Test models

| File | Source | License | Notes |
|---|---|---|---|
| `tests/models/3DBenchy.stl` | Copy of `resources/shapes/3DBenchy.stl` from PrusaSlicer (`prusaslicer@b028299c77`) | CC0 1.0 | #3DBenchy by Creative Tools / NTI Group; relicensed from CC-BY-ND to CC0 in February 2025 (CAD file April 2025), see [all3dp.com](https://all3dp.com/4/3dbenchy-is-public-domain-now/) and [Wikipedia](https://en.wikipedia.org/wiki/3DBenchy) |
| `tests/models/cube20.stl`, `hole_block.stl`, `thin_features.stl` | Generated for Mica | AGPL-3.0-only (as the project) | |
| `tests/models/taper.stl` | Generated by `tests/models/taper.py` | AGPL-3.0-only (as the project) | Watertight 40 mm taper, 0.15–2.4 mm wide, 5 mm high; bead-transition oracle |
| `crates/mica-core/src/arachne/upstream_tests.rs` geometries | `prusaslicer@b028299c77 tests/libslic3r/test_arachne.cpp` | AGPL-3.0 | All 20 upstream Arachne regressions; code provenance in the register |
| `crates/mica-core/tests/fixtures/arachne-core-one.tsv` | Measurements from PrusaSlicer 2.9.6 exports | AGPL-3.0-only (as the project) | Filament and run counts for 440 layers; commands and residual errors in `docs/arachne.md` |
| `tests/models/support_organic.py` outputs (not checked in) | Generated for Mica | AGPL-3.0-only (as the project) | Original arm-out T and hanging-sphere figurine oracle geometry; no downloaded data |

## Surface textures

| File | Source | License | Notes |
|---|---|---|---|
Bundled height maps for texture skin (`mica_core::texture::catalog`). Only maps with a clear
licence are shipped: the CC0 maps and the Hero Patterns (CC BY 4.0, attribution in the app's
credits data and in the texture gallery). BumpMesh's other maps (no stated licence, or made
with Filter Forge) are not bundled. Mica's procedural textures are generated at run time
(`mica_core::texture::procedural`), and custom images are the user's own, stored in their
projects.

| `crates/mica-core/assets/textures/autumn.png` | "Autumn" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-autumn.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/aztec.png` | "Aztec" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-aztec.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/bamboo.png` | "Bamboo" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-bamboo.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/bank_note.png` | "Bank Note" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-bank-note.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/bark.png` | "Bark" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/bark.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=Bark001 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/bubbles.png` | "Bubbles" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-bubbles.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/cage.png` | "Cage" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-cage.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/connections.png` | "Connections" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-connections.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/current.png` | "Current" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-current.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/curtain.png` | "Curtain" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-curtain.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/death_star.png` | "Death Star" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-death-star.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/endless_clouds.png` | "Endless Clouds" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-endless-clouds.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/eyes.png` | "Eyes" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-eyes.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/falling_triangles.png` | "Falling Triangles" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-falling-triangles.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/flipped_diamonds.png` | "Flipped Diamonds" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-flipped-diamonds.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/floating_cogs.png` | "Floating Cogs" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-floating-cogs.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/formal_invitation.png` | "Formal Invitation" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-formal-invitation.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/four_point_stars.png` | "Four Point Stars" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-four-point-stars.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/groovy.png` | "Groovy" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-groovy.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/happy_intersection.png` | "Happy Intersection" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-happy-intersection.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/hexagon_outline.png` | "Hexagon Outline" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-hexagons.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/intersecting_circles.png` | "Intersecting Circles" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-intersecting-circles.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/leaves.png` | "Leaves" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/leaves.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=ScatteredLeaves007 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/lips.png` | "Lips" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-lips.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/lisbon.png` | "Lisbon" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-lisbon.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/melt.png` | "Melt" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-melt.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/pine_bark.png` | "Pine Bark" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/pineBark.png` (`a6d6d6e6df`), originally https://polyhaven.com/a/pine_bark | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/rain.png` | "Rain" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-rain.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/roof_tiles.png` | "Roof Tiles" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/roofTiles.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=RoofingTiles002 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/roof_tiles_2.png` | "Roof Tiles 2" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/roofTiles_02.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=RoofingTiles004 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/shingles.png` | "Shingles" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/shingles.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=RoofingTiles001 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/signal.png` | "Signal" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-signal.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/speckle.png` | "Speckle" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-texture.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/straw.png` | "Straw" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/straw.png` (`a6d6d6e6df`), originally https://polyhaven.com/a/thatch_roof_angled | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/tiny_checkers.png` | "Tiny Checkers" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-tiny-checkers.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |
| `crates/mica-core/assets/textures/twill.png` | "Twill" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/twill.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=Fabric004 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/twill_2.png` | "Twill 2" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/twill_02.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=Fabric015 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/wicker.png` | "Wicker" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/wicker.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=Wicker001 | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/wicker_2.png` | "Wicker 2" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/wicker_02.png` (`a6d6d6e6df`), originally https://ambientcg.com/view?id=Wicker009A | CC0 1.0 | Resized to at most 512 px (8-bit grey PNG) for Mica |
| `crates/mica-core/assets/textures/yyy.png` | "YYY" from [BumpMesh](https://github.com/CNCKitchen/stlTexturizer) `textures/hero-yyy.png` (`a6d6d6e6df`), based on [Hero Patterns](https://heropatterns.com) by Steve Schoger | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | Pattern made into a height map by CNC Kitchen; resized to at most 512 px (8-bit grey PNG) for Mica; credited in the app (`mica_core::texture::catalog::credits`) |

## Imported data

Printer and filament profiles from upstream slicers are AGPL-3.0 data. Mica does not bundle them: they
are read at runtime from an installed PrusaSlicer or OrcaSlicer, or from the pinned checkouts under
`upstream/` (`scripts/fetch-upstream.sh --profiles`, ignored by version control).

## Heatshrink attribution (ISC)

The Rust implementation was written from the published heatshrink algorithm, as used by libbgcode,
and checked against PrusaSlicer output. It does not vendor or compile the heatshrink C sources.
Upstream: [atomicobject/heatshrink](https://github.com/atomicobject/heatshrink), v0.4.1.

Copyright (c) 2013-2015, Scott Vokes <vokes.s@gmail.com>
All rights reserved.

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT,
OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE,
DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS
ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS
SOFTWARE.

## MeatPack attribution (BSD-3-Clause)

Adaptation of MeatPack G-Code Compression by Scott Mudge, inherited through libbgcode's
`src/LibBGCode/binarize/meatpack.cpp` and `meatpack.hpp`:

- [OctoPrint-MeatPack](https://github.com/scottmudge/OctoPrint-MeatPack)
- [bubnikv's OctoPrint-MeatPack adaptation](https://github.com/bubnikv/OctoPrint-MeatPack)
- [Prusa-Firmware-MeatPack](https://github.com/scottmudge/Prusa-Firmware-MeatPack)

Copyright (c) 2020, Scott Mudge. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software without
   specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## QOI format documentation attribution (MIT)

The Rust encoder and test decoder implement the data format documented in Dominic Szablewski's
[QOI](https://qoiformat.org/) header, read from the pinned PrusaSlicer bundle. No QOI library is linked.

Copyright (c) 2021 Dominic Szablewski

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## prusa-fdm-mixer attribution (MIT)

`crates/mica-core/src/fdm_mixer.rs` is a Rust port of the prusa-fdm-mixer color model
(github.com/prusa3d/prusa-fdm-mixer, `cpp/prusa_fdm_mixer.{hpp,cpp}` and the checks of
`cpp/test_prusa_fdm_mixer.cpp`), the same code PrusaSlicer 2.9.6 bundles. Mixed-color swatches
use it; it never changes G-code.

MIT License

Copyright (c) 2026 Ondrej Bartas (Prusa Research s.r.o.) and contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Additional inherited infill attribution

The PrusaSlicer fill ports retain
Prusa Research and Slic3r author notices. The Hilbert state table was adapted upstream from
Kevin Ryde's Math::PlanePath::HilbertCurve; honeycomb geometry credits David Eccles (gringer).
Lightning retains the upstream CuraEngine notice: Copyright (c) 2021 Ultimaker B.V.;
CuraEngine is released under the AGPLv3 or higher. Orca CrossHatch credits Bambu Lab and
David Eccles. File-level sources are registered in PROVENANCE.md.

The export option table `crates/mica-core/src/threemf/write_mapping.tsv` is a snapshot of the existing
`mica-profiles/src/orca/mapping.tsv`, derived from the pinned Orca and Prusa `PrintConfig.cpp`
(AGPL-3.0-or-later, used under AGPL-3.0-only). It introduces no dependency or external dataset.
It lives in core so project saving has no profile-library dependency.

The quality corpus in `tests/corpus/` adds no libraries or third-party model data. It references
already-attributed `tests/models/` assets, generates original geometry in temporary storage, and
stores only numeric measurements/hashes from locally installed PrusaSlicer/OrcaSlicer oracles.
Private projects and their G-code are never distributed in the corpus.

Orca batch A's `crates/mica-core/src/config/orca_options.tsv` derives types and model data from
`orca@8500fcdcca src/libslic3r/PrintConfig.cpp` (AGPL-3.0). Neutral defaults preserve Prusa behavior.
The small-area default curve and upstream algorithm credit Alexander Þór's GPLv3
[Small-Area-Flow-Comp](https://github.com/Alexander-T-Moss/Small-Area-Flow-Comp), retained in the port.
The optional process schema `crates/mica-core/src/config/orca_options.tsv` records types, defaults
and enum names from `orca@8500fcdcca` `src/libslic3r/PrintConfig.cpp` (AGPL-3.0).
GUI labels/tooltips are regenerated from the same pin by `cargo xtask gen-option-text`.
`docs/orca-options-b-results.json` contains original numeric measurements from local OrcaSlicer
2.4.1 and Mica runs against the repository's existing models and original generated fixtures.
It contains no upstream source, presets, meshes or G-code; Orca source behavior is attributed above.

## Printer connections

- `ureq` 3.4.2 / `ureq-proto` 0.6.4 (MIT OR Apache-2.0), `http`, `httparse`, `bytes`, `base64`, `percent-encoding`, `utf8-zero`: HTTP/JSON transport in `mica-printhost`; fetched in the existing lockfile.
- `rustls` 0.23 / `rustls-webpki`, `rustls-pki-types`, `webpki-roots`, `ring`, `untrusted`, `zeroize`, `subtle`: ureq's existing rustls TLS dependencies and certificate roots (rustls: Apache-2.0 OR ISC OR MIT; rustls-webpki/untrusted: ISC; ring: Apache-2.0 AND ISC; subtle: BSD-3-Clause; webpki-roots: CDLA-Permissive-2.0; remaining helpers: MIT OR Apache-2.0; upstream notices remain in the crates). TLS verification remains enabled.
- Existing `serde`/`serde_json` parse printer responses; existing `toml` 0.9 and `serde` parse the CLI's credential-reference configuration. No new package versions or fixture data were fetched.

Mock HTTP responses in `mica-printhost` are original representative fixtures, not printer captures.
Moonraker adaptations from the pinned OrcaSlicer and PrusaSlicer sources are registered in `PROVENANCE.md`.

## Model library

`crates/mica-library` (the Library tab's index). All versions are pinned exactly in its
`Cargo.toml`; fetched once on 2026-10-17. Pure Rust except the OS file-event bindings.

| Component | Version | License | Used by | Purpose |
|---|---|---|---|---|
| `redb` | 4.4.0 | MIT OR Apache-2.0 | `mica-library` | The library index (`index.redb` in the app's data folder): pure-Rust, ACID, crash-safe embedded key-value store, no further dependencies |
| `xxhash-rust` (feature `xxh3` only) | 0.8.19 | BSL-1.0 | `mica-library` | xxh3-128 content hashes for duplicate detection; pure Rust, no dependencies |
| `notify` (feature `macos_fsevent`) | 8.2.0 | CC0-1.0 | `mica-library` | Live updates from file-system events (FSEvents on macOS, inotify on Linux, ReadDirectoryChangesW on Windows) |
| `notify-types` | 2.1.0 | MIT OR Apache-2.0 | `notify` | Event types |
| `fsevent-sys` | 4.1.0 | MIT | `notify` (macOS) | CoreServices FSEvents bindings |
| `kqueue`, `kqueue-sys`, `mio` 1.2.4 | 1.2.1, 1.1.2, 1.2.4 | MIT | `notify` (BSD/iOS; locked for macOS but only compiled with `macos_kqueue`, which is off) | kqueue backend |
| `inotify`, `inotify-sys` | 0.11.5, 0.1.8 | ISC | `notify` (Linux only) | inotify backend |
| `windows-sys` 0.60, `windows-targets` 0.53 and its `windows_*` target crates | 0.60.2, 0.53.5, 0.53.1 | MIT OR Apache-2.0 | `notify` (Windows only) | Windows API bindings |
| `walkdir`, `libc`, `log`, `bitflags`, `serde`, `serde_json`, `zip`, `rayon` | already in the tree | MIT / Apache-2.0 / Unlicense | `mica-library` | Unchanged versions from the existing lockfile |

The library ships no data: tests, the screenshots, the monkey test and the scan benchmark
build a synthetic library in a temporary folder from the repository's own `tests/models`
files plus generated shapes and G-code.


## G-code option inventory

| Data | Version / source | License | Used by | Purpose |
|---|---|---|---|---|
| Orca G-code option inventory | orca `8500fcdcca`, snapmaker-orca `b1831e5dcb` | AGPL-3.0-only | `mica-gcode` | `src/orca_gcode_options.tsv`: keys, types and nullability derived from the existing checked-in `mica-profiles/src/orca/*schema.json` tables (`PrintConfigDef`); filters translated canonical edits to upstream option names |
