#![no_std]
#![no_main]

use panic_halt as _;
use esp_hal::{clock::CpuClock, delay::Delay, main};

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let _peripherals = esp_hal::init(config);
    let delay = Delay::new();
    loop {
        delay.delay_millis(500);
    }
}
