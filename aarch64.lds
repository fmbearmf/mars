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

    PROVIDE(__KBASE = __KBASE);

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

    .reclaimable (NOLOAD) : ALIGN(16K)
    {
        __reclaimable_start = .;
        *(.reclaimable .reclaimable.*)
        . = ALIGN(16K);
        __reclaimable_end = .;
    }

    . = ALIGN(16K);
    __KEND = .;
}
