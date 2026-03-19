use crate::{
    DispatchContext, SyscallArgs, SYS_GETPRIORITY, SYS_GETRUSAGE, SYS_PRLIMIT64, SYS_SYSLOG,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_GETPRIORITY => ctx.dispatcher.sys_getpriority(),
        SYS_GETRUSAGE => ctx.dispatcher.sys_getrusage(args, ctx.procs),
        SYS_PRLIMIT64 => ctx.dispatcher.sys_prlimit64(args, ctx.procs),
        SYS_SYSLOG => ctx.dispatcher.sys_syslog(args, ctx.procs),
        _ => return None,
    })
}
