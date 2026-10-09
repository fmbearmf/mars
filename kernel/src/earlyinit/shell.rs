use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering, fence};
use kernel_shell::{
    CommandRequest, CpuState, OUTPUT_CAPACITY, PROMPT, Shell, Snapshot, Telemetry, ThreadKind,
    ThreadMetadata, ThreadState,
};
use klib::{scheduler::GLOBAL_SCHEDULER, stack::Stack, thread::Thread};

use super::earlycon::earlycon_read_byte;

static STARTED: AtomicBool = AtomicBool::new(false);
static UPTIME_START: AtomicU64 = AtomicU64::new(0);

const MAX_WORKERS: usize = 512;
const WORK_CHUNKS: usize = 128;
const WORK_ITERATIONS: usize = 16_384;
const WORK_TIMEOUT_SECONDS: u64 = 15;
const WORK_MAX_POLLS: usize = 20_000_000;

struct WorkerRecord {
    initial_cpu: AtomicU64,
    final_cpu: AtomicU64,
    segment_seq: AtomicU64,
    segment_start: AtomicU64,
    segment_end: AtomicU64,
    segment_cpu: AtomicU64,
    done: AtomicBool,
    thread_id: AtomicU64,
    progress: AtomicU64,
}

const EMPTY_WORKER: WorkerRecord = WorkerRecord {
    initial_cpu: AtomicU64::new(u64::MAX),
    final_cpu: AtomicU64::new(u64::MAX),
    segment_seq: AtomicU64::new(0),
    segment_start: AtomicU64::new(0),
    segment_end: AtomicU64::new(0),
    segment_cpu: AtomicU64::new(u64::MAX),
    done: AtomicBool::new(false),
    thread_id: AtomicU64::new(u64::MAX),
    progress: AtomicU64::new(0),
};

static WORKERS: [WorkerRecord; MAX_WORKERS] = [const { EMPTY_WORKER }; MAX_WORKERS];
static WORK_TRIAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static WORK_TRIAL_COUNT: AtomicU64 = AtomicU64::new(0);

pub fn initialize_uptime() {
    let _ = UPTIME_START.compare_exchange(
        0,
        hal::timer::counter(),
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

pub fn start() {
    if STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let stack = Stack::default();
    let thread = Arc::new(Thread::new_kernel(stack, shell_entry, 1));
    GLOBAL_SCHEDULER.spawn(thread);
}

extern "C" fn shell_entry(_: usize) -> ! {
    let mut shell = Shell::new();
    let mut output = [0; OUTPUT_CAPACITY];
    let mut telemetry = KernelTelemetry;
    write(PROMPT);

    loop {
        match earlycon_read_byte() {
            Ok(Some(byte)) => {
                shell.feed_streaming_with(byte, &mut telemetry, &mut |bytes| write(bytes));
            }
            Ok(None) => klib::scheduler::Scheduler::yield_now(),
            Err(()) => {
                let len = shell.input_error(&mut output);
                write(&output[..len]);
            }
        }
    }
}

struct KernelTelemetry;

impl Telemetry for KernelTelemetry {
    fn snapshot(&mut self, request: CommandRequest, index: usize) -> Option<Snapshot> {
        match request {
            CommandRequest::Uptime => {
                let frequency = hal::timer::frequency_hz();
                if frequency == 0 {
                    return None;
                }
                let elapsed =
                    hal::timer::counter().wrapping_sub(UPTIME_START.load(Ordering::Acquire));
                Some(Snapshot::Uptime {
                    seconds: elapsed / frequency,
                })
            }
            CommandRequest::Cpu => Some(Snapshot::Cpu {
                logical_id: klib::cpu_interface::CpuIdLogical::current().to_u32(),
            }),
            CommandRequest::Memory => Some(Snapshot::Memory {
                heap_used: klib::vm::KALLOCATOR.heap_usage(),
                page_used: klib::vm::KALLOCATOR.page_usage(),
                capacity: klib::vm::KALLOCATOR.capacity(),
            }),
            CommandRequest::Sched => {
                let (ready, injector) = GLOBAL_SCHEDULER.try_queue_snapshot()?;
                Some(Snapshot::Sched { ready, injector })
            }
            CommandRequest::Work => {
                run_work();
                None
            }
            CommandRequest::Cpus => {
                let cpu = klib::per_cpu::PerCpu::get(index)?;
                let (state, thread) = if !cpu.ready.load(Ordering::Acquire) {
                    (CpuState::NotReady, None)
                } else {
                    match GLOBAL_SCHEDULER.try_cpu_snapshot(cpu.id) {
                        Some(snapshot) => {
                            let thread = snapshot.thread;
                            let state = if thread.idle {
                                CpuState::Idle
                            } else {
                                CpuState::Active
                            };
                            (
                                state,
                                Some(ThreadMetadata {
                                    id: thread.id,
                                    state: match thread.state {
                                        klib::thread::ThreadState::Running => ThreadState::Running,
                                        klib::thread::ThreadState::Ready => ThreadState::Ready,
                                        klib::thread::ThreadState::Blocked => ThreadState::Blocked,
                                        klib::thread::ThreadState::Dead => ThreadState::Dead,
                                    },
                                    kind: if thread.is_kernel {
                                        ThreadKind::Kernel
                                    } else {
                                        ThreadKind::User
                                    },
                                    idle: thread.idle,
                                }),
                            )
                        }
                        None => (CpuState::Busy, None),
                    }
                };
                Some(Snapshot::CpuState {
                    logical_id: cpu.id.to_u32(),
                    state,
                    thread,
                })
            }
        }
    }
}

fn run_work() {
    let cpus = klib::per_cpu::PerCpu::all()
        .iter()
        .filter(|cpu| cpu.ready.load(Ordering::Acquire))
        .count();

    let count = cpus.saturating_mul(2);
    if count == 0 || count > MAX_WORKERS {
        klib::print!("work: unavailable CPU count {cpus}; worker limit {MAX_WORKERS}\r\n");
        return;
    }

    if WORK_TRIAL_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        let previous_count = WORK_TRIAL_COUNT.load(Ordering::Acquire) as usize;
        if previous_count != 0
            && WORKERS[..previous_count]
                .iter()
                .all(|worker| worker.done.load(Ordering::Acquire))
            && WORK_TRIAL_ACTIVE
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            && WORK_TRIAL_ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            // all previous workers published completion before their records can be reused
        } else {
            klib::print!(
                "work: previous trial still has active workers; retry after they finish\r\n"
            );

            return;
        }
    }

    WORK_TRIAL_COUNT.store(count as u64, Ordering::Release);

    for worker in &WORKERS[..count] {
        worker.initial_cpu.store(u64::MAX, Ordering::Relaxed);
        worker.final_cpu.store(u64::MAX, Ordering::Relaxed);
        worker.segment_seq.store(0, Ordering::Relaxed);
        worker.segment_start.store(0, Ordering::Relaxed);
        worker.segment_end.store(0, Ordering::Relaxed);
        worker.segment_cpu.store(u64::MAX, Ordering::Relaxed);
        worker.done.store(false, Ordering::Relaxed);
        worker.thread_id.store(u64::MAX, Ordering::Relaxed);
        worker.progress.store(0, Ordering::Relaxed);
    }

    let mut threads = Vec::with_capacity(count);
    for id in 0..count {
        let stack = Stack::default();
        let thread = Arc::new(Thread::new_kernel_with_arg(stack, work_worker, 1, id));

        WORKERS[id]
            .thread_id
            .store(u64::from(thread.thread_id()), Ordering::Relaxed);

        threads.push(thread);
    }

    let launch_cpu = klib::cpu_interface::CpuIdLogical::current().to_usize();
    let mut steal_cursor = GLOBAL_SCHEDULER.steal_cursor();
    if GLOBAL_SCHEDULER
        .spawn_batch_on_cpu(launch_cpu, &threads)
        .is_err()
    {
        WORK_TRIAL_ACTIVE.store(false, Ordering::Release);
        klib::print!(
            "work: rejected {count} workers. cpu {launch_cpu} local queue lacks the capacity\r\n"
        );

        return;
    }
    if cpus == 1 {
        klib::print!(
            "work: {cpus} CPU; queued {count} workers on CPU {launch_cpu}. Work stealing will not occur (there's one CPU).\r\n"
        );
    } else {
        klib::print!(
            "work: {cpus} CPUs (concurrent); queued {count} workers on CPU {launch_cpu}.\r\n"
        );
    }

    let start = hal::timer::counter();
    let timeout_ticks = hal::timer::frequency_hz().saturating_mul(WORK_TIMEOUT_SECONDS);

    let mut reported = [0u64; MAX_WORKERS];
    let mut steal_events = [klib::scheduler::StealEvent {
        sequence: 0,
        source_cpu: 0,
        destination_cpu: 0,
        thread_id: 0,
    }; 16];

    let mut simultaneous_reported = false;
    let mut steals_observed = 0usize;
    let mut last_report = start;
    let mut polls = 0;
    loop {
        let completed = WORKERS[..count]
            .iter()
            .filter(|worker| worker.done.load(Ordering::Acquire))
            .count();

        let now = hal::timer::counter();
        let frequency = hal::timer::frequency_hz();

        if completed == count
            || frequency == 0
            || now.wrapping_sub(last_report) >= (frequency / 100).max(1)
        {
            last_report = now;

            for (id, worker) in WORKERS[..count].iter().enumerate() {
                let progress = worker.progress.load(Ordering::Acquire);

                if progress >= reported[id] + 8
                    || (progress != 0 && worker.done.load(Ordering::Acquire))
                {
                    reported[id] = progress;

                    let cpu = worker.segment_cpu.load(Ordering::Acquire);
                    let percent = progress.saturating_mul(100) / WORK_CHUNKS as u64;

                    klib::print!(
                        "work: Worker {} reached {percent}%; latest completed chunk was on CPU {cpu}\r\n",
                        id + 1
                    );
                }
            }

            loop {
                let (next_cursor, count_events) =
                    GLOBAL_SCHEDULER.steal_events_since(steal_cursor, &mut steal_events);

                for event in &steal_events[..count_events] {
                    if WORKERS[..count].iter().any(|worker| {
                        worker.thread_id.load(Ordering::Acquire) == u64::from(event.thread_id)
                    }) {
                        steals_observed += 1;

                        klib::print!(
                            "work: Worker {} claimed from CPU {}'s waiting queue by CPU {}\r\n",
                            WORKERS[..count]
                                .iter()
                                .position(|worker| {
                                    worker.thread_id.load(Ordering::Acquire)
                                        == u64::from(event.thread_id)
                                })
                                .unwrap_or(0)
                                + 1,
                            event.source_cpu,
                            event.destination_cpu
                        );
                    }
                }

                steal_cursor = next_cursor;
                if count_events < steal_events.len() {
                    break;
                }
            }

            if !simultaneous_reported {
                'workers: for left in 0..count {
                    let left_seq = WORKERS[left].segment_seq.load(Ordering::Acquire);
                    let left_start = WORKERS[left].segment_start.load(Ordering::Relaxed);
                    let left_end = WORKERS[left].segment_end.load(Ordering::Relaxed);
                    let left_cpu = WORKERS[left].segment_cpu.load(Ordering::Relaxed);

                    // keep relaxed field reads before validation so that the sequence check brackets them
                    fence(Ordering::Acquire);

                    if left_seq & 1 != 0
                        || WORKERS[left].segment_seq.load(Ordering::Acquire) != left_seq
                        || left_start == 0
                        || left_start >= left_end
                        || left_cpu == u64::MAX
                    {
                        continue;
                    }

                    for right in left + 1..count {
                        let right_seq = WORKERS[right].segment_seq.load(Ordering::Acquire);
                        let right_start = WORKERS[right].segment_start.load(Ordering::Relaxed);
                        let right_end = WORKERS[right].segment_end.load(Ordering::Relaxed);
                        let right_cpu = WORKERS[right].segment_cpu.load(Ordering::Relaxed);

                        fence(Ordering::Acquire);

                        if right_seq & 1 == 0
                            && WORKERS[right].segment_seq.load(Ordering::Acquire) == right_seq
                            && right_start != 0
                            && right_start < right_end
                            && right_cpu != u64::MAX
                            && left_cpu != right_cpu
                            && left_start.max(right_start) < left_end.min(right_end)
                        {
                            simultaneous_reported = true;

                            let overlap = left_end.min(right_end) - left_start.max(right_start);
                            let frequency = hal::timer::frequency_hz();

                            if frequency != 0 {
                                let overlap_ns =
                                    (u128::from(overlap) * 1_000_000_000) / u128::from(frequency);

                                klib::print!(
                                    "work: confirmed overlapping compute: Worker {} on CPU {}, Worker {} on CPU {} ({}.{:03} us)\r\n",
                                    left + 1,
                                    left_cpu,
                                    right + 1,
                                    right_cpu,
                                    overlap_ns / 1_000,
                                    overlap_ns % 1_000
                                );
                            } else {
                                klib::print!(
                                    "work: confirmed overlapping compute: Worker {} on CPU {}, Worker {} on CPU {} ({overlap} timer ticks; timer frequency unavailable)\r\n",
                                    left + 1,
                                    left_cpu,
                                    right + 1,
                                    right_cpu
                                );
                            }

                            break 'workers;
                        }
                    }
                }
            }
        }

        if completed == count {
            klib::scheduler::Scheduler::yield_now();
            break;
        }

        if polls >= WORK_MAX_POLLS
            || (timeout_ticks != 0 && hal::timer::counter().wrapping_sub(start) >= timeout_ticks)
        {
            klib::print!(
                "work: timeout after {completed}/{count} workers; trial remains active; rerun after all workers finish\r\n"
            );

            return;
        }

        polls += 1;
        klib::scheduler::Scheduler::yield_now();
    }

    WORK_TRIAL_ACTIVE.store(false, Ordering::Release);
    for (id, worker) in WORKERS[..count].iter().enumerate() {
        let initial = worker.initial_cpu.load(Ordering::Acquire);
        let final_cpu = worker.final_cpu.load(Ordering::Acquire);

        klib::print!(
            "work: Worker {} finished computation; started on CPU {initial}, finished on CPU {final_cpu}\r\n",
            id + 1
        );
    }

    klib::print!(
        "work: completed computation for {count}/{count} workers; observed {steals_observed} queue steals; parallel compute overlap {}\r\n",
        if simultaneous_reported {
            "confirmed"
        } else {
            "not observed"
        }
    );
}

extern "C" fn work_worker(id: usize) -> ! {
    let record = &WORKERS[id];
    let initial = klib::cpu_interface::CpuIdLogical::current().to_u32();

    record
        .initial_cpu
        .store(u64::from(initial), Ordering::Release);

    let mut checksum = id as u64;
    for _ in 0..WORK_CHUNKS {
        {
            let _interrupts = hal::interrupt::InterruptGuard::new();
            let cpu = klib::cpu_interface::CpuIdLogical::current().to_u32();
            let start = hal::timer::ordered_counter();
            for value in 0..WORK_ITERATIONS {
                checksum = checksum
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(value as u64 + 1);
            }

            core::hint::black_box(checksum);

            let end = hal::timer::ordered_counter();

            record.segment_seq.fetch_add(1, Ordering::AcqRel);
            record.segment_cpu.store(u64::from(cpu), Ordering::Relaxed);
            record.segment_start.store(start, Ordering::Relaxed);
            record.segment_end.store(end, Ordering::Relaxed);
            record.segment_seq.fetch_add(1, Ordering::Release);
        }

        WORKERS[id].progress.fetch_add(1, Ordering::Release);
        klib::scheduler::Scheduler::yield_now();
    }

    let final_cpu = klib::cpu_interface::CpuIdLogical::current().to_u32();

    record
        .final_cpu
        .store(u64::from(final_cpu), Ordering::Release);
    record.progress.store(WORK_CHUNKS as u64, Ordering::Release);
    record.done.store(true, Ordering::Release);

    klib::scheduler::Scheduler::exit_current()
}

fn write(bytes: &[u8]) {
    let text = core::str::from_utf8(bytes).expect("shell output is ascii");
    klib::print!("{text}");
}
