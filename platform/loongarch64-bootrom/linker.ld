OUTPUT_ARCH(loongarch64)
ENTRY(_start)

MEMORY
{
  ROM : ORIGIN = 0x1c000000, LENGTH = 2M
}

SECTIONS
{
  . = ORIGIN(ROM);

  .text : ALIGN(4K) {
    KEEP(*(.text.entry))
    *(.text .text.*)
  } > ROM

  .rodata : ALIGN(4K) {
    *(.rodata .rodata.*)
  } > ROM

  .data : ALIGN(4K) {
    *(.data .data.*)
  } > ROM

  .bss : ALIGN(4K) {
    *(.bss .bss.*)
    *(COMMON)
  } > ROM

  /DISCARD/ : {
    *(.eh_frame)
    *(.eh_frame_hdr)
  }
}
