OUTPUT_ARCH(aarch64)

PHDRS
{
    text PT_LOAD;
    rodata PT_LOAD FLAGS (4);
    data PT_LOAD;
}

__ALIGN = 0x2000000;
__KBASE = 0xFFFF800000000000;

ENTRY(_start)

SECTIONS {
    . = __KBASE;

    .text : ALIGN(16K)
    {
        *(.text .text.*)
        . = ALIGN(16K);
    }

    .rodata : ALIGN(16K)
    {
        *(.rodata .rodata.*)
        . = ALIGN(16K);
    }

    .data : ALIGN(16K)
    {
        *(.data .data.*)
        . = ALIGN(16K);
    }

    .bss (NOLOAD) : ALIGN(16K)
    {
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16K);
    }

    .eh_frame : ALIGN(16K)
    {
        *(.eh_frame*)
        . = ALIGN(16K);
    }

    .got : ALIGN(16K)
    {
        *(.got*)
        . = ALIGN(16K);
    }

    . = ALIGN(16K);
    __boot_stack_bottom = .;
    . += 0x8000; /* 32k */
    __boot_stack_top = .;

    .reclaimable (NOLOAD) : ALIGN(16K)
    {
        __reclaimable_start = .;
        *(.reclaimable .reclaimable.*)
        __pt_pool_start = .;
        . += 0x100000; /* 1mib */
        __pt_pool_end = .;
        . = ALIGN(16K);
        __reclaimable_end = .;
    }

    __kend = .;
}
