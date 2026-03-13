#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[cfg(target_arch = "riscv64")]
use core::arch::global_asm;
use proc::Process;
use vfs::{KernelResult, KernelVfs};

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
    addi sp, sp, -16
    li a0, 1
    la a1, init_msg
    li a2, init_msg_end - init_msg
    li a7, 64
    ecall

    li a7, 172
    ecall
    li a7, 173
    ecall
    li a7, 178
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
parent_msg:
    .ascii "user:init wait complete\n"
parent_msg_end:
exec_fail_msg:
    .ascii "user:init execve failed\n"
exec_fail_msg_end:
child_path:
    .asciz "/bin/child"
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

#[cfg(target_arch = "riscv64")]
unsafe extern "C" {
    static whuse_user_init_start: u8;
    static whuse_user_init_entry: u8;
    static whuse_user_init_end: u8;
    static whuse_user_child_start: u8;
    static whuse_user_child_entry: u8;
    static whuse_user_child_end: u8;
}

pub fn seed_filesystem(vfs: &mut KernelVfs) -> KernelResult<()> {
    vfs.create_file("/", "/etc/motd", INIT_BANNER.as_bytes())?;
    vfs.create_file("/", "/bin/init", b"builtin-init")?;
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
    #[cfg(target_arch = "riscv64")]
    unsafe {
        return match path {
            "/bin/init" => Some(program_from_symbols(
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

    #[cfg(not(target_arch = "riscv64"))]
    {
        let _ = path;
        None
    }
}

#[cfg(target_arch = "riscv64")]
unsafe fn program_from_symbols(start: &u8, entry: &u8, end: &u8) -> BuiltinProgram {
    let start_ptr = start as *const u8 as usize;
    let end_ptr = end as *const u8 as usize;
    let entry_ptr = entry as *const u8 as usize;
    BuiltinProgram {
        entry: entry_ptr - start_ptr,
        image: core::slice::from_raw_parts(start as *const u8, end_ptr - start_ptr),
    }
}
