# ESP32-P4 rev103 deep hardware fixture

This standalone, non-publishable fixture preserves the four Rust sources and
locked dependency graph exercised by the [deep hardware validation record](../../README.md#esp32-p4-v13-deep-hardware-validation-pass).
It is diagnostic firmware, not an application example or a certification suite.
The recorded experiment used one ESP32-P4 v1.3 board with a 40 MHz crystal and
32 MB flash on 2026-10-03: one 90 MHz run, one 180 MHz run, and three 360 MHz runs
(the last two were reset-only repeats). Promoting these sources does not claim a
new hardware result.

Only the fixture sources, Cargo configuration, lockfile, and this reproduction
guide belong in the repository. Prepared SDK checkouts, build outputs, ELF
images, disassembly, capture logs, result manifests, and flash backups remain in
ignored `target/` directories or outside the repository. The historical generated
diagnostic directory remains historical evidence; do not edit it or force-add
ignored artifacts.

## Prepare and build (no device writes)

Commands below use a POSIX shell and start at the repository root. The measured
toolchain was Rust 1.98.1; install/select it and its RISC-V target and LLVM tools:

```sh
REPO_ROOT=$(pwd -P)
export REPO_ROOT
rustup toolchain install 1.98.1 --profile minimal
export RUSTUP_TOOLCHAIN=1.98.1
rustup target add riscv32imafc-unknown-none-elf
rustup component add llvm-tools-preview
```

Prepare a fresh pinned SDK using the root CLI:

```sh
mkdir -p target
cargo run --bin cargo-esp32p4-pre-v3 -- prepare target/hil-hardware-deep
```

The destination must not already exist; preparation deliberately does not
overwrite it. It creates `target/hil-hardware-deep/upstream` at the kit's pinned
upstream revision `e02f3613e9f9ba1ce00070eb387e3bf4fde2267b` with the embedded
compatibility patch. The fixture's five direct SDK path dependencies and its ROM
table reader use `../../target/hil-hardware-deep/upstream`. No SDK or compatibility
patch change is required, and no generated absolute-path patch table needs to be
merged into this fixture.

**Build from the fixture directory.** Cargo loads `.cargo/config.toml` from the
working directory and its ancestors, not from the directory named by
`--manifest-path`. A root-directory invocation with only `--manifest-path` would
miss this fixture's target, linker flags, and revision configuration.

```sh
(
  cd "$REPO_ROOT/tests/hil-hardware-deep" || exit 1
  ESP_HAL_CONFIG_MIN_CHIP_REVISION=103 \
    CARGO_TARGET_DIR="$REPO_ROOT/target/hil-hardware-deep-build-90" \
    cargo build --locked --release --no-default-features --features clock-90
)
(
  cd "$REPO_ROOT/tests/hil-hardware-deep" || exit 1
  ESP_HAL_CONFIG_MIN_CHIP_REVISION=103 \
    CARGO_TARGET_DIR="$REPO_ROOT/target/hil-hardware-deep-build-180" \
    cargo build --locked --release --no-default-features --features clock-180
)
(
  cd "$REPO_ROOT/tests/hil-hardware-deep" || exit 1
  ESP_HAL_CONFIG_MIN_CHIP_REVISION=103 \
    CARGO_TARGET_DIR="$REPO_ROOT/target/hil-hardware-deep-build-360" \
    cargo build --locked --release --no-default-features --features clock-360
)
```

Each ELF is at
`target/hil-hardware-deep-build-MHz/riscv32imafc-unknown-none-elf/release/hardware-deep`.
The default feature is `clock-90`; exactly one clock feature must be selected.
For a normal default build, this is also valid:

```sh
(
  cd "$REPO_ROOT/tests/hil-hardware-deep" || exit 1
  cargo build --locked --release
)
```

For that default command, first ensure there are no inherited overrides of
`CARGO_TARGET_DIR` or `ESP_HAL_CONFIG_MIN_CHIP_REVISION`. The checked-in config
selects `riscv32imafc-unknown-none-elf`, `-Tlinkall.x`, frame pointers, revision
`103`, and the root output directory `target/hil-hardware-deep-build-90`.
All profiles must keep revision `103`; `esp-hal` and patched `esp-sync` share
`ESP_HAL_CONFIG_MIN_CHIP_REVISION`. There is no separate ESP_SYNC revision knob.
These pre-v3 binaries must not be used on v3+ silicon, nor may a v3+ binary be
flashed to this v1.3 board.

Keep `[profile.release] lto = "off"`. With Rust 1.98.1, local ThinLTO lost the F
extension for the HAL's generic naked second-core trampoline and rejected its
`fscsr` instruction. The exercised diagnostic profile disabled LTO; it did not
remove floating-point state handling or alter the SDK/compatibility patch. Do
not re-enable LTO to make this a different reproduction.

## Inspect every image before flashing

Locate the `llvm-objdump` installed for the selected Rust toolchain and produce
whole-ELF code disassembly and symbol tables for **all three** profiles:

```sh
LLVM_OBJDUMP=$(find "$(rustc --print sysroot)/lib/rustlib" -type f -name llvm-objdump)
test -n "$LLVM_OBJDUMP" && test -x "$LLVM_OBJDUMP" || exit 1
for MHZ in 90 180 360; do
  OUT="$REPO_ROOT/target/hil-hardware-deep-build-$MHZ"
  ELF="$OUT/riscv32imafc-unknown-none-elf/release/hardware-deep"
  "$LLVM_OBJDUMP" --disassemble --no-show-raw-insn --print-imm-hex "$ELF" \
    > "$OUT/hardware-deep.disassembly.txt" || exit 1
  "$LLVM_OBJDUMP" --syms "$ELF" > "$OUT/hardware-deep.symbols.txt" || exit 1
done
```

Review all executable code, not just the retained lock routine. Inspect the CSR
operand of every CSR instruction, including aliases such as `csrr` and `csrw`:
there must be **no CSR operand `0x347` (decimal `839`)**. An instruction address,
branch target, or unrelated immediate containing those digits is not a CSR
operand. Resolve any undecoded executable instructions before accepting the
inspection. The historical whole-ELF inspections found none in any profile;
that observation is not a substitute for checking newly built images.

Check each image's selected ROM symbols against the prepared
`esp-rom-sys/ld/esp32p4-eco0_4/rom/esp32p4.rom.ld` and `additional.ld` tables:

| Symbol | Expected address in the pinned rev103 tables |
| --- | --- |
| `ets_delay_us` | `0x4fc0003c` |
| `crc32_le` | `0x4fc005f8` |
| `crc32_be` | `0x4fc00604` |
| `MD5Init` | `0x4fc005ec` |
| `MD5Update` | `0x4fc005f0` |
| `MD5Final` | `0x4fc005f4` |
| `memset` | `0x4fc00268` |
| `memcpy` | `0x4fc0026c` |
| `memmove` | `0x4fc00270` |
| `memcmp` | `0x4fc00274` |

`build.rs` requires the exact rev103 profile and derives runtime address constants
from those prepared tables. The firmware additionally checks the selected
function addresses and exercises their behavior. Missing or mismatching ELF
symbols are a stop condition, not a reason to bypass the check.

## Device safety and full backup

**Stop before any device write unless the operator has explicitly authorized it.**
Install the exercised flasher version if needed:

```sh
cargo install espflash --version 4.6.0 --locked
espflash --version
espflash list-ports
```

Choose the actual board port from that list (the example below is not automatic
device selection). Close any other process owning it. Identify the physical board
and read its revision, crystal, flash capacity, and security flags:

```sh
PORT=/dev/ttyACM0 # replace with the verified board's port
espflash board-info --port "$PORT" --non-interactive
```

Proceed only for the intended ESP32-P4 **revision v1.3**, **40 MHz crystal**, and
**32 MB flash**, with **secure boot and flash encryption disabled**, matching the
measured scope. Board identification/backup can reset the target and load a RAM
stub even though they do not write flash. Unknown or enabled security flags,
wrong revision/capacity, or ambiguous device identity are stop conditions. Do not
change security settings or write eFuses. Do not use `--force`, erase the whole
chip, attach external GPIO/UART loopback wiring, or enable/test PSRAM.

Before the first flash write, make a complete 33,554,432-byte backup outside the
repository. This is a **separate, explicitly authorized operator command**, not
an automatic prelude to flashing:

```sh
BACKUP_DIR="$HOME/esp32p4-hil-backups"
mkdir -p "$BACKUP_DIR"
BACKUP="$BACKUP_DIR/before-deep-$(date +%Y%m%d-%H%M%S).bin"
espflash read-flash --port "$PORT" --non-interactive 0 33554432 "$BACKUP"
wc -c < "$BACKUP"
shasum -a 256 "$BACKUP"
```

Require an error-free read and exactly `33554432` bytes; retain the checksum and
board identification with the backup. A partial read is not a backup. Keep the
backup private and outside version control. Do not continue on backup failure.

## Explicit flash and bounded manual capture

After all image checks and the backup succeed, choose one already-built profile.
Run this command only under the separate authorization to overwrite flash:

```sh
MHZ=90 # choose 90, 180, or 360; do not rebuild implicitly while flashing
IMAGE="$REPO_ROOT/target/hil-hardware-deep-build-$MHZ/riscv32imafc-unknown-none-elf/release/hardware-deep"
espflash flash --port "$PORT" --min-chip-rev 1.3 --flash-size 32mb \
  --after no-reset --monitor --non-interactive "$IMAGE"
```

These are [espflash 4.6.0](https://github.com/esp-rs/espflash/tree/v4.6.0)
arguments. `--after no-reset` controls the flash operation; the non-interactive
monitor then initiates its own reset. Do not add `--no-reset` to this monitor.
Record the full terminal output manually using your terminal's capture facility;
this fixture supplies no automated flasher, runner, or PTY script.

Before starting a run, choose a finite **120–180 second manual deadline** (180
seconds is appropriate for the 90 MHz profile). Count from the firmware-start
reset, or monitor startup if no boot is captured, not from the beginning of the
flash transfer. The accepted historical runs took about 72–77 seconds on the
host; those durations are observations, not guaranteed time limits.

A complete accepted run must contain exactly one matching boot line:

```text
HIL_BOOT_DEEP rev103 cpu_mhz=90
```

For other images, the MHz must instead be `180` or `360`. Require all phase output,
including the following summaries, and exactly one final PASS:

```text
HIL_UART_SUMMARY matrix_vectors=360 timing_vectors=120 negative_configs=25 recovery_vectors=25 irq_checks=6
HIL_PROGRESS: UART completed_vectors=360
HIL_PROGRESS: fixed_cs core0=500000 core1=500000 protected=1500000 expected=1500000 nested_cs_both=true
HIL_PROGRESS: AppCpu_stalled=true ProCpu_running=true T1_stopped=true
HIL_PROGRESS: all phases complete rom_checks=532504 uart_vectors=360 fixed_counter_value=1500000 soak_duration_s=10
HIL_PASS_DEEP
```

Also verify the raw-lock counts and the soak's printed protected/expected counter
are exact matches. Soak iteration counts and elapsed time vary; do not replace
them with historical numbers. `HIL_PASS_DEEP` does **not** terminate the firmware
or monitor: the firmware idles and the CLI keeps blocking after PASS. Use
**Ctrl+C** to close the monitor and release the port after the complete result.

Stop and retain the capture on `HIL_FAIL`, panic, trap, unexpected reset, wrong
boot profile, incomplete counters, or no PASS before the deadline. Do not retry
a failing workload, suppress its output, or treat monitor exit status as proof
of PASS. Non-interactive startup can lose the earliest boot output; a capture
with PASS but no boot line is **incomplete and rejected**, not an accepted run.
Diagnose capture readiness separately rather than relabeling it successful.

### Planned reset-only repeats

To reproduce the two recorded 360 MHz repeats, leave the checked 360 MHz image
installed and attach an **interactive** monitor without reflashing:

```sh
MHZ=360
IMAGE="$REPO_ROOT/target/hil-hardware-deep-build-$MHZ/riscv32imafc-unknown-none-elf/release/hardware-deep"
espflash monitor --port "$PORT" --after no-reset --elf "$IMAGE"
```

At initial attachment, espflash 4.6.0 displays `Commands:` and may hold the RAM
stub; it does **not** automatically start the firmware in this interactive mode.
Wait until the monitor is ready for input, then press **Ctrl+R once** to start the
first reset-only run and its manual deadline. After its complete PASS, press
**Ctrl+R once** for the second planned repeat. Preserve separate run boundaries;
each repeat requires exactly one boot and the same full checks as above. Do not
reset during a workload or repeatedly press reset to chase PASS. Ctrl+C closes
the interactive monitor after the second complete result.

## Coverage and limits

Per complete run, the preserved firmware checks:

- **Internal UART0–4:** 60 configurations across XTAL, PLL_F80M, and RC_FAST;
  baud rates 9,600, 115,200, 1,000,000, and 2,000,000; 360 loopback vectors with
  lengths 1, 127, 128, 129, 513, and 1,024 bytes. It also completes 120 timing
  vectors, 25 invalid-configuration checks, 25 recovery vectors, and six UART1
  interrupt checks. These are internal loopbacks, not external signal tests.
- **Dual-core interrupts and locks:** 1,000,000 raw-lock pairs on core0 and
  100,000 on core1, nested raw-lock/critical-section checks, and 1,000 remote
  progress/IRQ probes while core0 is masked. Fixed cross-core critical sections
  perform 500,000 entries per core, incrementing by 2 and 1 for an exact shared
  counter of 1,500,000. A 10-second dual-core/dual-IRQ soak checks independent
  iteration counts and an exact weighted shared counter. Cooperative DONE/guard
  handoff stops TIMG1 and stalls AppCpu outside locks while ProCpu keeps running.
- **Selected ROM and internal SRAM:** CRC32 and MD5 known vectors, the ten
  selected ROM addresses, `memset`, `memcpy`, overlapping `memmove` in both
  directions, and `memcmp` equality/unsigned ordering. ROM delays of 100, 1,000,
  and 10,000 microseconds are measured against XTAL-derived time. A 32 KiB
  internal SRAM region is checked with 65 patterns; the final ROM/SRAM count is
  532,504 completed unit checks.

Clock features select a **software CPU profile**; the boot MHz label is not an
independent physical frequency measurement. UART timing and ROM delay checks
use XTAL-derived references, not a calibrated external instrument. RC_FAST's
recorded measured/nominal XTAL-relative ratio was approximately **0.85**
(0.847756–0.849858). Passing internal loopback therefore does not certify
absolute baud accuracy. Do not change tolerances or the SDK to hide this result.

External GPIO/UART routing and signals, absolute external baud/delay calibration,
PSRAM, LP_UART, unselected ROM APIs (including the f64 ABI), other peripherals,
power-cycle/reset-during-workload behavior, and long-duration thermal/reliability
certification remain unvalidated. The root README is the measured-scope record;
these reproduction instructions do not extend its claims.

## Final device state

Closing the monitor does not restore the original firmware. The last flashed
diagnostic remains on the device; after PASS, AppCpu is stalled with TIMG1
stopped, while ProCpu remains in the PASS idle loop with TIMG0 active. The recorded
experiment left the 360 MHz diagnostic installed. Restore the full backup only
if the operator explicitly requests that separate write operation and the board's
security state permits the appropriate restoration procedure. No blanket raw
restore recipe is provided for encrypted/secure devices, and security protections
must never be bypassed with force flags or eFuse changes.
