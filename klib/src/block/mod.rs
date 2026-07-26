/// errors that can occur during block I/O
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BlockError {
    /// requested op goes past device capacity
    OutOfBounds,
    /// buffer len isn't a multiple of the block size
    UnalignedBuffer,
    HardwareError,
    /// tried to write to a read-only device
    ReadOnly,
    /// device isn't ready
    NotReady,
}

pub type Result<T> = core::result::Result<T, BlockError>;

pub trait BlockDevice {
    /// flush any caches to the backing device.
    fn flush(&mut self) -> self::Result<()> {
        Ok(())
    }
    /// read >= 1 blocks starting from a given logical block address.
    fn read_blocks(&mut self, lba: u64, buf: &mut [u8]) -> self::Result<()>;
    /// write >= 1 blocks starting from a given logical block address.
    fn write_blocks(&mut self, lba: u64, buf: &[u8]) -> Result<()>;
    /// block size, in bytes.
    fn block_size(&self) -> usize;
    /// total number of accessible blocks on the device
    fn block_count(&self) -> u64;
}
