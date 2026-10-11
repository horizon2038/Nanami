OUTPUT_FORMAT("elf64-littleaarch64")

ENTRY(_start)

SECTIONS
{
    . = 0x1000;

    .text ALIGN(0x1000) :
    {
        *(.text);
        *(.text.*)
    }

    .rodata ALIGN(0x1000) :
    {
        *(.rodata);
        *(.rodata.*);
    }

    .data ALIGN(0x1000) :
    {
        *(.data);
        *(.data.*);
    }

    .bss ALIGN(0x1000) :
    {
        __ipc_buffer_start = .;
        . += 0x1000;
        __ipc_buffer_end = .;

        *(.bss);
        *(.bss.*);
    }

    /DISCARD/ :
    {
        *(*.eh_frame);
    }
}
