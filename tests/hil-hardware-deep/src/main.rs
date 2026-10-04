#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::{Config, clock::CpuClock};
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

pub mod rom_tests;
pub mod stress;
pub mod uart_tests;

pub mod rom_addresses {
    include!(concat!(env!("OUT_DIR"), "/rom_addresses.rs"));
}

// Every caller must be core0, outside ISRs, CSs, and raw interrupt masks.
// Request cooperative core1 shutdown; NEVER park it while it might own a CS.
pub fn fail(stage: &str) -> ! {
    stress::request_abort();
    println!("HIL_FAIL: {}", stage);
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(any(
    all(feature = "clock-90", feature = "clock-180"),
    all(feature = "clock-90", feature = "clock-360"),
    all(feature = "clock-180", feature = "clock-360"),
))]
compile_error!("Select exactly one CPU clock; nondefault builds require --no-default-features");
#[cfg(not(any(feature = "clock-90", feature = "clock-180", feature = "clock-360")))]
compile_error!("Select exactly one of clock-90, clock-180, clock-360");

#[cfg(feature = "clock-90")]
const CPU_CLOCK: CpuClock = CpuClock::_90MHz;
#[cfg(feature = "clock-90")]
const CPU_MHZ: u32 = 90;
#[cfg(feature = "clock-180")]
const CPU_CLOCK: CpuClock = CpuClock::_180MHz;
#[cfg(feature = "clock-180")]
const CPU_MHZ: u32 = 180;
#[cfg(feature = "clock-360")]
const CPU_CLOCK: CpuClock = CpuClock::_360MHz;
#[cfg(feature = "clock-360")]
const CPU_MHZ: u32 = 360;

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(Config::default().with_cpu_clock(CPU_CLOCK));
    println!("HIL_BOOT_DEEP rev103 cpu_mhz={}", CPU_MHZ);

    println!(
        "HIL_STAGE: selected ROM/internal SRAM checks before TIMG startup; ROM delay uses local mask/XTAL timing, IRQ masking validated later"
    );
    let rom_count = rom_tests::run();
    if rom_count == 0 {
        fail("no ROM checks completed");
    }
    println!("HIL_PROGRESS: ROM meaningful_checks={}", rom_count);

    let mut runner = stress::start(peripherals.CPU_CTRL, peripherals.TIMG0, peripherals.TIMG1);
    println!(
        "HIL_STAGE: UART0-4 matrix; AppCpu idle progress and TIMG1 IRQ active, no concurrent CS stress"
    );
    let uart_count = uart_tests::run(
        peripherals.UART0,
        peripherals.UART1,
        peripherals.UART2,
        peripherals.UART3,
        peripherals.UART4,
    );
    if uart_count != uart_tests::EXPECTED_MATRIX_VECTORS {
        fail("UART matrix vector count");
    }
    println!("HIL_PROGRESS: UART completed_vectors={}", uart_count);

    runner.run_soak();
    let fixed_counter_value = runner.finish();
    if fixed_counter_value != 1_500_000 {
        fail("stress completion counter value");
    }
    println!(
        "HIL_PROGRESS: all phases complete rom_checks={} uart_vectors={} fixed_counter_value={} soak_duration_s=10",
        rom_count, uart_count, fixed_counter_value
    );
    println!("HIL_PASS_DEEP");
    loop {
        core::hint::spin_loop();
    }
}
