use crate::{
    DispatchContext, SyscallArgs, SYS_GETPRIORITY, SYS_GETRUSAGE, SYS_PRLIMIT64,
    SYS_SCHED_GETAFFINITY, SYS_SCHED_GETPARAM, SYS_SCHED_GETSCHEDULER, SYS_SCHED_SETAFFINITY,
    SYS_SCHED_SETPARAM, SYS_SCHED_SETSCHEDULER, SYS_SYSLOG,
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
        SYS_SCHED_SETPARAM => ctx.dispatcher.sys_sched_setparam(args, ctx.procs),
        SYS_SCHED_GETPARAM => ctx.dispatcher.sys_sched_getparam(args, ctx.procs),
        SYS_SCHED_SETSCHEDULER => ctx.dispatcher.sys_sched_setscheduler(args, ctx.procs),
        SYS_SCHED_GETSCHEDULER => ctx.dispatcher.sys_sched_getscheduler(args, ctx.procs),
        SYS_SCHED_SETAFFINITY => ctx.dispatcher.sys_sched_setaffinity(args, ctx.procs),
        SYS_SCHED_GETAFFINITY => ctx.dispatcher.sys_sched_getaffinity(args, ctx.procs),
        SYS_SYSLOG => ctx.dispatcher.sys_syslog(args, ctx.procs),
        _ => return None,
    })
}
