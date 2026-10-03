use alloc::{sync::Arc, vec};
use klib::{
    block::{
        BlockError, Command, IoRequest,
        registry::{ProviderHandle, attach},
    },
    scheduler::GLOBAL_SCHEDULER,
    sync::SleepingMutex,
};

use crate::controller::Controller;

pub(crate) fn run(
    handle: &ProviderHandle,
    controller: &Arc<SleepingMutex<'static, Controller>>,
    nsid: u32,
) {
    if !controller.lock(&GLOBAL_SCHEDULER).is_test_device() {
        return;
    }
    let command_result = { controller.lock(&GLOBAL_SCHEDULER).test_invalid_opcode(nsid) };
    let result = command_result
        .map_err(|_| "command-error handling failed")
        .and_then(|()| exercise(handle))
        .and_then(|()| {
            let mut controller = controller.lock(&GLOBAL_SCHEDULER);
            if controller.is_timeout_test() {
                controller
                    .test_timeout()
                    .map_err(|_| "timeout/reset handling failed")
            } else {
                controller.shutdown().map_err(|_| "shutdown failed")
            }
        });
    match result {
        Ok(()) => {
            log::info!("NVME SELF-TEST PASS");
            // something weird to avoid false positives
            exit(42);
        }
        Err(error) => {
            log::error!("NVME SELF-TEST FAIL: {error}");
            exit(1);
        }
    }
}

fn exercise(handle: &ProviderHandle) -> Result<(), &'static str> {
    let mut disk = attach(handle);
    disk.access(1, 1, 1).map_err(|_| "open failed")?;
    let bs = disk.block_size();
    let capacity = disk.block_count();
    let mut first = vec![0u8; bs];
    disk.request(IoRequest {
        lba: 0,
        cmd: Command::Read { buf: &mut first },
    })
    .map_err(|_| "initial read failed")?;
    if !first.starts_with(b"MARS_NVME_TEST_IMAGE") {
        return Err("not the disposable test image");
    }

    // unaligned CPU buffers, both PRP forms, PRP lists, transfer splitting and CQ phase wraps
    for round in 0..130usize {
        let len = [bs, 4096, 8192, 16384, 40960][round % 5].max(bs);
        let lba = 16 + ((round % 8) * (40960 / bs)) as u64;
        let mut expected = vec![0u8; len + 3];
        for (index, byte) in expected[3..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_add(round as u8) ^ 0xa5;
        }
        disk.request(IoRequest {
            lba,
            cmd: Command::Write {
                buf: &expected[3..],
            },
        })
        .map_err(|_| "write failed")?;
        disk.request(IoRequest {
            lba: 0,
            cmd: Command::Flush,
        })
        .map_err(|_| "flush failed")?;
        let mut actual = vec![0u8; len + 1];
        disk.request(IoRequest {
            lba,
            cmd: Command::Read {
                buf: &mut actual[1..],
            },
        })
        .map_err(|_| "readback failed")?;
        if actual[1..] != expected[3..] {
            return Err("readback mismatch");
        }
    }
    let mut empty = [];
    disk.request(IoRequest {
        lba: capacity,
        cmd: Command::Read { buf: &mut empty },
    })
    .map_err(|_| "empty end-of-device read failed")?;
    if disk.request(IoRequest {
        lba: capacity,
        cmd: Command::Read { buf: &mut first },
    }) != Err(BlockError::OutOfBounds)
    {
        return Err("bounds check failed");
    }
    let unaligned = vec![0u8; bs + 1];
    if disk.request(IoRequest {
        lba: 1,
        cmd: Command::Write { buf: &unaligned },
    }) != Err(BlockError::UnalignedBuffer)
    {
        return Err("alignment check failed");
    }
    disk.access(-1, -1, -1).map_err(|_| "close/flush failed")?;
    Ok(())
}

fn exit(status: usize) -> ! {
    // sys_exit_extended, only enabled by the runner
    unsafe { hal::debug::exit(status) }
}
