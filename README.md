# ESP32-P4 pre-v3 source patch kit
[![cargo-esp32p4-pre-v3 on crates.io](https://img.shields.io/crates/v/cargo-esp32p4-pre-v3)](https://crates.io/crates/cargo-esp32p4-pre-v3) [![esp-hal-p4-pre-v3 on crates.io](https://img.shields.io/crates/v/esp-hal-p4-pre-v3)](https://crates.io/crates/esp-hal-p4-pre-v3) [![docs.rs](https://img.shields.io/docsrs/cargo-esp32p4-pre-v3)](https://docs.rs/cargo-esp32p4-pre-v3)

Source patch + preparation tool for pre-v3 ESP32-P4 silicon (rev < 3.0).
Most users never run the tool — use the published package below with the normal `esp-hal` API.

## Quick start

```sh
espflash board-info
```

- Rev 3.0+: use upstream `esp-hal`. Stop here.
- Rev below 3.0: depend on the compatibility package:

```toml
[dependencies]
esp-hal = { package = "esp-hal-p4-pre-v3", version = "=1.1.0-p4v13.2", features = ["esp32p4"] }
```

> Only add `esp-hal-p4-pre-v3`. The other 11 `*-p4-pre-v3` crates on crates.io are transitive lockstep deps pulled in automatically — do not add them manually or mix versions.

Build (rev 1.3 = `103`):

```sh
rustup target add riscv32imafc-unknown-none-elf
ESP_HAL_CONFIG_MIN_CHIP_REVISION=103 CARGO_TARGET_DIR=target/rev103 \
  cargo build --release --target riscv32imafc-unknown-none-elf --features esp32p4
```

`.cargo/config.toml`:

```toml
[target.'cfg(target_arch = "riscv32")']
rustflags = ["-C", "link-arg=-Tlinkall.x", "-C", "force-frame-pointers"]
```

Rules: `MIN_CHIP_REVISION` unset defaults to `300` (v3+) and will not run on pre-v3. Pre-v3 and v3+ binaries are incompatible across the v3 boundary (pre-v3 uses a non-standard CLIC; v3+ emits CSR `0x347` which traps on pre-v3) — no fallback, do not flash the wrong family. Use a separate `target-dir` per revision; Cargo does not always invalidate on env change. Do not force `linker = "rust-lld"`.

## Published packages

Rev 3.0+ uses upstream `esp-hal`, pre-v3 uses [`esp-hal-p4-pre-v3`](https://crates.io/crates/esp-hal-p4-pre-v3) ([docs](https://docs.rs/esp-hal-p4-pre-v3)). Tool crate: [`cargo-esp32p4-pre-v3`](https://crates.io/crates/cargo-esp32p4-pre-v3). Feature profiles: `esp32p4`, `critical-section` + `unstable`, `log-04`, `defmt`, and their maximal valid combination (`log-04` and `defmt` are mutually exclusive). The 12-crate closure and per-version BOMs live in `releases/esp-hal-p4-pre-v3/`.

## Advanced: rebuild / audit from source
> Registry users stop here. Everything below is ONLY for the `prepare` path (local rebuild/audit) — skip it if you depend on `esp-hal-p4-pre-v3` from crates.io.

Prerequisites: Rust 1.95+, Git. Layout:

```text
work/
├── app/
└── prepared/   # created by the command
```

```sh
cargo install cargo-esp32p4-pre-v3
cd work && cargo esp32p4-pre-v3 prepare ./prepared
```

This fetches upstream `esp-hal@e02f3613e9f9ba1ce00070eb387e3bf4fde2267b`, verifies the patch checksum, applies it to `./prepared/upstream`, and writes `./prepared/Cargo.patch.toml` (absolute path entries). Destination must not exist; nothing is overwritten. Maintainer-only release cut:

```sh
cargo esp32p4-pre-v3 distribute PREPARED METADATA_JSON DEST
```

Merge the `[patch.crates-io]` entries into the app's workspace-root `Cargo.toml`:

```toml
[patch.crates-io]
esp-hal = { path = "../prepared/upstream/esp-hal" }
esp-backtrace = { path = "../prepared/upstream/esp-backtrace" }
esp-bootloader-esp-idf = { path = "../prepared/upstream/esp-bootloader-esp-idf" }
esp-println = { path = "../prepared/upstream/esp-println" }
```

App deps stay as upstream versions (e.g. `esp-hal = "=1.1.0"`); the `[patch]` table above redirects them to the prepared checkout. No `package = "esp-hal-p4-pre-v3"` rename on this path.

## Checks

Checks run in CI — see `.github/workflows/ci.yml` (fmt, clippy, host + consumer tests, release build, package/publish dry-run).

## Scope and limits

Patch covers pre-v3 CLIC, revision-dependent clocks, UART0–UART4 clocking/baud dividers, and ROM linker-table selection, for the single snapshot pinned in `patches/manifest.json`. No rolling upstream tracking; not general support for every `esp-hal` 1.1.0 source.

Hardware: smoke + deep validation PASS on one v1.3 board (40 MHz crystal, 2026-10-03) — boot, timer IRQs, `SingleCoreInterruptLock` loops, internal UART0–4 loopback, dual-core lock stress, selected ROM checks; no `0x347` in rev103 images. Full logs and repro: [tests/hil-hardware-deep](tests/hil-hardware-deep/README.md). External routing/baud calibration, PSRAM, LP_UART, unselected ROM APIs, power/reset and thermal behavior remain unvalidated.

Based on [esp-rs/esp-hal](https://github.com/esp-rs/esp-hal). Pre-v3 CLIC/CPLL via [hatomist](https://github.com/hatomist/esp-hal); UART + ROM tables in this kit's patch ([example at pinned rev](https://github.com/esp-rs/esp-hal/blob/e02f3613e9f9ba1ce00070eb387e3bf4fde2267b/examples/interrupt/uart/src/main.rs)). License [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE); ROM derivations in [PROVENANCE.md](PROVENANCE.md).
