use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec,
    vec::Vec,
};

use crate::device::Provider;
use crate::graph::{Graph, Layer};
use crate::{
    Completion, IoError,
    bio::{Bio, BioFlags, BioOp, SubmitError},
    buffer::{BufferAccess, BufferId, BufferLease, IoBuffer},
    device::{Channel, ChannelRequest, Device, DeviceInfo},
};

type TestBuffer = Mutex<Vec<u8>>;
static BUFFERS: Mutex<Vec<Weak<TestBuffer>>> = Mutex::new(Vec::new());

fn buffer(data: Vec<u8>) -> (IoBuffer, BufferLease) {
    let len = data.len();
    let owner = Arc::new(Mutex::new(data));
    let mut buffers = BUFFERS.lock().unwrap();

    let id = BufferId {
        slot: u32::try_from(buffers.len()).unwrap(),
        generation: 0,
    };

    buffers.push(Arc::downgrade(&owner));

    (
        IoBuffer {
            id,
            offset: 0,
            len,
            access: BufferAccess::ReadWrite,
        },
        BufferLease::new(id, owner),
    )
}

fn backing(buf: IoBuffer) -> Arc<TestBuffer> {
    assert_eq!(buf.id.generation, 0);

    BUFFERS.lock().unwrap()[buf.id.slot as usize]
        .upgrade()
        .expect("buffer lease must keep backing memroy alive")
}

struct Slice {
    pub start: u64,
    pub blocks: u64,
}

struct SliceDevice {
    parent: Arc<Provider>,
    info: DeviceInfo,
    offset: u64,
}

impl Layer for Slice {
    fn create(&self, inputs: &[Arc<Provider>]) -> Result<Arc<dyn Device>, IoError> {
        if inputs.len() != 1 {
            return Err(IoError::InvalidTopology);
        }

        let parent = Arc::clone(&inputs[0]);
        let parent_info = parent.info();

        let end = self
            .start
            .checked_add(self.blocks)
            .ok_or(IoError::OutOfBounds)?;

        if end > parent_info.block_count || self.blocks == 0 {
            return Err(IoError::OutOfBounds);
        }

        Ok(Arc::new(SliceDevice {
            parent,
            offset: self.start,
            info: DeviceInfo {
                block_count: self.blocks,
                ..parent_info
            },
        }))
    }
}

impl Device for SliceDevice {
    fn info(&self) -> DeviceInfo {
        self.info
    }

    fn open_channel(&self, request: ChannelRequest) -> Result<Box<dyn Channel>, IoError> {
        let inner = self.parent.open_channel(request)?;

        Ok(Box::new(SliceChannel {
            inner,
            offset: self.offset,
            info: self.info,
        }))
    }
}

struct SliceChannel {
    inner: Box<dyn Channel>,
    offset: u64,
    info: DeviceInfo,
}

impl Channel for SliceChannel {
    fn submit(&mut self, mut bio: Bio) -> Result<(), SubmitError> {
        if let Err(e) = bio.validate(self.info) {
            return Err(SubmitError::Failed(e, bio));
        }

        if bio.op == BioOp::Flush {
            return self.inner.submit(bio);
        }

        let original_lba = bio.lba;

        bio.lba = match original_lba.checked_add(self.offset) {
            Some(lba) => lba,
            None => {
                return Err(SubmitError::Failed(IoError::OutOfBounds, bio));
            }
        };

        match self.inner.submit(bio) {
            Ok(()) => Ok(()),
            Err(SubmitError::Full(mut bio)) => {
                bio.lba = original_lba;
                Err(SubmitError::Full(bio))
            }
            Err(SubmitError::Failed(error, mut bio)) => {
                bio.lba = original_lba;
                Err(SubmitError::Failed(error, bio))
            }
        }
    }

    fn poll(&mut self, budget: usize) -> usize {
        self.inner.poll(budget)
    }
}

struct RamDisk {
    data: Arc<Mutex<Vec<u8>>>,
    info: DeviceInfo,
}

impl RamDisk {
    fn new(blocks: u64, block_size: u32) -> Self {
        Self {
            data: Arc::new(Mutex::new(vec![0; blocks as usize * block_size as usize])),
            info: DeviceInfo::simple(block_size, blocks),
        }
    }
}

struct RamChannel {
    data: Arc<Mutex<Vec<u8>>>,
    info: DeviceInfo,
}

impl Device for RamDisk {
    fn info(&self) -> DeviceInfo {
        self.info
    }

    fn open_channel(
        &self,
        _request: ChannelRequest,
    ) -> Result<alloc::boxed::Box<dyn Channel>, IoError> {
        Ok(Box::new(RamChannel {
            data: Arc::clone(&self.data),
            info: self.info,
        }))
    }
}

impl Channel for RamChannel {
    fn submit(&mut self, bio: Bio) -> Result<(), SubmitError> {
        if let Err(e) = bio.validate(self.info) {
            return Err(SubmitError::Failed(e, bio));
        }

        if bio.op == BioOp::Flush {
            bio.complete(Ok(()));
            return Ok(());
        }

        let offset = bio.lba as usize * self.info.block_size as usize;
        let length = bio.blocks as usize * self.info.block_size as usize;

        let descriptor = bio.data.unwrap();
        let owner = backing(descriptor);
        let mut bytes = owner.lock().unwrap();
        let start = usize::try_from(descriptor.offset).unwrap();
        let end = start.checked_add(length).unwrap();

        let buffer = &mut bytes[start..end];
        let mut storage = self.data.lock().unwrap();

        match bio.op {
            BioOp::Read => {
                buffer.copy_from_slice(&storage[offset..offset + length]);
            }
            BioOp::Write => storage[offset..offset + length].copy_from_slice(buffer),
            BioOp::Flush | BioOp::Discard | BioOp::WriteZeroes => unreachable!(),
        }

        drop(storage);
        drop(bytes);

        bio.complete(Ok(()));

        Ok(())
    }
}

fn bio(
    op: BioOp,
    lba: u64,
    blocks: u32,
    buffer: Option<(IoBuffer, BufferLease)>,
    completed: Arc<AtomicUsize>,
) -> Bio {
    let (data, lease) = match buffer {
        Some((data, lease)) => (Some(data), Some(lease)),
        None => (None, None),
    };

    Bio {
        op,
        lba,
        blocks,
        data,
        lease,
        flags: BioFlags::default(),
        completion: Completion::new(move |result, _buffer| {
            assert_eq!(result, Ok(()));
            completed.fetch_add(1, Ordering::SeqCst);
        }),
    }
}

#[test]
fn read_write_through_slice() {
    let mut graph = Graph::new();

    let disk = graph.add_device(Arc::new(RamDisk::new(128, 512))).unwrap();

    let partition = graph
        .stack(
            &Slice {
                start: 16,
                blocks: 32,
            },
            &[disk],
        )
        .unwrap();

    let completed = Arc::new(AtomicUsize::new(0));

    let mut ch = partition.open_channel(ChannelRequest::default()).unwrap();

    ch.submit(bio(
        BioOp::Write,
        0,
        1,
        Some(buffer(vec![0xAB; 512])),
        Arc::clone(&completed),
    ))
    .unwrap_or_else(|_| panic!("write submit failed"));

    let read = buffer(vec![0; 512]);
    let read_bytes = backing(read.0);

    ch.submit(bio(BioOp::Read, 0, 1, Some(read), Arc::clone(&completed)))
        .unwrap_or_else(|_| panic!("read submit failed"));

    assert_eq!(read_bytes.lock().unwrap().as_slice(), &[0xAB; 512]);
    assert_eq!(completed.load(Ordering::SeqCst), 2);
    assert_eq!(partition.in_flight(), 0);
}

#[test]
fn nested_slices_do_translate_lbas() {
    let mut graph = Graph::new();

    let disk = Arc::new(RamDisk::new(128, 512));
    let storage = Arc::clone(&disk.data);

    let root = graph.add_device(disk).unwrap();

    let first = graph
        .stack(
            &Slice {
                start: 10,
                blocks: 80,
            },
            &[root],
        )
        .unwrap();

    let second = graph
        .stack(
            &Slice {
                start: 7,
                blocks: 20,
            },
            &[first],
        )
        .unwrap();

    let completed = Arc::new(AtomicUsize::new(0));
    let mut channel = second.open_channel(ChannelRequest::default()).unwrap();

    channel
        .submit(bio(
            BioOp::Write,
            3,
            1,
            Some(buffer(vec![0x5A; 512])),
            Arc::clone(&completed),
        ))
        .unwrap_or_else(|_| panic!("submission failed"));

    // 10 + 7 + 3 is like 20 ish
    let bytes = storage.lock().unwrap();
    assert_eq!(bytes[20 * 512], 0x5A);
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}
