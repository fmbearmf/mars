use core::mem::MaybeUninit;

use mars_models::{
    declare_register,
    memory::registers::volatile::{PureReadable, RPureReadWrite, RReadWrite, Writeable},
};
use zerocopy::FromBytes;

declare_register!(GicdCtlr, u32, {
    field EnableLPIs => (
        offset: 0,
        size: 1,
        type: bool,
    );
    field RegisterWritePending => (
        offset: 3,
        size: 1,
        type: bool,
    );
});
