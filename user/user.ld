ENTRY(_start)

SECTIONS {
    . = 0x400000;

    .text : ALIGN(4K) {
        *(.text._start)
        *(.text .text.*)
    }

    . = ALIGN(4K);
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    . = ALIGN(4K);
    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    . = ALIGN(4K);
    .bss : ALIGN(4K) {
        *(.bss .bss.*)
    }

    /DISCARD/ : {
        *(.eh_frame)
        *(.note .note.*)
    }
}
