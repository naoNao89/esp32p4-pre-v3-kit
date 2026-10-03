use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use critical_section::Mutex;
use esp_hal::{
    Blocking,
    delay::Delay,
    handler,
    peripherals::{CPU_CTRL, TIMG0, TIMG1},
    ram,
    system::{AppCoreGuard, Cpu, CpuControl, Stack, is_running},
    time::{Duration, Instant},
    timer::{PeriodicTimer, timg::TimerGroup},
};
use esp_println::println;
use esp_sync::raw::{RawLock, SingleCoreInterruptLock};

const N: u32 = 500_000;
const IDLE: u32 = 0;
const FIXED: u32 = 1;
const SOAK: u32 = 2;
const STOP_SOAK: u32 = 3;
const FINISH: u32 = 4;
const ERR_ID: u32 = 1;
const ERR_TIMER: u32 = 2;
const ERR_MASK: u32 = 4;
const ERR_COLLISION: u32 = 8;
const ERR_PROTECTED: u32 = 16;
const ERR_TIMEOUT: u32 = 32;
const ERR_OVERFLOW: u32 = 64;

static ERRORS: AtomicU32 = AtomicU32::new(0);
static ABORT: AtomicBool = AtomicBool::new(false);
static COMMAND: AtomicU32 = AtomicU32::new(IDLE);
static CORE1_READY: AtomicBool = AtomicBool::new(false);
static CORE1_RAW_DONE: AtomicBool = AtomicBool::new(false);
static FIXED_DONE: AtomicBool = AtomicBool::new(false);
static SOAK_DONE: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static T0_TICKS: AtomicU32 = AtomicU32::new(0);
static T1_TICKS: AtomicU32 = AtomicU32::new(0);
static CORE1_PROGRESS: AtomicU32 = AtomicU32::new(0);
static CORE1_RAW_ITERATIONS: AtomicU32 = AtomicU32::new(0);
static CORE1_FIXED_ITERATIONS: AtomicU32 = AtomicU32::new(0);
static CORE1_SOAK_ITERATIONS: AtomicU32 = AtomicU32::new(0);
// Atomic even under CS: a broken exclusion implementation must not create Rust UB.
static CS_COUNTER: AtomicU32 = AtomicU32::new(0);
static SOAK_COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn request_abort() {
    ABORT.store(true, Ordering::Release);
}

fn core1_abort_if_requested() {
    if ABORT.load(Ordering::Acquire) {
        shutdown_core1();
        loop {
            core::hint::spin_loop();
        }
    }
}
static IN_SECTION: AtomicU32 = AtomicU32::new(0);
static TIMER0: Mutex<RefCell<Option<PeriodicTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));
static TIMER1: Mutex<RefCell<Option<PeriodicTimer<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));
static mut APP_CORE_STACK: Stack<8192> = Stack::new();

fn record(error: u32) {
    ERRORS.fetch_or(error, Ordering::Release);
}

fn hart_id() -> u32 {
    let id;
    unsafe {
        core::arch::asm!("csrr {0}, mhartid", out(reg) id, options(nomem, nostack));
    }
    id
}

fn interrupts_enabled() -> bool {
    let status: u32;
    unsafe {
        core::arch::asm!("csrr {0}, mstatus", out(reg) status, options(nomem, nostack));
    }
    status & 8 != 0
}

// Called only on core0, after every local lock/CS has been released.
fn check_errors(stage: &str) {
    let errors = ERRORS.load(Ordering::Acquire);
    if errors != 0 {
        println!("HIL_PROGRESS: stress_errors={:#x} stage={}", errors, stage);
        crate::fail(stage);
    }
}

#[handler]
#[ram]
fn t0_handler() {
    critical_section::with(|cs| {
        if let Some(timer) = TIMER0.borrow_ref_mut(cs).as_mut() {
            timer.clear_interrupt();
            if hart_id() != 0 {
                record(ERR_ID);
            }
            T0_TICKS.fetch_add(1, Ordering::Relaxed);
        } else {
            record(ERR_TIMER);
        }
    });
}

#[handler]
#[ram]
fn t1_handler() {
    critical_section::with(|cs| {
        if let Some(timer) = TIMER1.borrow_ref_mut(cs).as_mut() {
            timer.clear_interrupt();
            if hart_id() != 1 {
                record(ERR_ID);
            }
            T1_TICKS.fetch_add(1, Ordering::Relaxed);
        } else {
            record(ERR_TIMER);
        }
    });
}

fn wait_flag(flag: &AtomicBool, seconds: u64, stage: &str) {
    let start = Instant::now();
    while !flag.load(Ordering::Acquire) {
        check_errors(stage);
        if start.elapsed() >= Duration::from_secs(seconds) {
            crate::fail(stage);
        }
        core::hint::spin_loop();
    }
    check_errors(stage);
}

fn wait_command(expected: u32) -> bool {
    let start = Instant::now();
    while COMMAND.load(Ordering::Acquire) != expected {
        core1_abort_if_requested();
        CORE1_PROGRESS.fetch_add(1, Ordering::Relaxed);
        if start.elapsed() >= Duration::from_secs(180) {
            record(ERR_TIMEOUT);
            shutdown_core1();
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

// Both cores test nested tokens on their own thread, releasing in reverse order.
fn nested_raw(ticks: &AtomicU32) {
    let lock = SingleCoreInterruptLock;
    let delay = Delay::new();
    let violation = unsafe {
        let outer = lock.enter();
        let before = ticks.load(Ordering::Relaxed);
        let inner = lock.enter();
        delay.delay_millis(2);
        let mut bad = interrupts_enabled() || ticks.load(Ordering::Relaxed) != before;
        lock.exit(inner);
        delay.delay_millis(2);
        bad |= interrupts_enabled() || ticks.load(Ordering::Relaxed) != before;
        lock.exit(outer);
        bad
    };
    if violation || !interrupts_enabled() {
        record(ERR_MASK);
    }
}

// IN_SECTION covers the outer section only, including the nested-CS probe.
// All failures become atomic flags; the printing core reports them after unlock.
fn protected_update(counter: &AtomicU32, amount: u32, ticks: &AtomicU32, stretch: bool) {
    critical_section::with(|_| {
        if IN_SECTION.fetch_add(1, Ordering::SeqCst) != 0 {
            record(ERR_COLLISION);
        }
        let before = counter.load(Ordering::Relaxed);
        let local_ticks = ticks.load(Ordering::Relaxed);
        if interrupts_enabled() {
            record(ERR_MASK);
        }
        if stretch {
            critical_section::with(|_| {
                if interrupts_enabled() {
                    record(ERR_MASK);
                }
                Delay::new().delay_millis(1);
            });
            // Inner exit must preserve the outer mask and exclusion.
            Delay::new().delay_millis(1);
            if interrupts_enabled() || ticks.load(Ordering::Relaxed) != local_ticks {
                record(ERR_MASK);
            }
            if counter.load(Ordering::Relaxed) != before {
                record(ERR_PROTECTED);
            }
        }
        match before.checked_add(amount) {
            Some(next) => counter.store(next, Ordering::Relaxed),
            None => record(ERR_OVERFLOW),
        }
        if ticks.load(Ordering::Relaxed) != local_ticks {
            record(ERR_MASK);
        }
        if IN_SECTION.fetch_sub(1, Ordering::SeqCst) != 1 {
            record(ERR_COLLISION);
        }
    });
    if !interrupts_enabled() {
        record(ERR_MASK);
    }
}

fn core1_main(timg1: TIMG1<'static>) {
    if hart_id() != 1 {
        record(ERR_ID);
        // No printing and no silent hang: core0 observes ERR_ID in its bounded wait.
        loop {
            core::hint::spin_loop();
        }
    }
    let group = TimerGroup::new(timg1);
    let mut timer = PeriodicTimer::new(group.timer0);
    let started = critical_section::with(|cs| {
        timer.set_interrupt_handler(t1_handler);
        if timer.start(Duration::from_micros(1300)).is_err() {
            return false;
        }
        timer.listen();
        TIMER1.borrow_ref_mut(cs).replace(timer);
        true
    });
    if !started {
        record(ERR_TIMER);
        loop {
            core::hint::spin_loop();
        }
    }
    // The second-core bootstrap initializes vectors, but does not enable MIE.
    // Handler AND timer storage are now ready; enable global IRQs explicitly.
    unsafe {
        core::arch::asm!("csrsi mstatus, 8", options(nomem, nostack));
    }
    CORE1_READY.store(true, Ordering::Release);
    let started = Instant::now();
    while T1_TICKS.load(Ordering::Relaxed) < 10 {
        core1_abort_if_requested();
        if started.elapsed() >= Duration::from_secs(2) {
            record(ERR_TIMEOUT);
            shutdown_core1();
            loop {
                core::hint::spin_loop();
            }
        }
    }
    let lock = SingleCoreInterruptLock;
    let delay = Delay::new();
    let raw_start = Instant::now();
    for i in 0..100_000 {
        let bad = unsafe {
            let token = lock.enter();
            let before = T1_TICKS.load(Ordering::Relaxed);
            if i % 1000 == 0 {
                delay.delay_millis(2);
            } else {
                core::arch::asm!("nop", options(nomem, nostack));
            }
            let bad = interrupts_enabled() || T1_TICKS.load(Ordering::Relaxed) != before;
            lock.exit(token);
            bad
        };
        if bad || !interrupts_enabled() {
            record(ERR_MASK);
        }
        CORE1_RAW_ITERATIONS.store(i + 1, Ordering::Relaxed);
        core1_abort_if_requested();
        if raw_start.elapsed() >= Duration::from_secs(10) {
            record(ERR_TIMEOUT);
            break;
        }
    }
    nested_raw(&T1_TICKS);
    CORE1_RAW_DONE.store(true, Ordering::Release);

    if !wait_command(FIXED) {
        loop {
            core::hint::spin_loop();
        }
    }
    let fixed_start = Instant::now();
    for i in 0..N {
        protected_update(&CS_COUNTER, 1, &T1_TICKS, i % 1000 == 0);
        CORE1_FIXED_ITERATIONS.store(i + 1, Ordering::Relaxed);
        core1_abort_if_requested();
        if fixed_start.elapsed() >= Duration::from_secs(20) {
            record(ERR_TIMEOUT);
            break;
        }
    }
    FIXED_DONE.store(true, Ordering::Release);

    if !wait_command(SOAK) {
        loop {
            core::hint::spin_loop();
        }
    }
    let soak_start = Instant::now();
    let mut completed = 0u32;
    while COMMAND.load(Ordering::Acquire) == SOAK {
        protected_update(&SOAK_COUNTER, 1, &T1_TICKS, completed % 4096 == 0);
        core1_abort_if_requested();
        match completed.checked_add(1) {
            Some(next) => completed = next,
            None => {
                record(ERR_OVERFLOW);
                break;
            }
        }
        if soak_start.elapsed() >= Duration::from_secs(15) {
            record(ERR_TIMEOUT);
            break;
        }
    }
    CORE1_SOAK_ITERATIONS.store(completed, Ordering::Relaxed);
    SOAK_DONE.store(true, Ordering::Release);

    if !wait_command(FINISH) {
        loop {
            core::hint::spin_loop();
        }
    }
    shutdown_core1();
    loop {
        core::hint::spin_loop();
    }
}

fn shutdown_core1() {
    let lock = SingleCoreInterruptLock;
    // Remove ISR storage under CS, but unlisten/cancel OUTSIDE the CS.
    // The local raw mask makes that handoff safe against an interrupt seeing None.
    let stopped = unsafe {
        let token = lock.enter();
        let timer = critical_section::with(|cs| TIMER1.borrow_ref_mut(cs).take());
        let result = match timer {
            Some(mut timer) => {
                timer.unlisten();
                let ok = timer.cancel().is_ok();
                timer.clear_interrupt();
                ok
            }
            None => false,
        };
        lock.exit(token);
        result
    };
    if !stopped {
        record(ERR_TIMER);
    }
    // No CS/raw lock ownership remains; core0 may safely drop AppCoreGuard.
    DONE.store(true, Ordering::Release);
}

pub struct StressRunner {
    guard: AppCoreGuard<'static>,
    idle_ticks: u32,
    idle_progress: u32,
    complete: bool,
}

pub fn start(
    cpu_ctrl: CPU_CTRL<'static>,
    timg0: TIMG0<'static>,
    timg1: TIMG1<'static>,
) -> StressRunner {
    if hart_id() != 0 || Cpu::current() != Cpu::ProCpu {
        crate::fail("core0 ID");
    }
    println!("HIL_STAGE: TIMG0 setup period_us=1000");
    let group = TimerGroup::new(timg0);
    let mut timer = PeriodicTimer::new(group.timer0);
    let started = critical_section::with(|cs| {
        timer.set_interrupt_handler(t0_handler);
        if timer.start(Duration::from_millis(1)).is_err() {
            return false;
        }
        timer.listen();
        TIMER0.borrow_ref_mut(cs).replace(timer);
        true
    });
    if !started {
        crate::fail("TIMG0 start");
    }
    println!("HIL_STAGE: AppCpu startup TIMG1 period_us=1300 then core1 raw=100000");
    let mut cpu_control = CpuControl::new(cpu_ctrl);
    let guard = match cpu_control.start_app_core(
        unsafe { &mut *core::ptr::addr_of_mut!(APP_CORE_STACK) },
        move || core1_main(timg1),
    ) {
        Ok(guard) => guard,
        Err(_) => crate::fail("AppCpu start"),
    };
    wait_flag(&CORE1_READY, 2, "AppCpu ready");
    wait_flag(&CORE1_RAW_DONE, 10, "core1 raw checks");
    let start = Instant::now();
    while T0_TICKS.load(Ordering::Relaxed) < 10 || T1_TICKS.load(Ordering::Relaxed) < 10 {
        check_errors("dual timer startup");
        if start.elapsed() >= Duration::from_secs(2) {
            crate::fail("dual timer startup");
        }
    }
    if CORE1_RAW_ITERATIONS.load(Ordering::Relaxed) != 100_000 {
        crate::fail("core1 raw count");
    }
    println!(
        "HIL_PROGRESS: cores=0,1 core1_raw=100000 t0={} t1={} UART_idle_only=true",
        T0_TICKS.load(Ordering::Relaxed),
        T1_TICKS.load(Ordering::Relaxed)
    );
    StressRunner {
        guard,
        idle_ticks: T1_TICKS.load(Ordering::Relaxed),
        idle_progress: CORE1_PROGRESS.load(Ordering::Relaxed),
        complete: false,
    }
}

impl StressRunner {
    pub fn run_soak(&mut self) {
        check_errors("UART idle core1");
        if COMMAND.load(Ordering::Acquire) != IDLE
            || T1_TICKS.load(Ordering::Relaxed) == self.idle_ticks
            || CORE1_PROGRESS.load(Ordering::Relaxed) == self.idle_progress
        {
            crate::fail("core1 idle/IRQ progress during UART");
        }
        println!("HIL_STAGE: core0 raw=1000000 remote_inside_mask_probes=1000");
        let lock = SingleCoreInterruptLock;
        let delay = Delay::new();
        let raw_start = Instant::now();
        let mut probes = 0u32;
        for i in 0..1_000_000 {
            let (local_bad, remote_bad) = unsafe {
                let token = lock.enter();
                let local = T0_TICKS.load(Ordering::Relaxed);
                let remote_ticks = T1_TICKS.load(Ordering::Relaxed);
                let remote_progress = CORE1_PROGRESS.load(Ordering::Relaxed);
                let stretch = i % 1000 == 0;
                if stretch {
                    delay.delay_millis(2);
                } else {
                    core::arch::asm!("nop", options(nomem, nostack));
                }
                let local_bad = interrupts_enabled() || T0_TICKS.load(Ordering::Relaxed) != local;
                // Both snapshots and comparisons occur WITHIN this local mask window.
                let remote_bad = stretch
                    && (T1_TICKS.load(Ordering::Relaxed) == remote_ticks
                        || CORE1_PROGRESS.load(Ordering::Relaxed) == remote_progress);
                lock.exit(token);
                (local_bad, remote_bad)
            };
            if local_bad || !interrupts_enabled() {
                crate::fail("core0 raw local mask");
            }
            if remote_bad {
                crate::fail("remote progress inside core0 raw mask");
            }
            if i % 1000 == 0 {
                probes += 1;
            }
            check_errors("core0 raw");
            if raw_start.elapsed() >= Duration::from_secs(20) {
                crate::fail("core0 raw deadline");
            }
        }
        nested_raw(&T0_TICKS);
        check_errors("nested raw");
        println!(
            "HIL_PROGRESS: core0_raw=1000000 core1_raw=100000 masked_remote_probes={} nested_raw_both=true",
            probes
        );

        println!("HIL_STAGE: crosscore fixed CS updates_per_core=500000 increments=2,1");
        COMMAND.store(FIXED, Ordering::Release);
        let fixed_start = Instant::now();
        for i in 0..N {
            protected_update(&CS_COUNTER, 2, &T0_TICKS, i % 1000 == 0);
            check_errors("fixed CS exclusion/nesting/mask");
            if fixed_start.elapsed() >= Duration::from_secs(20) {
                crate::fail("fixed CS deadline");
            }
        }
        wait_flag(&FIXED_DONE, 20, "core1 fixed CS completion");
        let actual = critical_section::with(|_| CS_COUNTER.load(Ordering::Relaxed));
        if CORE1_FIXED_ITERATIONS.load(Ordering::Relaxed) != N
            || actual != 3 * N
            || IN_SECTION.load(Ordering::SeqCst) != 0
        {
            crate::fail("fixed CS exact count");
        }
        println!(
            "HIL_PROGRESS: fixed_cs core0={} core1={} protected={} expected=1500000 nested_cs_both=true",
            N, N, actual
        );

        println!("HIL_STAGE: dualcore dualIRQ CS soak duration_us=10000000");
        let ticks0 = T0_TICKS.load(Ordering::Relaxed);
        let ticks1 = T1_TICKS.load(Ordering::Relaxed);
        let start = Instant::now();
        COMMAND.store(SOAK, Ordering::Release);
        let mut iterations0 = 0u32;
        while start.elapsed() < Duration::from_secs(10) {
            protected_update(&SOAK_COUNTER, 2, &T0_TICKS, iterations0 % 4096 == 0);
            iterations0 = match iterations0.checked_add(1) {
                Some(next) => next,
                None => crate::fail("soak iteration overflow"),
            };
            check_errors("10s CS soak");
        }
        COMMAND.store(STOP_SOAK, Ordering::Release);
        let elapsed_us = start.elapsed().as_micros();
        wait_flag(&SOAK_DONE, 2, "core1 soak completion");
        let iterations1 = CORE1_SOAK_ITERATIONS.load(Ordering::Relaxed);
        let protected = critical_section::with(|_| SOAK_COUNTER.load(Ordering::Relaxed));
        let expected = u64::from(iterations0) * 2 + u64::from(iterations1);
        let delta0 = T0_TICKS.load(Ordering::Relaxed).wrapping_sub(ticks0);
        let delta1 = T1_TICKS.load(Ordering::Relaxed).wrapping_sub(ticks1);
        if elapsed_us < 10_000_000
            || elapsed_us > 12_000_000
            || iterations0 == 0
            || iterations1 == 0
            || u64::from(protected) != expected
            || delta0 < 10
            || delta1 < 10
            || IN_SECTION.load(Ordering::SeqCst) != 0
        {
            crate::fail("10s soak exact count/dual IRQ/duration");
        }
        check_errors("10s CS soak complete");
        println!(
            "HIL_PROGRESS: soak elapsed_us={} core0_iterations={} core1_iterations={} protected={} expected={} t0_delta={} t1_delta={}",
            elapsed_us, iterations0, iterations1, protected, expected, delta0, delta1
        );
        self.complete = true;
    }

    pub fn finish(self) -> u32 {
        if !self.complete {
            crate::fail("stress incomplete");
        }
        println!("HIL_STAGE: AppCpu DONE handoff outside locks then guard drop");
        COMMAND.store(FINISH, Ordering::Release);
        wait_flag(&DONE, 2, "AppCpu DONE/TIMG1 shutdown");
        if IN_SECTION.load(Ordering::SeqCst) != 0 {
            crate::fail("AppCpu DONE while CS occupied");
        }
        check_errors("AppCpu shutdown");
        drop(self.guard);
        if is_running(Cpu::AppCpu) || !is_running(Cpu::ProCpu) {
            crate::fail("CPU park/running state");
        }
        println!("HIL_PROGRESS: AppCpu_stalled=true ProCpu_running=true T1_stopped=true");
        3 * N
    }
}
