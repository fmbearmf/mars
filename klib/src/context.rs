use core::{borrow::Borrow, fmt, ops::Deref};

#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct RegisterFile {
    pub registers: [u64; 31],
    pub sp: u64,
    pub elr: usize,
    pub spsr: u64,
}

impl fmt::Debug for RegisterFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct HexSlice<'a>(&'a [u64]);

        impl fmt::Debug for HexSlice<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut list = f.debug_list();
                for v in self.0 {
                    list.entry(&format_args!("{:#x}", v));
                }
                list.finish()
            }
        }

        f.debug_struct("RegisterFile")
            .field("registers", &HexSlice(&self.registers))
            .field("sp", &format_args!("{:#x}", self.sp))
            .field("elr", &format_args!("{:#x}", self.elr))
            .field("spsr", &format_args!("{:#x}", self.spsr))
            .finish()
    }
}

//const _: () = assert!(size_of::<RegisterFile>() == 8 * 24);

#[derive(Eq, PartialEq)]
#[repr(transparent)]
pub struct RegisterFileRef<'a>(pub &'a mut RegisterFile);

impl fmt::Debug for RegisterFileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl RegisterFileRef<'_> {
    pub unsafe fn get_mut(&mut self) -> &mut RegisterFile {
        self.0
    }
}

impl AsRef<RegisterFile> for RegisterFileRef<'_> {
    fn as_ref(&self) -> &RegisterFile {
        self.0
    }
}

impl Borrow<RegisterFile> for RegisterFileRef<'_> {
    fn borrow(&self) -> &RegisterFile {
        self.0
    }
}

impl Deref for RegisterFileRef<'_> {
    type Target = RegisterFile;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}
