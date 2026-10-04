use core::ffi::c_void;
use core::hint::black_box;
use esp_hal::time::Instant;
use esp_println::println;
use esp_sync::raw::{RawLock, SingleCoreInterruptLock};

unsafe extern "C" {
    fn crc32_le(crc: u32, buf: *const u8, len: u32) -> u32;
    fn crc32_be(crc: u32, buf: *const u8, len: u32) -> u32;
    fn MD5Init(context: *mut u8);
    fn MD5Update(context: *mut u8, buf: *const u8, len: u32);
    fn MD5Final(digest: *mut u8, context: *mut u8);
    fn memset(s: *mut c_void, c: i32, n: usize) -> *mut c_void;
    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn memcmp(s1: *const c_void, s2: *const c_void, n: usize) -> i32;
    fn ets_delay_us(us: u32);
}

fn check_addresses() -> u32 {
    let mut checks = 0;
    let addrs = [
        (
            crc32_le as *const () as usize,
            crate::rom_addresses::ROM_CRC32_LE,
            "crc32_le",
        ),
        (
            crc32_be as *const () as usize,
            crate::rom_addresses::ROM_CRC32_BE,
            "crc32_be",
        ),
        (
            MD5Init as *const () as usize,
            crate::rom_addresses::ROM_MD5INIT,
            "MD5Init",
        ),
        (
            MD5Update as *const () as usize,
            crate::rom_addresses::ROM_MD5UPDATE,
            "MD5Update",
        ),
        (
            MD5Final as *const () as usize,
            crate::rom_addresses::ROM_MD5FINAL,
            "MD5Final",
        ),
        (
            memset as *const () as usize,
            crate::rom_addresses::ROM_MEMSET,
            "memset",
        ),
        (
            memcpy as *const () as usize,
            crate::rom_addresses::ROM_MEMCPY,
            "memcpy",
        ),
        (
            memmove as *const () as usize,
            crate::rom_addresses::ROM_MEMMOVE,
            "memmove",
        ),
        (
            memcmp as *const () as usize,
            crate::rom_addresses::ROM_MEMCMP,
            "memcmp",
        ),
        (
            ets_delay_us as *const () as usize,
            crate::rom_addresses::ROM_ETS_DELAY_US,
            "ets_delay_us",
        ),
    ];

    for &(actual, expected, name) in addrs.iter() {
        println!(
            "rom check: {} actual={:#010x} expected={:#010x}",
            name, actual, expected
        );
        if actual != expected {
            println!("HIL_FAIL: {} address mismatch", name);
            crate::fail("rom_address_mismatch");
        }
        checks += 1;
    }
    checks
}

fn test_crc() -> u32 {
    let mut checks = 0;
    let data = b"123456789";
    unsafe {
        let le = crc32_le(0, data.as_ptr(), 9);
        if le != 0xCBF43926 {
            println!("crc32_le actual={:#010x} expected=0xCBF43926", le);
            crate::fail("crc32_le_fail");
        }
        checks += 1;

        let be = !crc32_be(0, data.as_ptr(), 9);
        if be != 0x0376E6E7 {
            println!("!crc32_be actual={:#010x} expected=0x0376E6E7", be);
            crate::fail("crc32_be_fail");
        }
        checks += 1;
    }
    checks
}

fn test_md5() -> u32 {
    let mut checks = 0;

    let empty_md5 = [
        0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8, 0x42,
        0x7e,
    ];
    let digest_empty = esp_hal::rom::md5::compute(b"");
    if digest_empty.0 != empty_md5 {
        println!("md5 empty failed");
        crate::fail("md5_empty_fail");
    }
    checks += 1;

    let abc_md5 = [
        0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0, 0xd6, 0x96, 0x3f, 0x7d, 0x28, 0xe1, 0x7f,
        0x72,
    ];
    let digest_abc = esp_hal::rom::md5::compute(b"abc");
    if digest_abc.0 != abc_md5 {
        println!("md5 abc failed");
        crate::fail("md5_abc_fail");
    }
    checks += 1;

    checks
}

fn test_mem_functions() -> u32 {
    let mut checks = 0;
    const BUF_SIZE: usize = 256;
    static mut D: [u8; BUF_SIZE] = [0; BUF_SIZE];
    static mut S: [u8; BUF_SIZE] = [0; BUF_SIZE];

    unsafe {
        let d_ptr = core::ptr::addr_of_mut!(D) as *mut u8;
        let s_ptr = core::ptr::addr_of_mut!(S) as *mut u8;

        let dyn_memset =
            black_box(memset as unsafe extern "C" fn(*mut c_void, i32, usize) -> *mut c_void);
        let dyn_memcpy = black_box(
            memcpy as unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> *mut c_void,
        );
        let dyn_memmove = black_box(
            memmove as unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> *mut c_void,
        );
        let dyn_memcmp =
            black_box(memcmp as unsafe extern "C" fn(*const c_void, *const c_void, usize) -> i32);

        // memset
        dyn_memset(d_ptr.cast(), 0xAA, BUF_SIZE);
        for i in 0..BUF_SIZE {
            if core::ptr::read_volatile(d_ptr.add(i)) != 0xAA {
                crate::fail("memset_fail");
            }
        }
        checks += 1;

        // memcpy
        for i in 0..BUF_SIZE {
            core::ptr::write_volatile(s_ptr.add(i), (i & 0xFF) as u8);
            core::ptr::write_volatile(d_ptr.add(i), 0);
        }
        dyn_memcpy(d_ptr.cast(), s_ptr.cast(), BUF_SIZE);
        for i in 0..BUF_SIZE {
            if core::ptr::read_volatile(d_ptr.add(i)) != (i & 0xFF) as u8 {
                crate::fail("memcpy_fail");
            }
        }
        checks += 1;

        // memmove forward overlap (dest > src)
        for i in 0..BUF_SIZE {
            core::ptr::write_volatile(d_ptr.add(i), (i & 0xFF) as u8);
        }
        dyn_memmove(d_ptr.add(16).cast(), d_ptr.cast(), 128);
        for i in 0..128 {
            if core::ptr::read_volatile(d_ptr.add(i + 16)) != (i & 0xFF) as u8 {
                crate::fail("memmove_forward_fail");
            }
        }
        checks += 1;

        // memmove backward overlap (dest < src)
        for i in 0..BUF_SIZE {
            core::ptr::write_volatile(d_ptr.add(i), (i & 0xFF) as u8);
        }
        dyn_memmove(d_ptr.cast(), d_ptr.add(16).cast(), 128);
        for i in 0..128 {
            if core::ptr::read_volatile(d_ptr.add(i)) != ((i + 16) & 0xFF) as u8 {
                crate::fail("memmove_backward_fail");
            }
        }
        checks += 1;

        // memcmp
        for i in 0..BUF_SIZE {
            core::ptr::write_volatile(d_ptr.add(i), (i & 0xFF) as u8);
            core::ptr::write_volatile(s_ptr.add(i), (i & 0xFF) as u8);
        }
        if dyn_memcmp(d_ptr.cast(), s_ptr.cast(), BUF_SIZE) != 0 {
            crate::fail("memcmp_eq_fail");
        }
        checks += 1;

        core::ptr::write_volatile(s_ptr.add(128), 0x00);
        if dyn_memcmp(d_ptr.cast(), s_ptr.cast(), BUF_SIZE) <= 0 {
            crate::fail("memcmp_gt_fail");
        }
        checks += 1;

        core::ptr::write_volatile(d_ptr.add(128), 0x00);
        core::ptr::write_volatile(s_ptr.add(128), 0xFF);
        if dyn_memcmp(d_ptr.cast(), s_ptr.cast(), BUF_SIZE) >= 0 {
            crate::fail("memcmp_lt_fail");
        }
        checks += 1;
    }
    checks
}
fn test_delay() -> u32 {
    let mut checks = 0;

    let delays = [100, 1000, 10000];

    for &d in &delays {
        let elapsed = unsafe {
            let lock = SingleCoreInterruptLock;
            let token = lock.enter();
            let start = Instant::now();
            ets_delay_us(d);
            let e = start.elapsed().as_micros() as u32;
            lock.exit(token);
            e
        };

        println!("ets_delay_us({}) took {} us", d, elapsed);

        let min_expected = (d * 8) / 10;
        let max_expected = (d * 12) / 10;

        if elapsed < min_expected || elapsed > max_expected {
            println!("ets_delay_us({}) out of bounds", d);
            crate::fail("ets_delay_us_fail");
        }
        checks += 1;
    }

    checks
}

static mut SRAM_BUFFER: [u32; 8192] = [0; 8192];

fn test_sram() -> u32 {
    let mut checks = 0;
    unsafe {
        let len = 8192; // Use constant instead of SRAM_BUFFER.len()
        let sram_ptr = core::ptr::addr_of_mut!(SRAM_BUFFER) as *mut u32;

        // Data pattern
        for i in 0..len {
            core::ptr::write_volatile(sram_ptr.add(i), i as u32 ^ 0xDEADBEEF);
        }
        for i in 0..len {
            if core::ptr::read_volatile(sram_ptr.add(i)) != (i as u32 ^ 0xDEADBEEF) {
                crate::fail("sram_pattern_fail");
            }
            checks += 1; // individual check
        }

        // Walking ones
        for bit in 0..32 {
            let pat = 1u32 << bit;
            for i in 0..len {
                core::ptr::write_volatile(sram_ptr.add(i), pat);
            }
            for i in 0..len {
                if core::ptr::read_volatile(sram_ptr.add(i)) != pat {
                    crate::fail("sram_walk_ones_fail");
                }
                checks += 1; // individual check
            }
        }

        // Walking zeros
        for bit in 0..32 {
            let pat = !(1u32 << bit);
            for i in 0..len {
                core::ptr::write_volatile(sram_ptr.add(i), pat);
            }
            for i in 0..len {
                if core::ptr::read_volatile(sram_ptr.add(i)) != pat {
                    crate::fail("sram_walk_zeros_fail");
                }
                checks += 1; // individual check
            }
        }
    }
    checks
}

pub fn run() -> u32 {
    let mut total_checks = 0;
    total_checks += check_addresses();
    total_checks += test_crc();
    total_checks += test_md5();
    total_checks += test_mem_functions();
    total_checks += test_delay();
    total_checks += test_sram();
    total_checks
}
