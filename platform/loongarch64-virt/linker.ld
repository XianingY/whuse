OUTPUT_ARCH(loongarch64)
ENTRY(_start)

MEMORY
{
  RAM : ORIGIN = 0x90000000, LENGTH = 128M
}

SECTIONS
{
  . = ORIGIN(RAM);

  .text : ALIGN(4K) {
    KEEP(*(.text.entry))
    *(.text .text.*)
  } > RAM

  .rodata : ALIGN(4K) {
    *(.rodata .rodata.*)
  } > RAM

  .data : ALIGN(4K) {
    *(.data .data.*)
  } > RAM

  .bss : ALIGN(4K) {
    _bss_start = .;
    *(.bss.stack)
    *(.bss .bss.*)
    *(COMMON)
    _bss_end = .;
  } > RAM

  /DISCARD/ : {
    *(.eh_frame)
    *(.eh_frame_hdr)
  }
}
