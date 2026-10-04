#![cfg(test)]

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn run_cmd(cmd: &mut Command, desc: &str) -> String {
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("Failed to run {}: {}", desc, e));
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "{} failed with {}\nSTDOUT:\n{}\nSTDERR:\n{}",
            desc, output.status, stdout, stderr
        );
    }

    String::from_utf8(output.stdout).unwrap()
}
fn check_consumer(revision: &str) {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let dest = temp_dir.path().join("dest");

    let prepare_bin = env!("CARGO_BIN_EXE_cargo-esp32p4-pre-v3");

    // 1. Prepare
    let mut cmd = Command::new(prepare_bin);
    cmd.arg("prepare").arg(&dest);
    run_cmd(&mut cmd, "cargo-esp32p4-pre-v3 prepare");

    // 2. Create consumer crate
    let consumer_dir = dest.join("consumer");
    let mut cmd = Command::new("cargo");
    cmd.arg("new").arg("consumer").current_dir(&dest);
    run_cmd(&mut cmd, "cargo new consumer");

    // 3. Write Cargo.toml
    let upstream_dir = dest.join("upstream");
    let patch_path = dest.join("Cargo.patch.toml");
    let patch_content = fs::read_to_string(&patch_path).unwrap();

    let sync_path = serde_json::to_string(upstream_dir.join("esp-sync").to_str().unwrap()).unwrap();
    let rom_path =
        serde_json::to_string(upstream_dir.join("esp-rom-sys").to_str().unwrap()).unwrap();

    let mut cargo_toml = format!(
        r#"[package]
name = "consumer"
version = "0.1.0"
edition = "2021"

[workspace]

[dependencies]
esp-hal = {{ version = "=1.1.0", features = ["esp32p4"] }}
esp-sync = {{ path = {}, features = ["esp32p4"] }}
esp-rom-sys = {{ path = {}, features = ["esp32p4"] }}
"#,
        sync_path, rom_path
    );
    cargo_toml.push('\n');
    cargo_toml.push_str(&patch_content);
    cargo_toml.push_str(&format!("\n\"esp-rom-sys\" = {{ path = {} }}\n", rom_path));

    fs::write(consumer_dir.join("Cargo.toml"), cargo_toml).unwrap();

    // 4. Write src/main.rs
    let main_rs = r#"#![no_std]
#![no_main]

extern crate esp_rom_sys;
use esp_sync::raw::{RawLock, SingleCoreInterruptLock};

#[inline(never)]
#[no_mangle]
pub fn test_lock() {
    let lock = SingleCoreInterruptLock;
    unsafe {
        let state = lock.enter();
        core::hint::black_box(&state);
        lock.exit(state);
    }
}

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    loop {
        test_lock();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
"#;
    fs::write(consumer_dir.join("src").join("main.rs"), main_rs).unwrap();

    // 5. Build
    let target_dir = consumer_dir.join("target").join(format!("rev{}", revision));
    let mut cmd = Command::new("cargo");
    cmd.arg("rustc")
        .arg("--release")
        .arg("--target")
        .arg("riscv32imafc-unknown-none-elf")
        .arg("--")
        .arg("-C")
        .arg("link-arg=-Tlinkall.x")
        .current_dir(&consumer_dir);

    // Strip env vars
    for (key, _) in env::vars() {
        if key.starts_with("ESP_SYNC_CONFIG")
            || key == "RUSTFLAGS"
            || key == "CARGO_ENCODED_RUSTFLAGS"
        {
            cmd.env_remove(key);
        }
    }
    cmd.env("ESP_HAL_CONFIG_MIN_CHIP_REVISION", revision);
    cmd.env("CARGO_TARGET_DIR", &target_dir);

    run_cmd(&mut cmd, "cargo rustc");

    // 6. Find llvm-objdump
    let obj_file = target_dir
        .join("riscv32imafc-unknown-none-elf")
        .join("release")
        .join("consumer");
    assert!(obj_file.exists(), "Could not find compiled binary");

    let mut sysroot_cmd = Command::new("rustc");
    sysroot_cmd.arg("--print").arg("sysroot");
    let sysroot = run_cmd(&mut sysroot_cmd, "rustc --print sysroot")
        .trim()
        .to_string();

    let mut host_cmd = Command::new("rustc");
    host_cmd.arg("-vV");
    let host_out = run_cmd(&mut host_cmd, "rustc -vV");
    let host = host_out
        .lines()
        .find(|l| l.starts_with("host:"))
        .unwrap()
        .split_once(':')
        .unwrap()
        .1
        .trim();

    let objdump = Path::new(&sysroot)
        .join("lib")
        .join("rustlib")
        .join(host)
        .join("bin")
        .join(format!("llvm-objdump{}", env::consts::EXE_SUFFIX));

    assert!(
        objdump.exists(),
        "llvm-objdump not found at {}. Install it via `rustup component add llvm-tools`",
        objdump.display()
    );

    // 7. Disassemble test_lock
    let mut cmd = Command::new(&objdump);
    cmd.arg("-d")
        .arg("--disassemble-symbols=test_lock")
        .arg(&obj_file);

    let out = run_cmd(&mut cmd, "objdump");

    // 8. Parse output
    let mut has_347 = false;
    let mut has_enter = false;
    let mut has_restore = false;

    let asm_line_re =
        regex::Regex::new(r"^\s*[0-9a-fA-F]+:\s+(?:[0-9a-fA-F]{2}\s*)+\s+(\S+)\s+(.*)$").unwrap();
    let csr_re = regex::Regex::new(r"\b(0x347|839|mintthresh)\b").unwrap();
    let mstatus_re = regex::Regex::new(r"\b(mstatus|0x300|768)\b").unwrap();

    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(caps) = asm_line_re.captures(line) {
            let mnemonic = &caps[1];
            let operands = &caps[2];

            if mnemonic.contains("csr") && csr_re.is_match(operands) {
                has_347 = true;
            }

            if mstatus_re.is_match(operands) {
                if ["csrrci", "csrrc", "csrci"].contains(&mnemonic) {
                    has_enter = true;
                }
                if ["csrs", "csrsi"].contains(&mnemonic) {
                    has_restore = true;
                }
            }
        }
    }

    assert!(
        has_enter && has_restore,
        "ERROR: Lock path (mstatus read/restore) not found in test_lock. Optimizer might have removed it.\nDisassembly:\n{}",
        out
    );

    if revision == "103" {
        assert!(
            !has_347,
            "FAIL: CSR 347 is present in test_lock for revision 103 (should be absent)\nDisassembly:\n{}",
            out
        );
        println!("PASS: CSR 347 is correctly ABSENT in test_lock for revision 103.");
    } else if revision == "300" {
        assert!(
            has_347,
            "FAIL: CSR 347 is absent in test_lock for revision 300 (should be present)\nDisassembly:\n{}",
            out
        );
        println!("PASS: CSR 347 is correctly PRESENT in test_lock for revision 300.");
    } else {
        panic!("Unknown revision {}", revision);
    }
}

#[test]
#[ignore]
fn revision_103() {
    check_consumer("103");
}

#[test]
#[ignore]
fn revision_300() {
    check_consumer("300");
}
