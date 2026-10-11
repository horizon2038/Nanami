OUTPUT_FORMAT("elf64-x86-64")
ENTRY(_start)

SECTIONS
{
    . = 0x01000000;

    .text : ALIGN(0x1000)
    {
        *(.text)
        *(.text.*)
    }

    .rodata : ALIGN(0x1000)
    {
        *(.rodata)
        *(.rodata.*)
    }

    .data : ALIGN(0x1000)
    {
        *(.data)
        *(.data.*)
    }

    .bss : ALIGN(0x1000)
    {
        __bss_start = .;
        *(.bss)
        *(.bss.*)
        *(COMMON)

        . = ALIGN(0x1000);
        __stack_start = .;
        . += 0x40000;
        __stack_end = .;

        . = ALIGN(0x1000);
        __ipc_buffer_start = .;
        . += 0x1000;
        __ipc_buffer_end = .;

        . = ALIGN(0x1000);
        __heap_start = .;
        __heap_end = .;

        __bss_end = .;
    }
}
