use core::ops::Add;
use hax_lib::{
    Refinement, assume, attributes, ensures, exclude, opaque, refinement_type, requires,
};

const ADDR_SIZE: u64 = 8;

// make sure that's actually the size of a memory address
#[exclude]
const _: () = {
    assert!(ADDR_SIZE as usize == core::mem::size_of::<usize>());
    assert!(ADDR_SIZE as usize == core::mem::size_of::<u64>());
};

#[refinement_type(|x| x != 0 && x % ADDR_SIZE == 0)]
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Addr(u64);

// hax scoping issue
mod addr_impl {
    use hax_lib::{RefineAs, assume};

    use super::{ADDR_SIZE, Addr, Refinement, attributes, ensures, requires};

    const _: u64 = ADDR_SIZE;

    #[cfg(not(hax))]
    impl Refinement for Addr {
        type InnerType = u64;

        fn new(x: Self::InnerType) -> Self {
            Self(x)
        }

        fn get(self) -> Self::InnerType {
            self.0
        }

        fn get_mut(&mut self) -> &mut Self::InnerType {
            &mut self.0
        }

        fn invariant(value: Self::InnerType) -> hax_lib::Prop {
            unimplemented!()
        }
    }

    #[cfg(not(hax))]
    impl RefineAs<Addr> for u64 {
        fn into_checked(self) -> Addr {
            use hax_lib::Refinement;
            Addr::new(self)
        }
    }

    #[attributes]
    impl Addr {
        #[requires(offset <= u64::MAX - self.get() && offset % ADDR_SIZE == 0)]
        #[ensures(|result| result.get() % ADDR_SIZE == 0 && result.get() != 0)]
        pub fn offset(self, offset: u64) -> Self {
            use hax_lib::RefineAs;

            let value = self.get() + offset;

            assert!(value > 0);
            assert!(self.get() % ADDR_SIZE == 0);
            assert!(offset % ADDR_SIZE == 0);
            assert!(value % ADDR_SIZE == 0);

            value.into_checked()
        }
    }
}
