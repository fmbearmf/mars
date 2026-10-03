use core::marker::PhantomData;

/// unfinalized
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Draft;

/// bound to host memory and validated against constraints
#[derive(Debug)]
pub struct Bound<'a> {
    pub(crate) _marker: PhantomData<&'a mut ()>,
}

/// placed into SQ
#[derive(Debug)]
pub struct Submitted<'a> {
    pub(crate) _marker: PhantomData<&'a mut ()>,
}

/// processed by controller. buffer lifetime is up
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Completed;
