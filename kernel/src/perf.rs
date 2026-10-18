//! Perf counters for the desktop event loop — **measurement only**.
//!
//! ### Why this module exists
//!
//! Two symptoms are on the table: the cursor is visibly laggy, and the guest
//! heartbeat claims ~49 s of uptime after 120 s of wall-clock. Both point at
//! the event loop in `crate::desktop_run`, but "the loop is slow" hides three
//! very different bugs — and fixing the wrong one wastes the session:
//!
//! 1. *Work per wake is too big*: every 100 Hz `Event::Tick` triggers a full
//!    `compose` (which starts with a full-screen `g.clear()`) plus a 3 MiB
//!    `flush`, so a single iteration takes >10 ms and the loop can never
//!    keep up with the PIT;
//! 2. *Events are lost*: ticks are produced faster than they are drained and
//!    overflow the ring, or the `Tick` events that do arrive carry stale
//!    tick numbers (the uptime anomaly);
//! 3. *Input never arrives*: VirtualBox delivers no PS/2 mouse packets, so
//!    the cursor can only move through the keyboard fallback at all.
//!
//! This module settles **which** one it is by measuring — never by changing
//! what the loop does. Zero behaviour change by construction:
//!
//! * no existing log line was removed or moved, and no compose/flush/dirty
//!   decision was touched to add these hooks;
//! * the aggregator is one plain local struct — no allocation, no locks, no
//!   global state beyond the single `AtomicU64` holding the calibration;
//! * a TSC read is three instructions, and every helper below is
//!   `#[inline]`/`#[inline(always)]` — free next to a 3 MiB framebuffer
//!   copy.
//!
//! ### What each number answers
//!
//! * `frames/s` — how often the loop redrew in the last second. ~100 means
//!   "redraws on literally every PIT tick" (the starvation hypothesis);
//!   far less means it only repaints when something actually changed.
//! * `compose_*` vs `flush_*` — the split between `WindowManager::compose`
//!   (starts with a full-screen clear) and `Graphics::flush` (full 3 MiB
//!   copy to the framebuffer). Whichever half dominates owns the per-frame
//!   cost, and therefore the fix.
//! * `mouse_evt/s` — mouse events that actually reached the loop. 0 next to
//!   a frozen cursor means the packets never arrive (or never get read) —
//!   an *input* problem, not a redraw problem.
//! * `events_drained_avg/max` — events popped per drain pass. A max that
//!   keeps growing means the loop is falling behind its producers: it wakes
//!   up, finds an ever-fatter pile, and spends the whole wake draining it.
//! * `wakes/s` — loop iterations, i.e. resumes from `hlt`. ~100 means the
//!   loop wakes for every tick; well under 100 means single iterations eat
//!   more than 10 ms. Read together with `frames/s`: `wakes ≈ 100` with
//!   `frames ≈ 100` = redraw-every-tick; `frames ≪ wakes` = the dirty flag
//!   is doing its job.
//! * `hw_ticks` vs `evt_ticks` vs `dropped` — the uptime-anomaly
//!   discriminator, printed raw with no scaling: `hw_ticks` is
//!   `timer::ticks()` (hardware truth, bumped by IRQ0), `evt_ticks` is the
//!   tick value the heartbeat `Event::Tick` carried (what the *event
//!   stream* believes at report time), and `dropped` is `event::dropped()`
//!   (ring-buffer overflows since boot). Read them against each other:
//!   `hw_ticks` racing ahead of `evt_ticks` ⇒ ticks exist in hardware but
//!   are not reaching (or not being drained by) the loop; `dropped > 0`
//!   ⇒ they overflowed the queue on the way; *both* flat while wall-clock
//!   advances ⇒ IRQs themselves are being lost.
//!
//! ### TSC calibration
//!
//! [`tsc_now`] reads the x86 time-stamp counter, but "TSC ticks" are not
//! microseconds until someone knows the frequency. [`calibrate`] measures
//! the TSC against the PIT over [`CALIBRATE_TICKS`] ticks (500 ms) with pure
//! integer math — no floats (the kernel target is `+soft-float`; an f64
//! divide would be software anyway) — and stores cycles-per-microsecond in
//! [`CYCLES_PER_US`]. Every conversion answers `0` µs when calibration is
//! missing or failed, so a broken measurement reads as `0` instead of a
//! plausible-looking lie.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::timer;

/// TSC cycles per microsecond, measured by [`calibrate`] against the PIT.
///
/// `0` = not calibrated yet, or calibration failed — every conversion then
/// reports `0` µs rather than inventing a number.
static CYCLES_PER_US: AtomicU64 = AtomicU64::new(0);

/// Calibration window: 50 PIT ticks = 500 ms at [`timer::TICKS_PER_SECOND`]
/// — long enough to swamp tick-arrival jitter, short enough that nobody
/// notices the boot delay.
const CALIBRATE_TICKS: u64 = 50;

/// Bounded-fallback guard #1 (spin iterations): a broken PIT must never
/// hang boot, so the wait for [`CALIBRATE_TICKS`] also gives up after this
/// many `spin_loop()`s. The value is deliberately far above what 500 ms
/// needs — even a pathological ~1 spin/cycle `pause` at ~4 GHz burns only
/// ~1.3 s to reach it, while the healthy path exits on ticks at 0.5 s — so
/// the guard can fire *only* when the PIT really is dead.
const CALIBRATE_SPIN_LIMIT: u64 = 4_000_000_000;

/// Bounded-fallback guard #2 (TSC cycles): if the PIT is dead but the TSC
/// runs, give up after this many cycles — ≥2 s at any plausible frequency
/// (a 20 GHz CPU would be needed to reach it in the 500 ms window), and
/// bounded to seconds even on a slow one. Together with the spin guard this
/// covers either clock dying alone: dead PIT ⇒ this fires; dead TSC (so the
/// cycle check never advances) ⇒ the spin guard fires.
const CALIBRATE_TSC_LIMIT: u64 = 10_000_000_000;

// ---------------------------------------------------------------------------
// TSC primitives
// ---------------------------------------------------------------------------

/// Current value of the x86 time-stamp counter, fenced so the read is not
/// reordered against surrounding work (the `lfence` before/after pairs make
/// the *instructions around* the `rdtsc` land outside the measured window —
/// exactly what the compose/flush split needs).
#[inline(always)]
pub fn tsc_now() -> u64 {
    // SAFETY: `lfence` and `rdtsc` are unprivileged, side-effect-free reads
    // of the counter — no memory, I/O or FP state is touched, so there is
    // nothing to invalidate or race with. CPUID.EDX:TSC is set on every
    // x86_64 (and VirtualBox/QEMU emulate it), so `rdtsc` cannot #UD here;
    // the call-in-`unsafe` form is required because the intrinsics carry
    // `#[target_feature]` attributes while this target builds with
    // `-sse2` (`x86_64-unknown-none`) — the instructions themselves do not
    // depend on the FPU/SSE state the target disables.
    unsafe {
        core::arch::x86_64::_mm_lfence();
        let tsc = core::arch::x86_64::_rdtsc();
        core::arch::x86_64::_mm_lfence();
        tsc
    }
}

/// Microseconds elapsed since the `tsc_now()` snapshot `t0`.
///
/// Returns `0` when the TSC was never calibrated (see [`CYCLES_PER_US`]) —
/// an uncalibrated µs number would be worse than no number at all.
///
/// The desktop loop times its two phases via the two-snapshot
/// [`us_between`] form, so nothing calls this one *yet*; it is the
/// one-ended twin kept alongside it (the `#[allow(dead_code)]` is only for
/// that, the kernel being a bin crate where `pub` is not "exported").
#[allow(dead_code)]
#[inline]
pub fn us_since(t0: u64) -> u64 {
    us_between(t0, tsc_now())
}

/// Microseconds between two [`tsc_now`] snapshots, rounded to nearest.
///
/// Companion to [`us_since`] for interval timing: callers snapshot once and
/// diff both ends (`a..b`), which keeps a pair of measurements comparable
/// even if the conversion constant were re-calibrated between them (it is
/// not — calibration happens once at boot — but the two-snapshot shape is
/// what the compose/flush split uses). Returns `0` while uncalibrated.
#[inline]
pub fn us_between(start: u64, end: u64) -> u64 {
    let cycles_per_us = CYCLES_PER_US.load(Ordering::Relaxed);
    if cycles_per_us == 0 {
        // Calibration failed or has not run yet: no honest conversion
        // exists, so the honest answer is "0 µs", never a guess.
        return 0;
    }
    // `u128` for the intermediate so the round-to-nearest add can never
    // overflow; `cycles_per_us >= 1` here, so the result fits back in u64.
    let cycles = end.wrapping_sub(start) as u128;
    let cycles_per_us = cycles_per_us as u128;
    ((cycles + cycles_per_us / 2) / cycles_per_us) as u64
}

// ---------------------------------------------------------------------------
// PIT-based calibration
// ---------------------------------------------------------------------------

/// Measures the TSC against the PIT once at boot and stores
/// cycles-per-microsecond in [`CYCLES_PER_US`].
///
/// Call **after** `timer::init` (the PIT must be programmed) **and** after
/// interrupts are enabled: `timer::ticks()` only advances from the IRQ0
/// handler, so with `IF=0` the counter is frozen and this function would
/// spin against it until a guard tripped. Both guards keep a genuinely
/// broken PIT from hanging boot — see [`CALIBRATE_SPIN_LIMIT`] and
/// [`CALIBRATE_TSC_LIMIT`].
///
/// Prints exactly one line on success or failure:
///
/// ```text
/// [perf   ] tsc calibrated: {cycles_per_us} cycles/us over {elapsed_ticks} PIT ticks
/// [perf   ] tsc calibration FAILED, us timings will print as 0
/// ```
pub fn calibrate() {
    let t0 = tsc_now();
    let tick0 = timer::ticks();

    // Spin (never `hlt`) until the PIT has produced 50 ticks: halting here
    // would make the spin-count fallback meaningless — a CPU in `hlt` with
    // a dead PIT never executes another instruction to check its bound.
    let mut spins = 0u64;
    loop {
        if timer::ticks().wrapping_sub(tick0) >= CALIBRATE_TICKS {
            break;
        }
        core::hint::spin_loop();
        spins = spins.wrapping_add(1);
        if spins >= CALIBRATE_SPIN_LIMIT || tsc_now().wrapping_sub(t0) >= CALIBRATE_TSC_LIMIT {
            calibrate_failed();
            return;
        }
    }

    let t1 = tsc_now();
    let elapsed_ticks = timer::ticks().wrapping_sub(tick0);

    // ticks -> whole microseconds (integer only):
    //   elapsed_ticks / TICKS_PER_SECOND seconds = elapsed_us µs.
    let elapsed_us = elapsed_ticks as u128 * 1_000_000 / timer::TICKS_PER_SECOND as u128;
    let cycles = t1.wrapping_sub(t0) as u128;
    if elapsed_us == 0 || cycles == 0 {
        // Division-by-zero guard (and "TSC did not move") — a zero window
        // has no ratio to compute. Same contract as any other failure.
        calibrate_failed();
        return;
    }

    // cycles / elapsed_us, rounded to nearest (add half the divisor).
    let cycles_per_us = ((cycles + elapsed_us / 2) / elapsed_us) as u64;
    CYCLES_PER_US.store(cycles_per_us, Ordering::Relaxed);
    serial_println!(
        "[perf   ] tsc calibrated: {cycles_per_us} cycles/us over {elapsed_ticks} PIT ticks"
    );
}

/// Failure path shared by both guards: store 0 so every [`us_between`] /
/// [`us_since`] reports 0 µs, and say so verbatim on the serial log.
fn calibrate_failed() {
    CYCLES_PER_US.store(0, Ordering::Relaxed);
    serial_println!("[perf   ] tsc calibration FAILED, us timings will print as 0");
}

// ---------------------------------------------------------------------------
// The aggregator
// ---------------------------------------------------------------------------

/// One second's worth of loop statistics, kept in a plain local inside
/// `desktop_run` (no lock, no allocation) and reset by every
/// [`Perf::report`].
///
/// Field docs repeat the module's question-per-number table so the answer
/// is visible at the hook site too:
///
/// * [`frames`](Perf::frames) — redraws: is the loop repainting every tick?
/// * `compose_*`/`flush_*` — the expensive half of a redraw.
/// * [`mouse_evts`](Perf::mouse_evts) — is input arriving at all?
/// * `drained_*` — how fat are the drain batches (falling behind?)?
/// * [`wakes`](Perf::wakes) — how often the loop really resumed from `hlt`.
pub struct Perf {
    /// Redraws this second (one per `dirty` pass through the loop).
    pub frames: u64,
    /// Σ `wm.compose` time (µs) — numerator of `compose_avg_us`.
    pub compose_sum: u64,
    /// Slowest single `wm.compose` (µs) — a spike the average would hide.
    pub compose_max: u64,
    /// Σ `gfx.flush` time (µs) — numerator of `flush_avg_us`.
    pub flush_sum: u64,
    /// Slowest single `gfx.flush` (µs).
    pub flush_max: u64,
    /// `Event::Mouse` events drained this second — is the input path alive?
    pub mouse_evts: u64,
    /// Σ events popped across all drain passes — numerator of
    /// `events_drained_avg`.
    pub drained_sum: u64,
    /// Biggest single drain pass — growing ⇒ the loop is falling behind.
    pub drained_max: u64,
    /// Loop iterations (resumes from `hlt`) — ~100 ⇒ keeping up with the PIT.
    pub wakes: u64,
}

impl Perf {
    /// All-zero instance; also the reset value used by [`Perf::report`].
    pub fn new() -> Self {
        Self {
            frames: 0,
            compose_sum: 0,
            compose_max: 0,
            flush_sum: 0,
            flush_max: 0,
            mouse_evts: 0,
            drained_sum: 0,
            drained_max: 0,
            wakes: 0,
        }
    }

    /// One redraw finished: `wm.compose` took `compose_us`, the subsequent
    /// `gfx.flush` took `flush_us` (both in microseconds from
    /// [`us_between`]; 0 while uncalibrated).
    pub fn on_frame(&mut self, compose_us: u64, flush_us: u64) {
        self.frames += 1;
        self.compose_sum += compose_us;
        self.compose_max = self.compose_max.max(compose_us);
        self.flush_sum += flush_us;
        self.flush_max = self.flush_max.max(flush_us);
    }

    /// One drain pass finished: `events_in_batch` events were popped, of
    /// which `mouse_events_in_batch` were `Event::Mouse`. Batch size is the
    /// "falling behind the producers" tell; the mouse share separates input
    /// traffic from the 100 Hz tick background.
    pub fn on_drain(&mut self, events_in_batch: u64, mouse_events_in_batch: u64) {
        self.drained_sum += events_in_batch;
        self.drained_max = self.drained_max.max(events_in_batch);
        self.mouse_evts += mouse_events_in_batch;
    }

    /// One loop iteration resumed from `hlt` — a genuine wakeup (an IRQ
    /// actually brought the CPU back), i.e. the denominator of "how often
    /// does the loop get a chance to run at all".
    pub fn on_wake(&mut self) {
        self.wakes += 1;
    }

    /// Prints the three 1 Hz `[perf   ]` lines, then resets every counter
    /// so the next call reports the *next* second, not a running total.
    ///
    /// Arguments stay raw (no scaling): `hw_ticks` is `timer::ticks()`
    /// (hardware truth), `evt_ticks` is the heartbeat event's carried tick
    /// value, `dropped` is `event::dropped()` — together they discriminate
    /// lost IRQs from dropped events (see the module docs).
    ///
    /// An idle second (`frames == 0`) prints averages as 0 instead of
    /// dividing by zero — the aggregator must never panic the desktop.
    pub fn report(&mut self, hw_ticks: u64, evt_ticks: u64, dropped: u64) {
        let frames = self.frames;
        // An idle second (`frames == 0`) answers 0 for the averages instead
        // of dividing by zero — the aggregator must never panic the
        // desktop (`checked_div` is None exactly on that case).
        let compose_avg = self.compose_sum.checked_div(frames).unwrap_or(0);
        let flush_avg = self.flush_sum.checked_div(frames).unwrap_or(0);
        let drained_avg = self.drained_sum.checked_div(frames).unwrap_or(0);
        let compose_max = self.compose_max;
        let flush_max = self.flush_max;
        let mouse_evts = self.mouse_evts;
        let drained_max = self.drained_max;
        let wakes = self.wakes;

        serial_println!(
            "[perf   ] frames/s={frames} compose_avg_us={compose_avg} compose_max_us={compose_max} flush_avg_us={flush_avg} flush_max_us={flush_max}"
        );
        serial_println!(
            "[perf   ] mouse_evt/s={mouse_evts} events_drained_avg={drained_avg} events_drained_max={drained_max} wakes/s={wakes}"
        );
        serial_println!(
            "[perf   ] uptime hw_ticks={hw_ticks} evt_ticks={evt_ticks} dropped={dropped}"
        );

        *self = Self::new();
    }
}
