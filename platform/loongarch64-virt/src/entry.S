    .section .text.entry
    .globl _start
_start:
    # Emit an early byte so we can distinguish "reset path broken" from
    # "Rust/kernel logging broken".
    li.d $t0, 0x1fe001e0
    li.w $t1, 0x57
    st.b $t1, $t0, 0

    la.local $t0, _bss_start
    la.local $t1, _bss_end
    move $t2, $zero
1:
    beq $t0, $t1, 2f
    st.d $t2, $t0, 0
    addi.d $t0, $t0, 8
    b 1b
2:
    la.local $sp, boot_stack_top
    bl rust_main
3:
    idle 0
    b 3b

    .section .bss.stack
    .align 12
boot_stack:
    .space 4096 * 4
boot_stack_top:
