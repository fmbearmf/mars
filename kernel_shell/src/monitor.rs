#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CommandRequest {
    Uptime,
    Cpu,
    Memory,
    Sched,
    Cpus,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Snapshot {
    Uptime {
        seconds: u64,
    },
    Cpu {
        logical_id: u32,
    },
    Memory {
        heap_used: usize,
        page_used: usize,
        capacity: usize,
    },
    Sched {
        ready: usize,
        injector: usize,
    },
    CpuState {
        logical_id: u32,
        state: CpuState,
        thread: Option<ThreadMetadata>,
    },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CpuState {
    NotReady,
    Busy,
    Idle,
    Active,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ThreadMetadata {
    pub id: u32,
    pub state: ThreadState,
    pub kind: ThreadKind,
    pub idle: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ThreadState {
    Running,
    Ready,
    Blocked,
    Dead,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ThreadKind {
    Kernel,
    User,
}

pub trait Telemetry {
    fn snapshot(&mut self, request: CommandRequest, index: usize) -> Option<Snapshot>;
}
