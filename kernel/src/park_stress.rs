//! Phase 0 exit test: IRQ-context `unpark` against thread-context `park`.
//!
//! Phase A passes a token around a ring of threads with cross-CPU `unpark` from thread context.
//! Phase B lets every timer tick (IRQ context) complete outstanding requests and `unpark` the
//! requesting thread; it is the only wake source, so a lost wakeup stalls the thread and trips
//! the watchdog. Enable with the `park-stress` feature; results are logged on the serial console.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use alloc::{sync::Arc, vec::Vec};
use klib::{
    scheduler::GLOBAL_SCHEDULER,
    stack::Stack,
    sync::{FairSpinlock, Parker},
    this_cpu,
    thread::Thread,
};
use log::{error, info};

const WORKERS: usize = 16;
const RING_LAPS: usize = 2000;
const IRQ_ROUNDS: usize = 300;
/// ticks (on CPU 0) to wait for secondary CPUs to come online before spawning workers.
const START_DELAY_TICKS: usize = 200;
/// ticks (on CPU 0) without progress before the test is declared stalled.
const STALL_TICKS: usize = 2000;

static PARKERS: FairSpinlock<Vec<Parker<'static>>> = FairSpinlock::new(Vec::new());
static REGISTERED: AtomicUsize = AtomicUsize::new(0);
static TURN: AtomicUsize = AtomicUsize::new(0);
static REQ: [AtomicUsize; WORKERS] = [const { AtomicUsize::new(0) }; WORKERS];
static DONE: [AtomicUsize; WORKERS] = [const { AtomicUsize::new(0) }; WORKERS];
static FINISHED: AtomicUsize = AtomicUsize::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static REPORTED: AtomicBool = AtomicBool::new(false);
static CPU0_TICKS: AtomicUsize = AtomicUsize::new(0);
static LAST_PROGRESS: AtomicUsize = AtomicUsize::new(0);
static LAST_CHANGE: AtomicUsize = AtomicUsize::new(0);

pub fn start() {
    let launcher = Arc::new(Thread::new_kernel(
        Stack::default(),
        launcher as *const (),
        1,
    ));
    GLOBAL_SCHEDULER.spawn(launcher);
}

extern "C" fn launcher() -> ! {
    while CPU0_TICKS.load(Ordering::Acquire) < START_DELAY_TICKS {
        klib::scheduler::Scheduler::yield_now();
    }

    for i in 0..WORKERS {
        let worker = Arc::new(Thread::new_kernel(Stack::default(), worker as *const (), 1));
        worker.with_ctx_mut(|ctx| ctx.registers[0] = i as u64);
        GLOBAL_SCHEDULER.spawn(worker);
    }
    STARTED.store(true, Ordering::Release);
    info!("park_stress: spawned {WORKERS} workers");

    loop {
        GLOBAL_SCHEDULER.park_current();
    }
}

extern "C" fn worker(id: usize) -> ! {
    let me = Parker::current(&GLOBAL_SCHEDULER);
    {
        let mut parkers = PARKERS.lock();
        if parkers.len() < WORKERS {
            parkers.resize(WORKERS, me.clone());
        }
        parkers[id] = me.clone();
    }
    REGISTERED.fetch_add(1, Ordering::AcqRel);
    while REGISTERED.load(Ordering::Acquire) < WORKERS {
        klib::scheduler::Scheduler::yield_now();
    }

    // phase A: token ring, wakeups from thread context.
    let end = WORKERS * RING_LAPS;
    loop {
        let turn = TURN.load(Ordering::Acquire);
        if turn >= end {
            break;
        }
        if turn % WORKERS != id {
            me.park(&GLOBAL_SCHEDULER);
            continue;
        }
        TURN.store(turn + 1, Ordering::Release);
        if turn + 1 >= end {
            wake_all();
        } else {
            let next = PARKERS.lock()[(turn + 1) % WORKERS].clone();
            next.unpark(&GLOBAL_SCHEDULER);
        }
    }

    // phase B: the timer tick (IRQ context) is the only wake source.
    for round in 0..IRQ_ROUNDS {
        REQ[id].store(round + 1, Ordering::SeqCst);
        while DONE[id].load(Ordering::SeqCst) <= round {
            me.park(&GLOBAL_SCHEDULER);
        }
    }

    if FINISHED.fetch_add(1, Ordering::AcqRel) + 1 == WORKERS && !REPORTED.swap(true, Ordering::AcqRel)
    {
        info!("park_stress: PASS ({RING_LAPS} laps, {IRQ_ROUNDS} irq rounds x {WORKERS} threads)");
    }

    loop {
        me.park(&GLOBAL_SCHEDULER);
    }
}

fn wake_all() {
    let parkers = PARKERS.lock().clone();
    for p in parkers {
        p.unpark(&GLOBAL_SCHEDULER);
    }
}

/// called from the timer interrupt handler, in IRQ context, before scheduling.
pub fn on_tick() {
    if !STARTED.load(Ordering::Acquire) {
        if this_cpu!().id.to_usize() == 0 {
            CPU0_TICKS.fetch_add(1, Ordering::AcqRel);
        }
        return;
    }

    for id in 0..WORKERS {
        let req = REQ[id].load(Ordering::SeqCst);
        if req > DONE[id].load(Ordering::SeqCst) && DONE[id].fetch_max(req, Ordering::SeqCst) < req {
            // PARKERS is a leaf IRQ-masking spinlock, so taking it here cannot self-deadlock.
            let parker = PARKERS.lock().get(id).cloned();
            if let Some(parker) = parker {
                parker.unpark(&GLOBAL_SCHEDULER);
            }
        }
    }

    if this_cpu!().id.to_usize() == 0 && !REPORTED.load(Ordering::Acquire) {
        let ticks = CPU0_TICKS.fetch_add(1, Ordering::AcqRel);
        let progress = TURN.load(Ordering::Relaxed)
            + DONE.iter().map(|d| d.load(Ordering::Relaxed)).sum::<usize>();
        if progress != LAST_PROGRESS.swap(progress, Ordering::Relaxed) {
            LAST_CHANGE.store(ticks, Ordering::Relaxed);
        } else if ticks - LAST_CHANGE.load(Ordering::Relaxed) > STALL_TICKS
            && !REPORTED.swap(true, Ordering::AcqRel)
        {
            error!(
                "park_stress: FAIL, stalled at TURN={} finished={}",
                TURN.load(Ordering::Relaxed),
                FINISHED.load(Ordering::Relaxed)
            );
        }
    }
}
