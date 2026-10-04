use core::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};
use esp_hal::{
    Blocking,
    peripherals::{HP_SYS_CLKRST, UART0, UART1, UART2, UART3, UART4},
    ram,
    time::Instant,
    uart::{ClockSource, Config, ConfigError, Instance, RxConfig, TxConfig, Uart, UartInterrupt},
};
use esp_println::println;

pub const EXPECTED_MATRIX_VECTORS: u32 = 360;

#[derive(Default)]
struct Coverage {
    matrix_vectors: u32,
    timing_vectors: u32,
    negative_configs: u32,
    recovery_vectors: u32,
}

// Every reference is the PAC singleton's shared, volatile register interface;
// no duplicate HAL peripheral token is manufactured.
macro_rules! regs {
    ($id:expr) => {
        match $id {
            0 => UART0::regs(),
            1 => UART1::regs(),
            2 => UART2::regs(),
            3 => UART3::regs(),
            4 => UART4::regs(),
            _ => unreachable!(),
        }
    };
}

#[derive(Clone, Copy)]
struct Case {
    id: usize,
    clock: ClockSource,
    baud: u32,
    len: usize,
}

impl Case {
    fn stage(self, stage: &str) {
        println!(
            "HIL_STAGE uart={} clock={} baud={} bytes={} stage={}",
            self.id,
            clock_name(self.clock),
            self.baud,
            self.len,
            stage
        );
    }

    fn fail(self, stage: &str) -> ! {
        self.stage(stage);
        crate::fail(stage)
    }
}

fn clock_name(clock: ClockSource) -> &'static str {
    match clock {
        ClockSource::Xtal => "Xtal",
        ClockSource::PllF80m => "PllF80m",
        ClockSource::RcFast => "RcFast",
    }
}

fn source_model(clock: ClockSource) -> (u64, u8) {
    match clock {
        ClockSource::Xtal => (40_000_000, 0),
        ClockSource::PllF80m => (80_000_000, 2),
        ClockSource::RcFast => (20_000_000, 1),
    }
}

struct Clocks {
    words: [u32; 6],
    selectors: [u8; 5],
    dividers: [u8; 5],
    numerators: [u8; 5],
    denominators: [u8; 5],
}

fn clocks() -> Clocks {
    let r = HP_SYS_CLKRST::regs();
    let a = r.peri_clk_ctrl110().read();
    let b = r.peri_clk_ctrl111().read();
    let c = r.peri_clk_ctrl112().read();
    let d = r.peri_clk_ctrl113().read();
    let e = r.peri_clk_ctrl114().read();
    let f = r.peri_clk_ctrl115().read();
    Clocks {
        words: [a.bits(), b.bits(), c.bits(), d.bits(), e.bits(), f.bits()],
        selectors: [
            a.uart0_clk_src_sel().bits(),
            b.uart1_clk_src_sel().bits(),
            c.uart2_clk_src_sel().bits(),
            d.uart3_clk_src_sel().bits(),
            e.uart4_clk_src_sel().bits(),
        ],
        dividers: [
            b.uart0_sclk_div_num().bits(),
            c.uart1_sclk_div_num().bits(),
            d.uart2_sclk_div_num().bits(),
            e.uart3_sclk_div_num().bits(),
            f.uart4_sclk_div_num().bits(),
        ],
        numerators: [
            b.uart0_sclk_div_numerator().bits(),
            c.uart1_sclk_div_numerator().bits(),
            d.uart2_sclk_div_numerator().bits(),
            e.uart3_sclk_div_numerator().bits(),
            f.uart4_sclk_div_numerator().bits(),
        ],
        denominators: [
            b.uart0_sclk_div_denominator().bits(),
            c.uart1_sclk_div_denominator().bits(),
            d.uart2_sclk_div_denominator().bits(),
            e.uart3_sclk_div_denominator().bits(),
            f.uart4_sclk_div_denominator().bits(),
        ],
    }
}

// Independent nominal-source calculation: smallest integer predivider whose
// baud divisor fits the 12-bit integral field, then truncate the 4-bit fraction.
fn expected_dividers(case: Case) -> (u64, u64) {
    let (hz, _) = source_model(case.clock);
    let mut predivider = 1;
    while hz > 4095 * u64::from(case.baud) * predivider {
        predivider += 1;
    }
    (predivider, hz * 16 / (u64::from(case.baud) * predivider))
}

fn check_clocks(case: Case, before: Clocks, creating: bool) {
    let after = clocks();
    let (predivider, divisor16) = expected_dividers(case);
    let (_, selector) = source_model(case.clock);
    let baud = regs!(case.id).clkdiv().read();
    if after.selectors[case.id] != selector
        || u64::from(after.dividers[case.id]) + 1 != predivider
        || u64::from(baud.clkdiv().bits()) != divisor16 / 16
        || u64::from(baud.clkdiv_frag().bits()) != divisor16 % 16
        || after.numerators[case.id] != 0
        || after.denominators[case.id] != 0
    {
        println!(
            "HIL_UART_CLOCK uart={} mux={} sdiv={} int={} frac={} expected_mux={} expected_sdiv={} expected_div16={}",
            case.id,
            after.selectors[case.id],
            after.dividers[case.id],
            baud.clkdiv().bits(),
            baud.clkdiv_frag().bits(),
            selector,
            predivider - 1,
            divisor16
        );
        case.fail("clock/divider readback");
    }
    for n in 0..5 {
        if n != case.id
            && (before.selectors[n] != after.selectors[n]
                || before.dividers[n] != after.dividers[n])
        {
            println!("HIL_UART_NEIGHBOR owner={} changed_uart={}", case.id, n);
            case.fail("inactive UART clock changed");
        }
    }
    for n in 0..6 {
        // PAC CTRL110+n: source bits24:25; CTRL111+n: divider bits0:7.
        // Only initial creation may also turn on the tested UART's clock gate.
        let mut allowed = 0;
        if n == case.id {
            allowed |= 3 << 24;
            if creating {
                allowed |= 1 << 26;
            }
        }
        if n == case.id + 1 {
            allowed |= 0xff;
        }
        if (before.words[n] ^ after.words[n]) & !allowed != 0 {
            println!(
                "HIL_UART_NEIGHBOR uart={} ctrl={} before={:08x} after={:08x}",
                case.id,
                110 + n,
                before.words[n],
                after.words[n]
            );
            case.fail("clock neighbor bits changed");
        }
    }
    println!(
        "HIL_UART_CLOCK_READBACK uart={} clock={} baud={} mux={} sdiv={} int={} frac={} neighbors_preserved=true",
        case.id,
        clock_name(case.clock),
        case.baud,
        after.selectors[case.id],
        after.dividers[case.id],
        baud.clkdiv().bits(),
        baud.clkdiv_frag().bits()
    );
}

fn sync_loopback(case: Case, enabled: bool) {
    let r = regs!(case.id);
    r.conf0().modify(|_, w| w.loopback().bit(enabled));
    r.reg_update().modify(|_, w| w.reg_update().set_bit());
    let start = Instant::now();
    while r.reg_update().read().reg_update().bit_is_set() {
        if start.elapsed().as_micros() > 100_000 {
            case.fail("loopback REG_UPDATE timeout");
        }
    }
    if r.conf0().read().loopback().bit() != enabled {
        case.fail("loopback readback");
    }
}

fn mask_irq(id: usize) {
    regs!(id).int_ena().write(|w| unsafe { w.bits(0) });
}

fn clear_raw(id: usize) {
    // INT_CLR has write-one-to-clear bits0..19, all documented by this PAC.
    regs!(id)
        .int_clr()
        .write(|w| unsafe { w.bits(0x000f_ffff) });
}

fn config(case: Case) -> Config {
    Config::default()
        .with_clock_source(case.clock)
        .with_baudrate(case.baud)
}

fn apply(uart: &mut Uart<'static, Blocking>, case: Case, cfg: &Config) {
    mask_irq(case.id);
    let before = clocks();
    case.stage("apply_config begin"); // HAL's internal REG_UPDATE waits are unbounded.
    if let Err(error) = uart.apply_config(cfg) {
        println!("HIL_UART_CONFIG error={:?}", error);
        case.fail("apply_config error");
    }
    check_clocks(case, before, false);
    sync_loopback(case, true);
    clear_raw(case.id);
}

fn pattern(pos: usize) -> u8 {
    // Consecutive 256 bytes cover every byte, including NUL and 0xff.
    ((pos * 73 + 19) & 255) as u8
}

fn nominal_us(len: usize, baud: u32) -> u64 {
    (len as u64 * 10_000_000).div_ceil(u64::from(baud))
}

fn polling_vector(uart: &mut Uart<'static, Blocking>, case: Case) -> u64 {
    case.stage("polling vector begin");
    if regs!(case.id).int_ena().read().bits() != 0 {
        case.fail("polling interrupts enabled");
    }
    sync_loopback(case, true);
    let mut received = [0u8; 32];
    match uart.read_buffered(&mut received) {
        Ok(0) => {}
        Ok(_) => case.fail("unexpected bytes before vector"),
        Err(_) => case.fail("RX error before vector"),
    }
    clear_raw(case.id);
    let limit = nominal_us(case.len, case.baud) * 3 + 100_000;
    let start = Instant::now();
    let mut tx = 0;
    let mut rx = 0;
    let mut done_us = None;
    loop {
        if start.elapsed().as_micros() > limit {
            println!(
                "HIL_UART_TIMEOUT sent={} received={} expected={}",
                tx, rx, case.len
            );
            case.fail("polling vector deadline");
        }
        // Read before replenishing TX, so continuous traffic never starves RX.
        match uart.read_buffered(&mut received) {
            Ok(count) => {
                for &byte in &received[..count] {
                    if rx >= case.len || byte != pattern(rx) {
                        println!(
                            "HIL_UART_BYTE offset={} actual={} expected={}",
                            rx,
                            byte,
                            pattern(rx)
                        );
                        case.fail("polling exact byte mismatch");
                    }
                    rx += 1;
                }
            }
            Err(error) => {
                println!("HIL_UART_RX error={:?}", error);
                case.fail("polling RX error");
            }
        }
        if tx < case.len && uart.write_ready() {
            let count = (case.len - tx).min(32);
            let mut chunk = [0u8; 32];
            for (i, byte) in chunk[..count].iter_mut().enumerate() {
                *byte = pattern(tx + i);
            }
            // Clear any intermediate idle event BEFORE writing another chunk.
            regs!(case.id)
                .int_clr()
                .write(|w| w.tx_done().clear_bit_by_one());
            match uart.write(&chunk[..count]) {
                Ok(n) if n != 0 && n <= count => tx += n,
                _ => case.fail("polling TX error or no progress"),
            }
        }
        if tx == case.len
            && done_us.is_none()
            && regs!(case.id).int_raw().read().tx_done().bit_is_set()
        {
            done_us = Some(start.elapsed().as_micros());
        }
        if rx == case.len && done_us.is_some() {
            break;
        }
    }
    if let Err(error) = uart.check_for_rx_errors() {
        println!("HIL_UART_RX error={:?}", error);
        case.fail("RX error after vector");
    }
    if regs!(case.id).status().read().rxfifo_cnt().bits() != 0 {
        case.fail("extra RX bytes");
    }
    regs!(case.id)
        .int_clr()
        .write(|w| w.tx_done().clear_bit_by_one());
    if regs!(case.id).int_raw().read().tx_done().bit_is_set() {
        case.fail("TxDone clear readback");
    }
    done_us.unwrap()
}

fn timing(uart: &mut Uart<'static, Blocking>, mut case: Case) {
    case.len = 64;
    let short = polling_vector(uart, case);
    case.len = 1024;
    let long = polling_vector(uart, case);
    if long <= short {
        case.fail("nonpositive timing slope");
    }
    let slope = long - short;
    let (hz, _) = source_model(case.clock);
    let (predivider, divisor16) = expected_dividers(case);
    let expected = (960 * 10_000_000u64 * predivider * divisor16).div_ceil(hz * 16);
    let measured_baud = 960 * 10_000_000u64 / slope;
    let ratio_ppm = expected * 1_000_000 / slope;
    println!(
        "HIL_UART_TIMING uart={} clock={} baud={} short64_us={} long1024_us={} slope_us={} expected_us={} measured_baud={} nominal_source_ratio_ppm={}",
        case.id,
        clock_name(case.clock),
        case.baud,
        short,
        long,
        slope,
        expected,
        measured_baud,
        ratio_ppm
    );
    if case.clock != ClockSource::RcFast {
        let allowance = expected / 10 + 500;
        if slope.abs_diff(expected) > allowance {
            case.fail("two-sided XTAL timing slope");
        }
    } else {
        println!(
            "HIL_UART_RC_SCOPE uart={} baud={} RC-vs-XTAL ratio only; no absolute RC frequency certification",
            case.id, case.baud
        );
    }
}

fn negatives(uart: &mut Uart<'static, Blocking>, case: Case, coverage: &mut Coverage) {
    let valid = config(case);
    let tests = [
        (
            valid.with_baudrate(0),
            ConfigError::BaudrateNotSupported,
            "baud zero",
            0,
        ),
        (
            valid.with_baudrate(5_000_001),
            ConfigError::BaudrateNotSupported,
            "baud over 5M",
            5_000_001,
        ),
        (
            valid.with_rx(RxConfig::default().with_fifo_full_threshold(0)),
            ConfigError::RxFifoThresholdNotSupported,
            "RX threshold zero",
            case.baud,
        ),
        (
            valid.with_tx(TxConfig::default().with_fifo_empty_threshold(128)),
            ConfigError::TxFifoThresholdNotSupported,
            "TX threshold 128",
            case.baud,
        ),
        (
            valid.with_rx(RxConfig::default().with_timeout(103)),
            ConfigError::TimeoutTooLong,
            "RX timeout 103 symbols",
            case.baud,
        ),
    ];
    for (invalid, expected, label, invalid_baud) in tests {
        let negative = Case {
            baud: invalid_baud,
            ..case
        };
        mask_irq(case.id);
        let before = clocks();
        negative.stage(label);
        let result = uart.apply_config(&invalid);
        if result != Err(expected) {
            println!(
                "HIL_UART_NEGATIVE label={} result={:?} expected={:?}",
                label, result, expected
            );
            negative.fail("incorrect negative ConfigError");
        }
        println!(
            "HIL_UART_NEGATIVE uart={} clock={} baud={} label={} observed_error={:?}",
            case.id,
            clock_name(case.clock),
            invalid_baud,
            label,
            expected
        );
        // Invalid baud is rejected before division; other errors may partially
        // configure/reset the driver. Always restore, never rely on rollback.
        check_clocks(case, before, false);
        coverage.negative_configs += 1;
        apply(uart, case, &valid);
        polling_vector(uart, Case { len: 129, ..case });
        coverage.recovery_vectors += 1;
    }
}

fn matrix(id: usize, token: impl Instance + 'static) -> (Uart<'static, Blocking>, Coverage) {
    let initial = Case {
        id,
        clock: ClockSource::Xtal,
        baud: 115_200,
        len: 0,
    };
    mask_irq(id);
    let before = clocks();
    initial.stage("Uart::new begin");
    let mut uart = match Uart::new(token, config(initial)) {
        Ok(uart) => uart,
        Err(error) => {
            println!("HIL_UART_NEW error={:?}", error);
            initial.fail("Uart::new error");
        }
    };
    mask_irq(id);
    check_clocks(initial, before, true);
    sync_loopback(initial, true);
    let mut coverage = Coverage::default();
    for clock in [ClockSource::Xtal, ClockSource::PllF80m, ClockSource::RcFast] {
        for baud in [9600, 115_200, 1_000_000, 2_000_000] {
            let case = Case {
                id,
                clock,
                baud,
                len: 0,
            };
            apply(&mut uart, case, &config(case));
            for len in [1, 127, 128, 129, 513, 1024] {
                polling_vector(&mut uart, Case { len, ..case });
                coverage.matrix_vectors += 1;
            }
            timing(&mut uart, case);
            coverage.timing_vectors += 2;
            println!(
                "HIL_UART_CONFIG_COMPLETE uart={} clock={} baud={} matrix_vectors=6 timing_vectors=2",
                id,
                clock_name(clock),
                baud
            );
        }
    }
    if coverage.matrix_vectors != 72 || coverage.timing_vectors != 24 {
        initial.fail("per-UART matrix/timing coverage count");
    }
    // Restore a known source/baud before negative checks.
    apply(&mut uart, initial, &config(initial));
    negatives(&mut uart, initial, &mut coverage);
    println!(
        "HIL_UART_MATRIX_COMPLETE uart={} vectors={} timing_vectors={} negative_configs={} recovery_vectors={}",
        id,
        coverage.matrix_vectors,
        coverage.timing_vectors,
        coverage.negative_configs,
        coverage.recovery_vectors
    );
    (uart, coverage)
}

const FULL: u32 = 1;
const DONE: u32 = 2;
const TIMEOUT: u32 = 4;
static IRQ_FLAGS: AtomicU32 = AtomicU32::new(0);
static IRQ_FULL_COUNT: AtomicU32 = AtomicU32::new(0);
static IRQ_DONE_COUNT: AtomicU32 = AtomicU32::new(0);
static IRQ_TIMEOUT_COUNT: AtomicU32 = AtomicU32::new(0);
static IRQ_ERROR: AtomicU32 = AtomicU32::new(0);
static IRQ_RX_POS: AtomicUsize = AtomicUsize::new(0);
static IRQ_RX: [AtomicU8; 1024] = [const { AtomicU8::new(0) }; 1024];

#[esp_hal::handler]
#[ram]
fn uart1_handler() {
    let r = UART1::regs();
    let pending = r.int_st().read();
    let raw = r.int_raw().read();
    if raw.rxfifo_ovf().bit_is_set()
        || raw.glitch_det().bit_is_set()
        || raw.frm_err().bit_is_set()
        || raw.parity_err().bit_is_set()
    {
        IRQ_ERROR.fetch_or(1, Ordering::Relaxed);
    }
    let mut flags = 0;
    if pending.rxfifo_full().bit_is_set() {
        flags |= FULL;
        IRQ_FULL_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    if pending.rxfifo_tout().bit_is_set() {
        flags |= TIMEOUT;
        IRQ_TIMEOUT_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    if pending.tx_done().bit_is_set() {
        flags |= DONE;
        IRQ_DONE_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    if flags & (FULL | TIMEOUT) != 0 {
        // Fixed FIFO-sized bound, never clear Full without first draining.
        // Only this handler consumes RX while enabled; main only writes TX.
        let count = usize::from(r.status().read().rxfifo_cnt().bits()).min(128);
        let mut pos = IRQ_RX_POS.load(Ordering::Relaxed);
        for _ in 0..count {
            let byte = r.fifo().read().rxfifo_rd_byte().bits();
            if pos < IRQ_RX.len() {
                IRQ_RX[pos].store(byte, Ordering::Relaxed);
                pos += 1;
            } else {
                IRQ_ERROR.fetch_or(2, Ordering::Relaxed);
            }
        }
        IRQ_RX_POS.store(pos, Ordering::Release);
    }
    r.int_clr().write(|w| unsafe { w.bits(pending.bits()) });
    r.int_clr().write(|w| {
        w.rxfifo_ovf()
            .clear_bit_by_one()
            .glitch_det()
            .clear_bit_by_one()
            .frm_err()
            .clear_bit_by_one()
            .parity_err()
            .clear_bit_by_one()
    });
    IRQ_FLAGS.fetch_or(flags, Ordering::Release);
}

fn irq_mask_and_reset() {
    // Handler is bound only on core0. Entering this local critical section
    // ensures no handler is in flight, and masking prevents future RX access.
    // Atomic storage is race-safe even if the interrupt controller misbehaves.
    critical_section::with(|_| {
        mask_irq(1);
        clear_raw(1);
        IRQ_FLAGS.store(0, Ordering::Relaxed);
        IRQ_FULL_COUNT.store(0, Ordering::Relaxed);
        IRQ_DONE_COUNT.store(0, Ordering::Relaxed);
        IRQ_TIMEOUT_COUNT.store(0, Ordering::Relaxed);
        IRQ_ERROR.store(0, Ordering::Relaxed);
        IRQ_RX_POS.store(0, Ordering::Release);
    });
}

fn irq_check_error(case: Case) {
    let error = IRQ_ERROR.load(Ordering::Acquire);
    if error != 0 {
        mask_irq(1);
        println!("HIL_UART_IRQ_ERROR flags={}", error);
        case.fail("ISR RX error or atomic buffer overflow");
    }
}

fn irq_send(uart: &mut Uart<'static, Blocking>, case: Case, start: Instant) {
    let mut pos = 0;
    while pos < case.len {
        irq_check_error(case);
        if start.elapsed().as_micros() > nominal_us(case.len, case.baud) * 3 + 100_000 {
            mask_irq(1);
            case.fail("IRQ send deadline");
        }
        if uart.write_ready() {
            let count = (case.len - pos).min(16);
            let mut chunk = [0u8; 16];
            for (i, byte) in chunk[..count].iter_mut().enumerate() {
                *byte = pattern(pos + i);
            }
            match uart.write(&chunk[..count]) {
                Ok(n) if n != 0 && n <= count => pos += n,
                _ => {
                    mask_irq(1);
                    case.fail("IRQ TX error");
                }
            }
        }
    }
}

fn irq_report(case: Case) {
    println!(
        "HIL_UART_IRQ uart=1 bytes={} flags={} full_count={} done_count={} timeout_count={} rx_count={}",
        case.len,
        IRQ_FLAGS.load(Ordering::Acquire),
        IRQ_FULL_COUNT.load(Ordering::Relaxed),
        IRQ_DONE_COUNT.load(Ordering::Relaxed),
        IRQ_TIMEOUT_COUNT.load(Ordering::Relaxed),
        IRQ_RX_POS.load(Ordering::Acquire)
    );
}

fn irq_config_readback(case: Case, timeout: bool) {
    let r = UART1::regs();
    let tout = r.tout_conf().read();
    if r.conf1().read().rxfifo_full_thrhd().bits() != 32
        || tout.rx_tout_en().bit() != timeout
        || (timeout && tout.rx_tout_thrhd().bits() != 100)
    {
        case.fail("IRQ threshold/timeout readback");
    }
}

fn irq_listen(uart: &mut Uart<'static, Blocking>, case: Case) {
    uart.listen(UartInterrupt::RxFifoFull | UartInterrupt::TxDone | UartInterrupt::RxTimeout);
    let enabled = UART1::regs().int_ena().read();
    if !enabled.rxfifo_full().bit() || !enabled.tx_done().bit() || !enabled.rxfifo_tout().bit() {
        mask_irq(1);
        case.fail("IRQ enable readback");
    }
}

fn irq_positive(uart: &mut Uart<'static, Blocking>, case: Case, required: u32) {
    irq_mask_and_reset();
    apply(
        uart,
        case,
        &config(case).with_rx(
            RxConfig::default()
                .with_fifo_full_threshold(32)
                .with_timeout(10),
        ),
    );
    irq_mask_and_reset();
    irq_config_readback(case, true);
    case.stage("IRQ enabled vector begin");
    irq_listen(uart, case);
    let start = Instant::now();
    irq_send(uart, case, start);
    loop {
        irq_check_error(case);
        let count = IRQ_RX_POS.load(Ordering::Acquire);
        let flags = IRQ_FLAGS.load(Ordering::Acquire);
        if count == case.len && flags & required == required {
            break;
        }
        if count > case.len {
            mask_irq(1);
            case.fail("IRQ extra RX bytes");
        }
        if start.elapsed().as_micros() > nominal_us(case.len, case.baud) * 3 + 100_000 {
            mask_irq(1);
            irq_report(case);
            case.fail("IRQ receive/event deadline");
        }
    }
    // Handoff: ISR is disabled before main inspects/reset/reconfigures RX.
    critical_section::with(|_| mask_irq(1));
    irq_check_error(case);
    for pos in 0..case.len {
        if IRQ_RX[pos].load(Ordering::Relaxed) != pattern(pos) {
            case.fail("IRQ exact byte mismatch");
        }
    }
    if regs!(1).status().read().rxfifo_cnt().bits() != 0 {
        case.fail("IRQ FIFO not empty");
    }
    if case.len == 1 && IRQ_FULL_COUNT.load(Ordering::Relaxed) != 0 {
        case.fail("subthreshold Full event");
    }
    irq_report(case);
    clear_raw(1);
}

fn drain_masked(uart: &mut Uart<'static, Blocking>, case: Case) {
    let mut bytes = [0u8; 128];
    let count = match uart.read_buffered(&mut bytes) {
        Ok(count) => count,
        Err(error) => {
            println!("HIL_UART_RX error={:?}", error);
            case.fail("masked drain RX error");
        }
    };
    if count != case.len {
        case.fail("masked drain length");
    }
    for (i, byte) in bytes[..count].iter().enumerate() {
        if *byte != pattern(i) {
            case.fail("masked drain exact byte mismatch");
        }
    }
    if regs!(1).status().read().rxfifo_cnt().bits() != 0 {
        case.fail("masked drain extra bytes");
    }
    clear_raw(1);
}

fn irq_disabled(uart: &mut Uart<'static, Blocking>, case: Case) {
    irq_mask_and_reset();
    apply(
        uart,
        case,
        &config(case).with_rx(
            RxConfig::default()
                .with_fifo_full_threshold(32)
                .with_timeout(10),
        ),
    );
    irq_mask_and_reset();
    irq_config_readback(case, true);
    case.stage("all IRQ events masked negative begin");
    let start = Instant::now();
    irq_send(uart, case, start);
    let r = UART1::regs();
    let required_full = case.len >= 32;
    loop {
        let raw = r.int_raw().read();
        if raw.tx_done().bit_is_set()
            && raw.rxfifo_tout().bit_is_set()
            && (!required_full || raw.rxfifo_full().bit_is_set())
        {
            break;
        }
        if start.elapsed().as_micros() > 100_000 {
            case.fail("masked raw event deadline");
        }
    }
    // Leave a subthreshold tail untouched for many timeout windows. Raw
    // events exist, but the bound handler must not observe any enabled event.
    let observation = Instant::now();
    while observation.elapsed().as_micros() < 20_000 {
        if IRQ_FLAGS.load(Ordering::Acquire) != 0 || IRQ_RX_POS.load(Ordering::Acquire) != 0 {
            case.fail("masked interrupt delivered");
        }
    }
    if r.int_ena().read().bits() != 0 || r.status().read().rxfifo_cnt().bits() as usize != case.len
    {
        case.fail("masked negative FIFO/enable readback");
    }
    println!(
        "HIL_UART_IRQ_MASKED bytes={} raw_full={} raw_done={} raw_timeout={} observed_flags={}",
        case.len,
        r.int_raw().read().rxfifo_full().bit(),
        r.int_raw().read().tx_done().bit(),
        r.int_raw().read().rxfifo_tout().bit(),
        IRQ_FLAGS.load(Ordering::Acquire)
    );
    drain_masked(uart, case);
}

fn irq_timeout_off(uart: &mut Uart<'static, Blocking>, case: Case) {
    irq_mask_and_reset();
    apply(
        uart,
        case,
        &config(case).with_rx(
            RxConfig::default()
                .with_fifo_full_threshold(32)
                .with_timeout_none(),
        ),
    );
    irq_config_readback(case, false);
    irq_mask_and_reset();
    case.stage("RX timeout engine disabled subthreshold negative begin");
    irq_listen(uart, case);
    let start = Instant::now();
    irq_send(uart, case, start);
    while IRQ_FLAGS.load(Ordering::Acquire) & DONE == 0 {
        irq_check_error(case);
        if start.elapsed().as_micros() > 100_000 {
            mask_irq(1);
            case.fail("timeout-off TxDone deadline");
        }
    }
    let observation = Instant::now();
    while observation.elapsed().as_micros() < 20_000 {
        irq_check_error(case);
        if IRQ_FLAGS.load(Ordering::Acquire) & (FULL | TIMEOUT) != 0
            || UART1::regs().int_raw().read().rxfifo_tout().bit_is_set()
        {
            mask_irq(1);
            case.fail("disabled RX timeout triggered");
        }
    }
    critical_section::with(|_| mask_irq(1));
    irq_report(case);
    if IRQ_RX_POS.load(Ordering::Acquire) != 0 {
        case.fail("timeout-off subthreshold unexpectedly drained");
    }
    drain_masked(uart, case);
}

fn irq_phase(uart: &mut Uart<'static, Blocking>) -> u32 {
    let base = Case {
        id: 1,
        clock: ClockSource::Xtal,
        baud: 115_200,
        len: 0,
    };
    irq_mask_and_reset();
    base.stage("set_interrupt_handler begin");
    uart.set_interrupt_handler(uart1_handler);
    let mut completed = 0;
    // First one-byte tail is received by Timeout, without waiting for Full.
    irq_positive(uart, Case { len: 1, ..base }, DONE | TIMEOUT);
    completed += 1;
    irq_positive(uart, Case { len: 129, ..base }, FULL | DONE | TIMEOUT);
    completed += 1;
    // 64 bytes assert all three raw flags without overflowing the 128-byte FIFO.
    irq_disabled(uart, Case { len: 64, ..base });
    completed += 1;
    irq_disabled(uart, Case { len: 7, ..base });
    completed += 1;
    irq_timeout_off(uart, Case { len: 7, ..base });
    completed += 1;
    // Restore the enabled timeout and prove payload/event recovery afterwards.
    irq_positive(uart, Case { len: 1, ..base }, DONE | TIMEOUT);
    completed += 1;
    irq_mask_and_reset();
    base.stage("IRQ phase complete; handler masked");
    completed
}

pub fn run(
    uart0: UART0<'static>,
    uart1: UART1<'static>,
    uart2: UART2<'static>,
    uart3: UART3<'static>,
    uart4: UART4<'static>,
) -> u32 {
    println!(
        "HIL_UART_SCOPE HP UART0..4 internal loopback only; no GPIO; LP_UART excluded; baud ratios XTAL-relative"
    );
    let (u0, c0) = matrix(0, uart0);
    let (mut u1, c1) = matrix(1, uart1);
    let (u2, c2) = matrix(2, uart2);
    let (u3, c3) = matrix(3, uart3);
    let (u4, c4) = matrix(4, uart4);
    let mut coverage = Coverage::default();
    for c in [c0, c1, c2, c3, c4] {
        coverage.matrix_vectors += c.matrix_vectors;
        coverage.timing_vectors += c.timing_vectors;
        coverage.negative_configs += c.negative_configs;
        coverage.recovery_vectors += c.recovery_vectors;
    }
    if coverage.matrix_vectors != EXPECTED_MATRIX_VECTORS
        || coverage.timing_vectors != 120
        || coverage.negative_configs != 25
        || coverage.recovery_vectors != 25
    {
        crate::fail("UART coverage totals");
    }
    let irq_checks = irq_phase(&mut u1);
    if irq_checks != 6 {
        crate::fail("UART IRQ coverage total");
    }
    for id in 0..5 {
        mask_irq(id);
        sync_loopback(
            Case {
                id,
                clock: ClockSource::Xtal,
                baud: 115_200,
                len: 0,
            },
            false,
        );
    }
    drop((u0, u1, u2, u3, u4));
    println!(
        "HIL_UART_SUMMARY matrix_vectors={} timing_vectors={} negative_configs={} recovery_vectors={} irq_checks={}",
        coverage.matrix_vectors,
        coverage.timing_vectors,
        coverage.negative_configs,
        coverage.recovery_vectors,
        irq_checks
    );
    coverage.matrix_vectors
}
