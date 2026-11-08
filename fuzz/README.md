# Security fuzzing

The `threemf` target has four input modes:

- raw bytes fuzz ZIP framing and compression errors;
- `M3MF` followed by XML wraps that XML in a valid 3MF package;
- `MREL` wraps relationship XML beside a valid model;
- `MCFG` embeds project configuration beside a valid model.

It is a separate Cargo workspace so nightly/libFuzzer dependencies never enter release builds.
Install nightly and `cargo-fuzz`, then run:

```sh
rustup toolchain install nightly
cargo install --locked cargo-fuzz
cargo +nightly fuzz run threemf fuzz/corpus-seed/threemf -- \
  -dict=fuzz/dictionaries/threemf.dict -max_len=262144 -timeout=5 -max_total_time=600
```

Crashing inputs are written under `fuzz/artifacts/` and can be replayed by passing the artifact
path after the target name. Keep minimized regressions as ordinary deterministic tests whenever
possible.
