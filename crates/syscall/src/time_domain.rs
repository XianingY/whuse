use crate::{
    DispatchContext, SyscallArgs, SYS_ADJTIMEX, SYS_CLOCK_GETRES, SYS_CLOCK_GETTIME,
    SYS_CLOCK_NANOSLEEP, SYS_CLOCK_SETTIME, SYS_GETITIMER, SYS_GETTIMEOFDAY, SYS_SETITIMER,
    SYS_SLEEP, SYS_TIMES,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_SLEEP => ctx.dispatcher.sys_nanosleep(args, ctx.procs, ctx.scheduler),
        SYS_GETITIMER => ctx.dispatcher.sys_getitimer(args, ctx.procs),
        SYS_SETITIMER => ctx.dispatcher.sys_setitimer(args, ctx.procs),
        SYS_ADJTIMEX => ctx.dispatcher.sys_adjtimex(args, ctx.procs),
        SYS_CLOCK_GETTIME => ctx.dispatcher.sys_clock_gettime(args, ctx.procs),
        SYS_CLOCK_SETTIME => ctx.dispatcher.sys_clock_settime(args, ctx.procs),
        SYS_CLOCK_GETRES => ctx.dispatcher.sys_clock_getres(args, ctx.procs),
        SYS_CLOCK_NANOSLEEP => ctx
            .dispatcher
            .sys_clock_nanosleep(args, ctx.procs, ctx.scheduler),
        SYS_TIMES => ctx.dispatcher.sys_times(args, ctx.procs),
        SYS_GETTIMEOFDAY => ctx.dispatcher.sys_gettimeofday(args, ctx.procs),
        _ => return None,
    })
}
