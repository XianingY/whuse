#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
use core::arch::global_asm;
use alloc::string::String;
use proc::Process;
use vfs::{KernelResult, KernelVfs};

include!(concat!(env!("OUT_DIR"), "/generated_rootfs.rs"));

pub const INIT_BANNER: &str = "whuse: init process bootstrapped\n";

pub struct BuiltinProgram {
    pub entry: usize,
    pub image: &'static [u8],
}

#[cfg(target_arch = "riscv64")]
global_asm!(
    r#"
    .section .text.whuse_user_init, "ax"
    .balign 8
    .global whuse_user_init_start
    .global whuse_user_init_entry
    .global whuse_user_init_end
whuse_user_init_start:
whuse_user_init_entry:
    addi sp, sp, -384
    li a0, 1
    la a1, init_msg
    li a2, init_msg_end - init_msg
    li a7, 64
    ecall

    li a0, 0
    li a1, 0
    li a7, 19
    ecall
    mv s1, a0
    mv a0, s1
    la a1, one64
    li a2, 8
    li a7, 64
    ecall

    li a0, 0
    li a7, 20
    ecall
    mv s2, a0
    li t0, 1
    sw t0, 0(sp)
    sw zero, 4(sp)
    mv t0, s1
    sd t0, 8(sp)
    mv a0, s2
    li a1, 1
    mv a2, s1
    mv a3, sp
    li a7, 21
    ecall
    mv a0, s2
    addi a1, sp, 16
    li a2, 1
    li a3, 0
    li a4, 0
    li a5, 0
    li a7, 22
    ecall
    mv a0, s1
    addi a1, sp, 32
    li a2, 8
    li a7, 63
    ecall
    li a0, 1
    la a1, event_msg
    li a2, event_msg_end - event_msg
    li a7, 64
    ecall

    li a0, 1
    li a1, 1
    li a2, 0
    addi a3, sp, 40
    li a7, 199
    ecall
    lw s3, 40(sp)
    lw s4, 44(sp)
    mv a0, s3
    la a1, sock_payload
    li a2, sock_payload_end - sock_payload
    li a3, 0
    li a4, 0
    li a5, 0
    li a7, 206
    ecall
    mv a0, s4
    addi a1, sp, 48
    li a2, 4
    li a3, 0
    li a4, 0
    li a5, 0
    li a7, 207
    ecall
    li a0, 1
    la a1, socket_msg
    li a2, socket_msg_end - socket_msg
    li a7, 64
    ecall

    li a7, 172
    ecall
    mv s5, a0
    li a7, 173
    ecall
    li a7, 178
    ecall
    mv a0, s5
    li a1, 10
    li a7, 129
    ecall
    addi a0, sp, 56
    li a1, 8
    li a7, 136
    ecall
    addi a0, sp, 56
    addi a1, sp, 64
    li a2, 0
    li a3, 8
    li a7, 137
    ecall
    li a0, 1
    la a1, signal_msg
    li a2, signal_msg_end - signal_msg
    li a7, 64
    ecall

    li a0, 1
    li a1, 4096
    li a2, 0
    li a7, 194
    ecall
    mv s6, a0
    mv a0, s6
    li a1, 0
    li a2, 0
    li a7, 196
    ecall
    mv s7, a0
    mv a0, s6
    li a1, 2
    addi a2, sp, 200
    li a7, 195
    ecall
    mv a0, s7
    li a7, 197
    ecall
    li a0, 1
    la a1, shm_msg
    li a2, shm_msg_end - shm_msg
    li a7, 64
    ecall

    li a7, 220
    ecall
    beqz a0, 1f
    mv s0, a0
    li a7, 124
    ecall
    li a0, -1
    addi a1, sp, 0
    li a7, 260
    ecall
    li a0, 1
    la a1, parent_msg
    li a2, parent_msg_end - parent_msg
    li a7, 64
    ecall
    li a0, 0
    li a7, 94
    ecall
1:
    la a0, child_path
    li a1, 0
    li a2, 0
    li a7, 221
    ecall
    li a0, 1
    la a1, exec_fail_msg
    li a2, exec_fail_msg_end - exec_fail_msg
    li a7, 64
    ecall
    li a0, 99
    li a7, 94
    ecall

init_msg:
    .ascii "user:init entered\n"
init_msg_end:
event_msg:
    .ascii "user:eventfd epoll ok\n"
event_msg_end:
socket_msg:
    .ascii "user:socketpair ok\n"
socket_msg_end:
signal_msg:
    .ascii "user:signal ok\n"
signal_msg_end:
shm_msg:
    .ascii "user:shm ok\n"
shm_msg_end:
parent_msg:
    .ascii "user:init wait complete\n"
parent_msg_end:
exec_fail_msg:
    .ascii "user:init execve failed\n"
exec_fail_msg_end:
child_path:
    .asciz "/bin/child"
one64:
    .dword 1
sock_payload:
    .ascii "pong"
sock_payload_end:
whuse_user_init_end:

    .section .text.whuse_user_child, "ax"
    .balign 8
    .global whuse_user_child_start
    .global whuse_user_child_entry
    .global whuse_user_child_end
whuse_user_child_start:
whuse_user_child_entry:
    li a0, 1
    la a1, child_msg
    li a2, child_msg_end - child_msg
    li a7, 64
    ecall
    li a0, 42
    li a7, 94
    ecall

child_msg:
    .ascii "user:child exec ok\n"
child_msg_end:
whuse_user_child_end:
"#
);

#[cfg(target_arch = "loongarch64")]
global_asm!(
    r#"
    .section .text.whuse_user_init, "ax"
    .balign 8
    .global whuse_user_init_start
    .global whuse_user_init_entry
    .global whuse_user_init_end
whuse_user_init_start:
whuse_user_init_entry:
    addi.d $sp, $sp, -32
    ori $a0, $zero, 1
    la.local $a1, init_msg
    la.local $a2, init_msg_end
    sub.d $a2, $a2, $a1
    ori $a7, $zero, 64
    syscall 0
    ori $a0, $zero, 0
    ori $a7, $zero, 93
    syscall 0
init_msg:
    .ascii "user:init entered (loongarch)\n"
init_msg_end:
whuse_user_init_end:

    .section .text.whuse_user_child, "ax"
    .balign 8
    .global whuse_user_child_start
    .global whuse_user_child_entry
    .global whuse_user_child_end
whuse_user_child_start:
whuse_user_child_entry:
    ori $a0, $zero, 1
    la.local $a1, child_msg
    la.local $a2, child_msg_end
    sub.d $a2, $a2, $a1
    ori $a7, $zero, 64
    syscall 0
    ori $a0, $zero, 42
    ori $a7, $zero, 93
    syscall 0
child_msg:
    .ascii "user:child exec ok (loongarch)\n"
child_msg_end:
whuse_user_child_end:
"#
);

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
unsafe extern "C" {
    static whuse_user_init_start: u8;
    static whuse_user_init_entry: u8;
    static whuse_user_init_end: u8;
    static whuse_user_child_start: u8;
    static whuse_user_child_entry: u8;
    static whuse_user_child_end: u8;
}

pub fn seed_filesystem(vfs: &mut KernelVfs) -> KernelResult<()> {
    for (path, data) in ROOTFS_ENTRIES {
        if let Some(parent) = path.rsplit_once('/') {
            if !parent.0.is_empty() {
                create_dir_chain(vfs, parent.0)?;
            }
        }
        let _ = vfs.create_file("/", path, data);
    }
    vfs.create_file("/", "/etc/motd", INIT_BANNER.as_bytes())?;
    vfs.create_file("/", "/bin/init", b"builtin /sbin/init\n")?;
    vfs.create_file("/", "/bin/child", b"builtin-child")?;
    vfs.create_file("/", "/proc/version", b"whuse-riscv64-virt")?;
    Ok(())
}

pub fn seed_process(process: &mut Process) {
    process.address_space.install_bytes(0x1000, b"/etc/motd\0");
    process.address_space.install_bytes(0x2000, b"/tmp/boot.log\0");
    process.address_space.install_bytes(0x3000, b"hello from init\n\0");
    process.address_space.install_bytes(0x4000, &[0; 256]);
}

pub fn builtin_program(path: &str) -> Option<BuiltinProgram> {
    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    unsafe {
        return match path {
            "/sbin/init" => Some(program_from_symbols(
                &whuse_user_init_start,
                &whuse_user_init_entry,
                &whuse_user_init_end,
            )),
            "/bin/init" => Some(program_from_symbols(
                &whuse_user_init_start,
                &whuse_user_init_entry,
                &whuse_user_init_end,
            )),
            "/bin/sh" => Some(program_from_symbols(
                &whuse_user_init_start,
                &whuse_user_init_entry,
                &whuse_user_init_end,
            )),
            "/bin/child" => Some(program_from_symbols(
                &whuse_user_child_start,
                &whuse_user_child_entry,
                &whuse_user_child_end,
            )),
            _ => None,
        };
    }

    #[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
    {
        let _ = path;
        None
    }
}

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
unsafe fn program_from_symbols(start: &u8, entry: &u8, end: &u8) -> BuiltinProgram {
    let start_ptr = start as *const u8 as usize;
    let end_ptr = end as *const u8 as usize;
    let entry_ptr = entry as *const u8 as usize;
    BuiltinProgram {
        entry: entry_ptr - start_ptr,
        image: core::slice::from_raw_parts(start as *const u8, end_ptr - start_ptr),
    }
}

fn create_dir_chain(vfs: &mut KernelVfs, path: &str) -> KernelResult<()> {
    let mut current = String::new();
    for component in path.split('/').filter(|segment| !segment.is_empty()) {
        current.push('/');
        current.push_str(component);
        if current != "/" {
            let _ = vfs.mkdir("/", &current, 0o755);
        }
    }
    Ok(())
}
