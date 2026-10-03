# ESP32-P4 pre-v3 source patch kit

This kit contains a source patch and a preparation tool for pre-v3 ESP32-P4 support. It is not a replacement HAL crate or a runtime add-on: applications continue to use the normal `esp-hal` crates and API.

## Prepare the source

Prerequisites: Rust 1.95 or newer, Python 3.9 or newer, and Git.

For example, keep the kit, application, and generated source in sibling directories:

```text
work/
├── esp32p4-pre-v3-kit/
├── app/
└── prepared/                 # created by the command
```

From the kit root, run:

```sh
cargo xtask prepare ../prepared
```

The tool fetches the exact upstream `esp-hal` base revision `e02f3613e9f9ba1ce00070eb387e3bf4fde2267b`, applies the bundled patch, and places the checkout at `../prepared/upstream`. It also writes `../prepared/Cargo.patch.toml` with the Cargo path overrides. Relative destinations resolve from your current directory. The destination's parent directory must exist, and the destination itself must not already exist. The tool does not overwrite existing directories or use a shared source cache.

Merge the `[patch.crates-io]` entries from `../prepared/Cargo.patch.toml` into the application's workspace-root `Cargo.toml` (merge with an existing table if needed). The generated entries use absolute paths. For the sibling-directory layout above, the equivalent relative paths are:

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

Set the minimum chip revision for pre-v3 silicon (`major * 100 + minor`; revision 1.3 is `103`) when building:

```sh
ESP_HAL_CONFIG_MIN_CHIP_REVISION=103 cargo build --release --target riscv32imafc-unknown-none-elf --features esp32p4
```

If unset, the setting defaults to `300` (v3+), so explicitly set it for pre-v3 chips.

## Scope and limits

The patch covers pre-v3 CLIC interrupts, revision-dependent clocks, UART0–UART4 clocking and baud dividers, and ROM linker-table selection. It is for the single source snapshot pinned in `patches/manifest.json`; the preparation tool verifies the base commit and patch checksum. It does not follow a moving upstream branch or promise rolling updates. It is not general support for every `esp-hal` 1.1.0 source.

The preparation command was exercised against the official upstream repository. A standalone UART consumer using its generated path overrides was build/link checked at revisions `103` and `300` (`0x12c`), including linked ROM addresses and one prepared source for all ten coupled crates. Boot, ROM execution, UART timing, RcFast accuracy, and PSRAM have not been hardware-validated; other peripherals and board variants are not certified.

Based on [esp-rs/esp-hal](https://github.com/esp-rs/esp-hal). Pre-v3 CLIC and CPLL changes are credited to [hatomist](https://github.com/hatomist/esp-hal); the UART and ROM-table changes are in this kit's patch. See the [upstream UART example at the pinned base revision](https://github.com/esp-rs/esp-hal/blob/e02f3613e9f9ba1ce00070eb387e3bf4fde2267b/examples/interrupt/uart/src/main.rs). The source is provided under the included [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) license.
