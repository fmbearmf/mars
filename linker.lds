ENTRY(_start)

KERNEL_LOAD_PHYS_RAW = 0x42000000;
KERNEL_LOAD_VIRT_RAW = 0xFFFF800000000000;
KERNEL_OFFSET = KERNEL_LOAD_VIRT_RAW - KERNEL_LOAD_PHYS_RAW;

SECTIONS {
    . = KERNEL_LOAD_VIRT_RAW;

    .text . : AT( . - KERNEL_OFFSET) ALIGN(16K)
    {
        . = ALIGN(16K);
        *(.text .text.*)
    }

    .rodata . : AT( . - KERNEL_OFFSET) ALIGN(16K)
    {
        . = ALIGN(16K);
        *(.rodata .rodata.*)
    }

    .data . : AT( . - KERNEL_OFFSET) ALIGN(16K)
    {
        . = ALIGN(16K);
        *(.data .data.*)
    }

    .bss . (NOLOAD): AT( . - KERNEL_OFFSET) ALIGN(16K)
    {
        . = ALIGN(16K);
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16K);
        __bss_end = .;
    }

    .eh_frame . : AT( . - KERNEL_OFFSET)
    {
        *(.eh_frame*)
    }

    . = ALIGN(16K);
    __boot_stack_bottom = .;
    . += 0x8000; /* 32k */
    __boot_stack_top = .;

    .reclaimable . (NOLOAD): AT( . - KERNEL_OFFSET) ALIGN(16K)
    {
        . = ALIGN(16K);
        __reclaimable_start = .;
        *(.reclaimable .reclaimable.*)
        . = ALIGN(16K);
        __reclaimable_end = .;
    }

    kend = .;
}
