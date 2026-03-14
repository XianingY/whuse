    .section .text.entry
    .globl _start
_start:
    la sp, boot_stack_top
    call rust_main
1:
    wfi
    j 1b

    .section .bss.stack
    .align 12
boot_stack:
    .space 4096 * 4
boot_stack_top:
