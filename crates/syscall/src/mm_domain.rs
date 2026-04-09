use crate::{
    DispatchContext, SyscallArgs, SYS_BRK, SYS_MADVISE, SYS_MLOCK, SYS_MLOCK2, SYS_MLOCKALL,
    SYS_MMAP, SYS_MPROTECT, SYS_MREMAP, SYS_MSYNC, SYS_MUNLOCKALL, SYS_MUNMAP,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_BRK => ctx.dispatcher.sys_brk(args, ctx.procs),
        SYS_MREMAP => ctx.dispatcher.sys_mremap(args, ctx.procs),
        SYS_MMAP => ctx.dispatcher.sys_mmap(args, ctx.procs, ctx.vfs),
        SYS_MUNMAP => ctx.dispatcher.sys_munmap(args, ctx.procs),
        SYS_MPROTECT => ctx.dispatcher.sys_mprotect(args, ctx.procs),
        SYS_MSYNC => ctx.dispatcher.sys_msync(args, ctx.procs),
        SYS_MLOCK => ctx.dispatcher.sys_mlock(args, ctx.procs),
        SYS_MLOCK2 => ctx.dispatcher.sys_mlock2(args, ctx.procs),
        SYS_MLOCKALL => ctx.dispatcher.sys_mlockall(args, ctx.procs),
        SYS_MUNLOCKALL => ctx.dispatcher.sys_munlockall(ctx.procs),
        SYS_MADVISE => Ok(0),
        _ => return None,
    })
}
