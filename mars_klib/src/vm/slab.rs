use core::mem;

use crate::vm::PAGE_SIZE;

const MIN_OBJ_SIZE: usize = mem::size_of::<usize>();

const fn build_class_sizes() -> [usize; 9] {
    [
        usize::pow(2, 3),
        usize::pow(2, 4),
        usize::pow(2, 5),
        usize::pow(2, 6),
        usize::pow(2, 7),
        usize::pow(2, 8),
        usize::pow(2, 9),
        usize::pow(2, 10),
        usize::pow(2, 11),
    ]
}

const CLASS_SIZES: [usize; 9] = build_class_sizes();
const NUM_CLASSES: usize = CLASS_SIZES.len();

// 1 usize for key, 1 usize for value
const BUCKET_COUNT: usize = PAGE_SIZE / (size_of::<usize>() * 2);
