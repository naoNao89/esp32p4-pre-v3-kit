# Provenance

This document records the exact origins and checksums of the ESP32-P4 pre-v3 ROM tables included in this kit's source patch.

## ROM Tables (Copied)

The pre-v3 ROM linker tables are adapted directly from the `espressif/esp-idf` primary upstream repository. 
They are distributed under the [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0) (the standard ESP-IDF license for these files).

The files are pulled from two different ESP-IDF commits due to upstream renaming and copyright updates. In the `esp-rom-sys` component of the patch, the files with `eco0_4` suffixes were renamed to their standard names in `esp32p4-eco0_4/rom/`. 

Below is the reproducible verification of the exact SHA-256 hashes of the files injected by `patches/esp32p4-pre-v3.patch`:

| Kit Path (`esp-rom-sys/ld/esp32p4-eco0_4/...`) | ESP-IDF Original Path (`components/esp_rom/esp32p4/ld/...`) | ESP-IDF Commit | SHA-256 Checksum |
|---|---|---|---|
| `rom/esp32p4.rom.api.ld` | `esp32p4.rom.api.ld` | [`f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0`](https://github.com/espressif/esp-idf/blob/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.api.ld) | `8d2762e1ba52e632aa25233d0bf949c618e1ae890ac069fa7b37508aa4c084f5` |
| `rom/esp32p4.rom.ld` | `esp32p4.rom.eco0_4.ld` | [`f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0`](https://github.com/espressif/esp-idf/blob/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.ld) | `d981e64acca46cfa7a1ece367c29a6f00dd1d2d6b90a5c4b7a7cb2df3321395b` |
| `rom/esp32p4.rom.libgcc.ld` | `esp32p4.rom.eco0_4.libgcc.ld` | [`f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0`](https://github.com/espressif/esp-idf/blob/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.libgcc.ld) | `ffec740b0ac248cf46c65126aa3151aca01a0bf250b437609f3886cf496d8c9c` |
| `rom/esp32p4.rom.rvfp.ld` | `esp32p4.rom.eco0_4.rvfp.ld` | [`f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0`](https://github.com/espressif/esp-idf/blob/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.rvfp.ld) | `bb409833f995c8e3c40e9fbb736a4da340646441a36121f7d326b553c2483ba9` |
| `rom/esp32p4.rom.version.ld` | `esp32p4.rom.version.ld` | [`34614494943436c0b07a657dc411078f89d3c13f`](https://github.com/espressif/esp-idf/blob/34614494943436c0b07a657dc411078f89d3c13f/components/esp_rom/esp32p4/ld/esp32p4.rom.version.ld) | `efb581114e64cbab520da259a3a70e8ab841add52899c89149bbbe77c05c0e18` |

### Reproducible Provenance Verification

You can independently verify these hashes directly from the upstream repository:

```sh
curl -s https://raw.githubusercontent.com/espressif/esp-idf/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.api.ld | shasum -a 256
curl -s https://raw.githubusercontent.com/espressif/esp-idf/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.ld | shasum -a 256
curl -s https://raw.githubusercontent.com/espressif/esp-idf/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.libgcc.ld | shasum -a 256
curl -s https://raw.githubusercontent.com/espressif/esp-idf/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.rvfp.ld | shasum -a 256
curl -s https://raw.githubusercontent.com/espressif/esp-idf/34614494943436c0b07a657dc411078f89d3c13f/components/esp_rom/esp32p4/ld/esp32p4.rom.version.ld | shasum -a 256
```

## ROM Tables (Local Adaptation)

The `rom/additional.ld` file is not a direct copy from ESP-IDF. It is a local adaptation selectively picking symbols required by `esp-hal` without pulling in larger incompatible or redundant definitions. Its origin is ESP-IDF commit `f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0`.

Exact symbol mapping and origin checksums from ESP-IDF:
- `syscall_table_ptr`, `memset`: Derived from [`esp32p4.rom.eco0_4.libc.ld`](https://github.com/espressif/esp-idf/blob/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.libc.ld)
  - Source SHA-256: `ff16b52fc6f588300abe031450e43d2b9b097bed8693e5b7d3f958fd5b71bda6`
- `memcpy`, `strcpy`, `memmove`, `memcmp`, `strncpy`, `strcmp`, `strncmp`: Derived from [`esp32p4.rom.libc-suboptimal_for_misaligned_mem.ld`](https://github.com/espressif/esp-idf/blob/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.libc-suboptimal_for_misaligned_mem.ld)
  - Source SHA-256: `0f5ba8057b53c369fac428b57ba68749f1999a83154c25a5166970942b200fff`
- `ets_update_cpu_frequency`, `ets_printf`, `ets_delay_us`: Derived from [`esp32p4.rom.eco0_4.ld`](https://github.com/espressif/esp-idf/blob/f68bc1ba9fef2b2a80e43e3a581ad314ac9226f0/components/esp_rom/esp32p4/ld/esp32p4.rom.eco0_4.ld)
  - Source SHA-256: `d981e64acca46cfa7a1ece367c29a6f00dd1d2d6b90a5c4b7a7cb2df3321395b`

Its SHA-256 checksum in this patch is `7ce2458b1af983ef5c7db4eb122e896447e6833572bd5789a994bdca19fb5be6`.

## Local Structural Files

The files `rom-functions.x` and `libesp_rom_sys.a` inside the `esp-rom-sys` component of the patch are standard boilerplate structural files matching the convention used across all chips in the `esp-rs/esp-rom-sys` repository. For every supported chip architecture (e.g., `esp32`, `esp32s3`), `libesp_rom_sys.a` solely contains `INCLUDE "rom-functions.x"`, and `rom-functions.x` lists the `INCLUDE` directives for the actual ROM `.ld` files. They do not originate from ESP-IDF; they are authored within `esp-rs` under its dual MIT / Apache-2.0 license.

## Release Readiness

Future formal releases may feature an immutable Git tag and a strict patch digest pin to guarantee reproducibility. This document makes no promise that a current formal release exists or that it has passed all hardware validation (HIL) checks. See the `README.md` for the explicit hardware limits.
