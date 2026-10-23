OUTPUT_FORMAT("elf64-littleaarch64")
ENTRY(_start)
PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}
SECTIONS
{
    . = 0x1000;
    .text ALIGN(0x1000) : { *(.text) *(.text.*) } :text
    .rodata ALIGN(0x1000) : { *(.rodata) *(.rodata.*) } :rodata
    .data ALIGN(0x1000) : { *(.data) *(.data.*) } :data
    .bss ALIGN(0x1000) : {
        __alpha_environment = .;
        . += 0x10000;
        __alpha_environment_end = .;
        __ipc_buffer_start = .;
        . += 0x1000;
        __alpha_stack_start = .;
        . += 0x200000;
        __alpha_stack_end = .;
        *(.bss) *(.bss.*)
    } :data
    /DISCARD/ : { *(.eh_frame) *(.eh_frame.*) }
}
