    .section .text.entry
    .globl _start
_start:
    # Emit an early byte so we can distinguish "reset path broken" from
    # "Rust/kernel logging broken".
    li.d $t0, 0x1fe001e0
    li.w $t1, 0x57
    st.b $t1, $t0, 0

    # Configure Direct Mapped Windows (DMW) for kernel memory access.
    # DMW setup can be done early, before paging is enabled.
    #
    # DMW0 (CSR 0x180): uncached direct mapping for 0x8xxx_xxxx_xxxx_xxxx
    #   VSEG=0x8, MAT=0 (strongly-ordered uncached), PLV0=1
    #   Value: 0x8000_0000_0000_0001
    li.d $t0, 0x8000000000000001
    csrwr $t0, 0x180

    # DMW1 (CSR 0x181): cached direct mapping for 0x9xxx_xxxx_xxxx_xxxx
    #   VSEG=0x9, MAT=1 (coherent cached), PLV0=1
    #   Value: 0x9000_0000_0000_0011
    li.d $t0, 0x9000000000000011
    csrwr $t0, 0x181

    # DMW2 (CSR 0x182): weakly-ordered uncached for 0xaxxx_xxxx_xxxx_xxxx
    #   VSEG=0xa, MAT=2 (weakly-ordered uncached), PLV0=1
    #   Value: 0xa000_0000_0000_0021
    li.d $t0, 0xa000000000000021
    csrwr $t0, 0x182

    # DMW3 (CSR 0x183): disabled
    li.d $t0, 0x0000000000000000
    csrwr $t0, 0x183

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
    .space 4096 * 32
boot_stack_top:
