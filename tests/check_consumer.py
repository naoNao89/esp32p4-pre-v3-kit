import argparse
import os
import subprocess
import sys
import re

def run(cmd, cwd=None, env=None):
    print(f"Running: {' '.join(cmd)} in {cwd}")
    res = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    if res.returncode != 0:
        print(f"Command failed with {res.returncode}:\n{res.stdout}\n{res.stderr}")
        sys.exit(1)
    return res.stdout

def main():
    parser = argparse.ArgumentParser(description="Build and check minimal consumer")
    parser.add_argument("--revision", type=str, required=True, choices=["103", "300"])
    parser.add_argument("--destination", type=str, required=True)
    args = parser.parse_args()

    dest = os.path.abspath(args.destination)
    if os.path.exists(dest) or os.path.islink(dest):
        print(f"ERROR: Destination {dest} already exists. Refusing to overwrite.")
        sys.exit(1)

    print(f"Preparing patched upstream at {dest}...")
    run(["cargo", "xtask", "prepare", dest], env=dict(os.environ, PYTHON=sys.executable))

    print("Creating consumer crate...")
    consumer_dir = os.path.join(dest, "consumer")
    run(["cargo", "new", "consumer"], cwd=dest)
    
    upstream_dir = os.path.join(dest, "upstream")

    cargo_toml = f'''[package]
name = "consumer"
version = "0.1.0"
edition = "2021"

[workspace]

[dependencies]
esp-hal = {{ version = "=1.1.0", features = ["esp32p4"] }}
esp-sync = {{ path = "{os.path.join(upstream_dir, "esp-sync")}", features = ["esp32p4"] }}
esp-rom-sys = {{ path = "{os.path.join(upstream_dir, "esp-rom-sys")}", features = ["esp32p4"] }}
'''
    with open(os.path.join(dest, "Cargo.patch.toml"), "r") as patch_f:
        cargo_toml += "\n" + patch_f.read()
    
    # We must patch esp-rom-sys as well, using the identical absolute path
    cargo_toml += f'\n"esp-rom-sys" = {{ path = "{os.path.join(upstream_dir, "esp-rom-sys")}" }}\n'

    with open(os.path.join(consumer_dir, "Cargo.toml"), "w") as f:
        f.write(cargo_toml)

    main_rs = '''#![no_std]
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
'''
    with open(os.path.join(consumer_dir, "src", "main.rs"), "w") as f:
        f.write(main_rs)

    env = os.environ.copy()
    env["ESP_HAL_CONFIG_MIN_CHIP_REVISION"] = args.revision
    
    # Strip conflicting ambient variables
    for key in list(env.keys()):
        if key.startswith("ESP_SYNC_CONFIG") or key in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"]:
            del env[key]

    target_dir = os.path.join(consumer_dir, "target", f"rev{args.revision}")
    env["CARGO_TARGET_DIR"] = target_dir

    target = "riscv32imafc-unknown-none-elf"
    print(f"Building consumer with revision {args.revision}...")
    run(["cargo", "rustc", "--release", "--target", target, "--", "-C", "link-arg=-Tlinkall.x"], cwd=consumer_dir, env=env)

    obj_file = os.path.join(target_dir, target, "release", "consumer")
    if not os.path.exists(obj_file):
        print("Could not find compiled binary.")
        sys.exit(1)
    
    # Try finding llvm-objdump
    sysroot = run(["rustc", "--print", "sysroot"]).strip()
    host_out = run(["rustc", "-vV"])
    host = [line.split(":", 1)[1].strip() for line in host_out.splitlines() if line.startswith("host:")][0]
    
    objdump = os.path.join(sysroot, "lib", "rustlib", host, "bin", "llvm-objdump")
    if not os.path.exists(objdump):
        try:
            subprocess.run(["xcrun", "llvm-objdump", "--version"], check=True, capture_output=True)
            objdump = "xcrun llvm-objdump"
        except Exception:
            pass

    if not objdump.startswith("xcrun") and not os.path.exists(objdump):
        print(f"llvm-objdump not found at {objdump}, using fallback...")
        objdump = "llvm-objdump"

    print("Disassembling test_lock...")
    cmd = []
    if objdump == "xcrun llvm-objdump":
        cmd = ["xcrun", "llvm-objdump", "-d", "--disassemble-symbols=test_lock", obj_file]
    else:
        cmd = [objdump, "-d", "--disassemble-symbols=test_lock", obj_file]
    
    try:
        out = subprocess.check_output(cmd, text=True)
    except Exception as e:
        print(f"Failed to run objdump: {e}")
        sys.exit(1)

    print(out)

    lines = out.splitlines()
    has_347 = False
    has_enter = False
    has_restore = False
    
    # Regex for a disassembly line: e.g., "400026c8: 347295f3     	csrrw	a1, 0x347, t0"
    asm_line_re = re.compile(r"^\s*[0-9a-fA-F]+:\s+(?:[0-9a-fA-F]{2}\s*)+\s+(\S+)\s+(.*)$")
    
    for line in lines:
        line = line.strip()
        if not line:
            continue
        
        m = asm_line_re.match(line)
        if m:
            mnemonic = m.group(1)
            operands = m.group(2)
            
            if "csr" in mnemonic:
                if re.search(r"\b(?:0x347|839|mintthresh)\b", operands):
                    has_347 = True
            
            if "mstatus" in operands or "0x300" in operands or "768" in operands:
                if mnemonic in ["csrrci", "csrrc", "csrci"]:
                    has_enter = True
                if mnemonic in ["csrs", "csrsi"]:
                    has_restore = True

    if not has_enter or not has_restore:
        print("ERROR: Lock path (mstatus read/restore) not found in test_lock. Optimizer might have removed it.")
        sys.exit(1)

    if args.revision == "103":
        if has_347:
            print(f"FAIL: CSR 347 is present in test_lock for revision 103 (should be absent)")
            sys.exit(1)
        else:
            print(f"PASS: CSR 347 is correctly ABSENT in test_lock for revision 103.")
    elif args.revision == "300":
        if not has_347:
            print(f"FAIL: CSR 347 is absent in test_lock for revision 300 (should be present)")
            sys.exit(1)
        else:
            print(f"PASS: CSR 347 is correctly PRESENT in test_lock for revision 300.")

if __name__ == "__main__":
    main()
