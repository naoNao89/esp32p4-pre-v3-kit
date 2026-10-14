# ESP32-P4 v1.3 troubleshooting (Rust, `esp-hal`)

For ESP32-P4 revision 1.3 boards (`espflash board-info` reports `103`). Upstream
`esp-hal` supports revision 3.0+ only; on pre-v3 silicon use
[`esp-hal-p4-pre-v3`](https://crates.io/crates/esp-hal-p4-pre-v3) (= same API,
compatibility build from this repo).

## Symptoms and causes

| Symptom on a v1.3 board | Cause | Fix |
|---|---|---|
| Illegal-instruction trap on boot or in locks | Binary built for v3+ emits CSR `0x347` (`mintthresh`), absent on pre-v3 | Build with `ESP_HAL_CONFIG_MIN_CHIP_REVISION=103` (unset defaults to `300` = v3+) |
| Interrupts never fire / wrong priority behavior | Pre-v3 uses a non-standard memory-mapped CLIC; v3+ layout differs | Same fix: rev103 build of `esp-hal-p4-pre-v3` |
| UART baud wrong / RX garbage on UART0–4 | Pre-v3 clock tree and baud dividers differ | Same fix; validated sources are XTAL, PLL_F80M, RC_FAST (no external baud certification) |
| Changed the revision flag but behavior didn't change | Cargo doesn't always invalidate on env change | Use a separate target dir per revision, e.g. `CARGO_TARGET_DIR=target/rev103` |
| Link errors in runtime startup objects | Custom `linker = "rust-lld"` override in cargo config | Remove the override; pass `-C link-arg=-Tlinkall.x -C force-frame-pointers` via `rustflags` as in the README |
| Works on v3 board, fails on v1.3 (or reverse) | Binaries are strictly non-monotonic across the v3 boundary | There is no fallback: build one binary per silicon family, never flash cross-family |

## Choosing the crate

- `espflash board-info` → rev 3.0+: upstream `esp-hal`, nothing here is for you.
- Rev below 3.0 (e.g. 1.3): `esp-hal = { package = "esp-hal-p4-pre-v3",
  version = "=1.1.0-p4v13.2", features = ["esp32p4"] }` and build with
  `ESP_HAL_CONFIG_MIN_CHIP_REVISION=103`, target
  `riscv32imafc-unknown-none-elf`. Only add that one dep; the other 11
  `*-p4-pre-v3` crates are transitive.
- `= ` pin is deliberate: an unreviewed HAL upgrade on pre-v3 silicon can
  reintroduce a trap. Bump only after checking validation notes.

## How to confirm the right variant

Disassemble the lock routine and check CSR `0x347` (`mintthresh`):

```sh
llvm-objdump -d --disassemble-symbols=test_lock target/rev103/.../your_firmware | grep -i '347\|mintthresh'
```

- rev103 build: no `0x347` — correct for v1.3.
- rev300 build: `0x347` present — v3+ only, will trap on v1.3.

(The repo's ignored `consumer` test automates exactly this check for both revisions.)

## What is (and isn't) validated

Validated on one v1.3 board (40 MHz crystal, 2026-10-03): boot, 600 timer
IRQs, 100k lock enter/exit pairs, internal UART0–4 loopback across three clock
sources, dual-core lock stress with exact counters, selected ROM delay/mem checks.
Full logs: [tests/hil-hardware-deep](tests/hil-hardware-deep/README.md).

Explicitly unvalidated: external GPIO/UART routing and absolute baud
calibration, PSRAM, LP_UART, unselected ROM APIs (incl. f64 ABI), other
peripherals, power-cycle / reset-during-workload, long-duration thermal.
