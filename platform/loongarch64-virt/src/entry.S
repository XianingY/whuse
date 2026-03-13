    .section .text.entry
    .globl _start
_start:
    la.local $sp, boot_stack_top
    move $a0, $zero
    move $a1, $zero
    bl rust_main
1:
    idle 0
    b 1b

    .section .bss.stack
    .align 12
boot_stack:
    .space 4096 * 4
boot_stack_top:
