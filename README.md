# ESP32-P4 pre-v3 source patch kit

This kit contains a source patch and a preparation tool for pre-v3 ESP32-P4 support. It is not a replacement HAL crate or a runtime add-on: applications continue to use the normal `esp-hal` crates and API.

## Prepare the source

Prerequisites: Rust 1.95 or newer, and Git.

For example, keep your application and the generated source in sibling directories:

```text
work/
├── app/
└── prepared/                 # created by the command
```

Install the CLI tool:

```sh
cargo install cargo-esp32p4-pre-v3
```

Then from the `work` directory (alongside `app`), run the prepare command:

```sh
cargo esp32p4-pre-v3 prepare ./prepared
```

For local development within the kit repository, use `cargo run -- prepare ../prepared`.

The tool fetches the exact upstream `esp-hal` base revision `e02f3613e9f9ba1ce00070eb387e3bf4fde2267b` via Git, applies the embedded patch, and places the checkout at `./prepared/upstream`. It also writes `./prepared/Cargo.patch.toml` with the Cargo path overrides. Relative destinations resolve from your current directory. The destination's parent directory must exist, and the destination itself must not already exist. The tool does not overwrite existing directories or use a shared source cache. The patch and base manifest are embedded directly in the single Rust binary; an original kit checkout or Python setup is not required after installation.

Merge the `[patch.crates-io]` entries from `./prepared/Cargo.patch.toml` into the application's workspace-root `Cargo.toml` (merge with an existing table if needed). The generated entries use absolute paths. For the sibling-directory layout above, the equivalent relative paths are:

```toml
[patch.crates-io]
esp-hal = { path = "../prepared/upstream/esp-hal" }
esp-backtrace = { path = "../prepared/upstream/esp-backtrace" }
esp-bootloader-esp-idf = { path = "../prepared/upstream/esp-bootloader-esp-idf" }
esp-println = { path = "../prepared/upstream/esp-println" }
```

These are local path overrides into the prepared checkout, not Git dependencies on this kit. Keep the normal crate dependencies and features in the application; for example:

```toml
[dependencies]
critical-section = "1.1.3"
esp-backtrace = { version = "=0.19.0", features = ["panic-handler", "println"] }
esp-bootloader-esp-idf = "=0.5.0"
esp-hal = { version = "=1.1.0", features = ["log-04", "unstable"] }
esp-println = { version = "=0.17.0", features = ["log-04"] }

[features]
esp32p4 = ["esp-backtrace/esp32p4", "esp-bootloader-esp-idf/esp32p4", "esp-hal/esp32p4", "esp-println/esp32p4"]
```

Install the RISC-V target `riscv32imafc-unknown-none-elf` and pass the linker script in the application's `.cargo/config.toml`:

```toml
[target.'cfg(target_arch = "riscv32")']
rustflags = ["-C", "link-arg=-Tlinkall.x", "-C", "force-frame-pointers"]
```

**Warning: Non-monotonic binary compatibility**
ESP32-P4 binaries are strictly non-monotonic across the v3 boundary. A binary built for pre-v3 (e.g., `103`) uses a non-standard memory-mapped CLIC and is incompatible with v3+ silicon. A binary built for v3+ (e.g., `300`) emits CSR `0x347` instructions which trap as illegal instructions on pre-v3 silicon. There is no graceful fallback; do not flash the wrong family.

Set the minimum chip revision for your target silicon (`major * 100 + minor`; revision 1.3 is `103`) via the HAL revision environment variable. Because Cargo does not always invalidate builds correctly when this environment variable changes, it is strongly recommended to use separate target directories for each revision to avoid stale configurations:

```sh
# Build for pre-v3 silicon (e.g., rev 1.3)
ESP_HAL_CONFIG_MIN_CHIP_REVISION=103 CARGO_TARGET_DIR=target/rev103 cargo build --release --target riscv32imafc-unknown-none-elf --features esp32p4

# Build for v3+ silicon (e.g., rev 3.0)
ESP_HAL_CONFIG_MIN_CHIP_REVISION=300 CARGO_TARGET_DIR=target/rev300 cargo build --release --target riscv32imafc-unknown-none-elf --features esp32p4
```

If unset, the revision strictly defaults to `300` (v3+), which will not run on pre-v3 chips. Both `esp-hal` and patched `esp-sync` read this same `ESP_HAL_CONFIG_MIN_CHIP_REVISION` variable to ensure synchronized lock semantics across crates.

## Checks

From the kit root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --test consumer -- --ignored --test-threads=1
cargo package --list
cargo publish --dry-run
```

Consumer checks are run as an ignored test and require the RISC-V target, network access, and `llvm-objdump` (`rustup component add llvm-tools-preview`). CI runs these same host and consumer checks, as well as a dry-run check of the package structure (`publish --dry-run` performs validation only; no actual publication occurs). If testing packaging with uncommitted changes locally, append `--allow-dirty` to both the `cargo package` and `cargo publish` commands.

## Scope and limits

The patch covers pre-v3 CLIC interrupts, revision-dependent clocks, UART0–UART4 clocking and baud dividers, and ROM linker-table selection. It is for the single source snapshot pinned in `patches/manifest.json`; the preparation tool verifies the base commit and patch checksum. It does not follow a moving upstream branch or promise rolling updates. It is not general support for every `esp-hal` 1.1.0 source.

The preparation command was exercised against the official upstream repository. A minimal HAL consumer was build/link checked for revisions `103` and `300`; disassembly of its retained `esp-sync` lock routine showed CSR `0x347` absent for `103` and present for `300`.

### ESP32-P4 v1.3 limited hardware smoke test: PASS

Tested on one ESP32-P4 silicon revision v1.3 board (40 MHz crystal, 32 MB flash) with a rev103 build on 2026-10-03:

- Firmware booted successfully.
- 600 timer interrupt callbacks completed.
- 100,000 `SingleCoreInterruptLock` enter/exit pairs completed.
- 100,000 critical-section iterations completed.
- No lock-invariant failure was reported; periodic locked intervals checked that timer callbacks did not run while interrupts were masked.
- No illegal-instruction trap was observed during the completed run.

Final output:

```text
HIL_PROGRESS ticks 600 lock_iterations 100000 cs_iterations 100000
HIL_PASS
```

Output used USB Serial/JTAG, not peripheral UART. The capture contains a USB-UART reset before the successful run; PASS applies to the complete run after that reset.

This is a bounded single-core smoke test, not general peripheral certification or full ESP32-P4 v1.3 support. Peripheral UART TX/RX, baud-rate changes, RcFast/delay calibration, dual-core stress, PSRAM, ROM-table coverage, and other peripherals remain unvalidated.

**Hardware validation checklist:**
- [x] boot on one v1.3 board
- [x] timer interrupt smoke
- [x] single-core interrupt-lock/critical-section loop smoke
- [ ] UART TX/RX
- [ ] baud change
- [ ] delay calibration
- [ ] dualcore interrupt stress

Based on [esp-rs/esp-hal](https://github.com/esp-rs/esp-hal). Pre-v3 CLIC and CPLL changes are credited to [hatomist](https://github.com/hatomist/esp-hal); the UART and ROM-table changes are in this kit's patch. See the [upstream UART example at the pinned base revision](https://github.com/esp-rs/esp-hal/blob/e02f3613e9f9ba1ce00070eb387e3bf4fde2267b/examples/interrupt/uart/src/main.rs). The source is provided under the included [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) license. For ROM table derivations, see [PROVENANCE.md](PROVENANCE.md).
