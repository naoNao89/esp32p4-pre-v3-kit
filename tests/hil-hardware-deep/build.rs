use std::{env, fs, path::PathBuf};

const SYMBOLS: [(&str, &str); 10] = [
    ("ets_delay_us", "ROM_ETS_DELAY_US"),
    ("crc32_le", "ROM_CRC32_LE"),
    ("crc32_be", "ROM_CRC32_BE"),
    ("MD5Init", "ROM_MD5INIT"),
    ("MD5Update", "ROM_MD5UPDATE"),
    ("MD5Final", "ROM_MD5FINAL"),
    ("memset", "ROM_MEMSET"),
    ("memcpy", "ROM_MEMCPY"),
    ("memmove", "ROM_MEMMOVE"),
    ("memcmp", "ROM_MEMCMP"),
];

fn main() {
    println!("cargo:rerun-if-env-changed=ESP_HAL_CONFIG_MIN_CHIP_REVISION");
    assert_eq!(
        env::var("ESP_HAL_CONFIG_MIN_CHIP_REVISION").as_deref(),
        Ok("103"),
        "deep firmware requires the exact rev103 linker profile"
    );
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let mut addresses = [None; SYMBOLS.len()];
    for relative in [
        "../../target/hil-hardware-deep/upstream/esp-rom-sys/ld/esp32p4-eco0_4/rom/esp32p4.rom.ld",
        "../../target/hil-hardware-deep/upstream/esp-rom-sys/ld/esp32p4-eco0_4/rom/additional.ld",
    ] {
        let path = root.join(relative);
        println!("cargo:rerun-if-changed={}", path.display());
        let table = fs::read_to_string(&path).expect("read pinned eco0_4 ROM table");
        for line in table.lines() {
            let line = line.trim();
            let (body, suffix) = match line.strip_prefix("PROVIDE") {
                Some(rest) => (
                    rest.trim_start().strip_prefix('(').expect("PROVIDE syntax"),
                    ");",
                ),
                None => (line, ";"),
            };
            let Some((symbol, value)) = body.split_once('=') else {
                continue;
            };
            let Some(index) = SYMBOLS.iter().position(|(name, _)| *name == symbol.trim()) else {
                continue;
            };
            let value = value
                .trim()
                .strip_suffix(suffix)
                .expect("ROM assignment terminator")
                .trim();
            let hex = value
                .strip_prefix("0x")
                .expect("ROM address must be hexadecimal literal");
            assert!(
                !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "malformed ROM address for {}",
                symbol
            );
            let address = u32::from_str_radix(hex, 16).expect("ROM address width");
            assert!(
                (0x4fc0_0000..0x4fc2_0000).contains(&address) && address % 4 == 0,
                "invalid ROM function address for {}",
                symbol
            );
            if let Some(previous) = addresses[index] {
                assert_eq!(
                    previous, address,
                    "conflicting ROM definitions for {}",
                    symbol
                );
            }
            addresses[index] = Some(address);
        }
    }
    let mut generated = String::new();
    for ((symbol, constant), address) in SYMBOLS.iter().zip(addresses) {
        let address = address.unwrap_or_else(|| panic!("missing pinned ROM symbol {symbol}"));
        generated.push_str(&format!("pub const {constant}: usize = 0x{address:08x};\n"));
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("rom_addresses.rs");
    fs::write(output, generated).expect("write ROM address constants");
}
