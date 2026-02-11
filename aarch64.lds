OUTPUT_ARCH(aarch64)

FLAG_R = 4;
FLAG_W = 2;
FLAG_X = 1;

PHDRS
{
    text PT_LOAD FLAGS(4 | 1);
    rodata PT_LOAD FLAGS (4);
    data PT_LOAD FLAGS(4 | 2);
}

__ALIGN = 0x4000;
__KBASE = 0xFFFF800000000000;

ENTRY(_start)

SECTIONS {
    . = __KBASE;

    PROVIDE(__KBASE = __KBASE);

    .text : ALIGN(__ALIGN)
    {
        *(.text .text.*)
    } :text

    . = ALIGN(__ALIGN);

    .rodata : ALIGN(__ALIGN)
    {
        *(.rodata .rodata.*)
    } :rodata

    .eh_frame :
    {
        *(.eh_frame*)
    } :rodata

    . = ALIGN(__ALIGN);

    .data : ALIGN(__ALIGN)
    {
        *(.data .data.*)
    } :data

    .bss (NOLOAD) :
    {
        *(.bss .bss.*)
        *(COMMON)
    } :data

    .reclaimable (NOLOAD) :
    {
        __reclaimable_start = .;
        *(.reclaimable .reclaimable.*)
        __reclaimable_end = .;
    } :data

    . = ALIGN(__ALIGN);
    __KEND = .;
}
