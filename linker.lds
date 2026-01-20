ENTRY(_start)

KERNEL_LOAD_PHYS_RAW = 0x42000000;
KERNEL_LOAD_VIRT_RAW = 0xFFFF800000000000;
KERNEL_OFFSET = KERNEL_LOAD_VIRT_RAW - KERNEL_LOAD_PHYS_RAW;

SECTIONS {
    . = KERNEL_LOAD_VIRT_RAW;

    /*.boot : AT( . - KERNEL_OFFSET)
    {
        *(.boot)
        *(.boot.*)
        . = ALIGN(64K);
    }*/

    kboot_end = .;

    .text . : AT( . - KERNEL_OFFSET) ALIGN(64K)
    {
        . = ALIGN(64K);
        *(.text .text.*)
    }

    .rodata . : AT( . - KERNEL_OFFSET) ALIGN(64K)
    {
        . = ALIGN(64K);
        *(.rodata .rodata.*)
    }

    .data . : AT( . - KERNEL_OFFSET) ALIGN(64K)
    {
        . = ALIGN(64K);
        *(.data .data.*)
    }

    .bss . (NOLOAD): AT( . - KERNEL_OFFSET) ALIGN(64K)
    {
        . = ALIGN(64K);
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

    . = ALIGN(64K);
    . += 0x8000; /* 32k */
    __boot_stack_top = .;
    kend = .;
}
