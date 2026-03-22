use crate::{
    DispatchContext, SyscallArgs, SYS_GETEGID, SYS_GETEUID, SYS_GETGID, SYS_GETGROUPS,
    SYS_GETRANDOM, SYS_GETUID, SYS_MEMBARRIER, SYS_MEMFD_CREATE, SYS_PIDFD_GETFD, SYS_PIDFD_OPEN,
    SYS_PIDFD_SEND_SIGNAL, SYS_PRCTL, SYS_RISCV_FLUSH_ICACHE, SYS_SECCOMP, SYS_SETGID,
    SYS_SETGROUPS, SYS_SETRESUID, SYS_SETREUID, SYS_SETUID, SYS_SYSINFO, SYS_UMASK, SYS_UNAME,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_GETUID => ctx.dispatcher.sys_getuid(ctx.procs),
        SYS_GETEUID => ctx.dispatcher.sys_geteuid(ctx.procs),
        SYS_GETGID => ctx.dispatcher.sys_getgid(ctx.procs),
        SYS_GETEGID => ctx.dispatcher.sys_getegid(ctx.procs),
        SYS_GETGROUPS => ctx.dispatcher.sys_getgroups(args, ctx.procs),
        SYS_SETGROUPS => ctx.dispatcher.sys_setgroups(args, ctx.procs),
        SYS_SETUID => ctx.dispatcher.sys_setuid(args, ctx.procs),
        SYS_SETREUID => ctx.dispatcher.sys_setreuid(args, ctx.procs),
        SYS_SETRESUID => ctx.dispatcher.sys_setresuid(args, ctx.procs),
        SYS_SETGID => ctx.dispatcher.sys_setgid(args, ctx.procs),
        SYS_UNAME => ctx.dispatcher.sys_uname(args, ctx.procs),
        SYS_SYSINFO => ctx.dispatcher.sys_sysinfo(args, ctx.procs, ctx.scheduler),
        SYS_UMASK => ctx.dispatcher.sys_umask(args, ctx.procs),
        SYS_PRCTL => ctx.dispatcher.sys_prctl(),
        SYS_GETRANDOM => ctx.dispatcher.sys_getrandom(args, ctx.procs),
        SYS_MEMFD_CREATE => ctx.dispatcher.sys_memfd_create(args, ctx.procs, ctx.vfs),
        SYS_MEMBARRIER => ctx.dispatcher.sys_membarrier(args),
        SYS_PIDFD_SEND_SIGNAL => ctx
            .dispatcher
            .sys_pidfd_send_signal(args, ctx.procs, ctx.vfs),
        SYS_PIDFD_OPEN => ctx.dispatcher.sys_pidfd_open(args, ctx.procs, ctx.vfs),
        SYS_PIDFD_GETFD => ctx.dispatcher.sys_pidfd_getfd(args, ctx.procs, ctx.vfs),
        SYS_SECCOMP => Ok(0),
        SYS_RISCV_FLUSH_ICACHE => Ok(0),
        _ => return None,
    })
}
