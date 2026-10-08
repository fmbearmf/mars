use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kernel_shell::{
    CommandRequest, CpuState, OUTPUT_CAPACITY, PROMPT, Shell, Snapshot, Telemetry, ThreadKind,
    ThreadMetadata, ThreadState,
};
use klib::{scheduler::GLOBAL_SCHEDULER, stack::Stack, thread::Thread};

use super::earlycon::earlycon_read_byte;

static STARTED: AtomicBool = AtomicBool::new(false);
static UPTIME_START: AtomicU64 = AtomicU64::new(0);

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

fn write(bytes: &[u8]) {
    let text = core::str::from_utf8(bytes).expect("shell output is ascii");
    klib::print!("{text}");
}
