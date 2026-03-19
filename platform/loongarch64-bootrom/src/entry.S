    .section .text.entry
    .globl _start
_start:
    li.d $t0, 0x1fe001e0
    li.w $t1, 0x42
    st.b $t1, $t0, 0

    li.d $t0, 0x90000000
    jirl $zero, $t0, 0

1:
    idle 0
    b 1b
