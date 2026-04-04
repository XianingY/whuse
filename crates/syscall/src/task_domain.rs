use crate::{
    DispatchContext, SyscallArgs, SYS_CLONE, SYS_CLONE3, SYS_EXECVE, SYS_EXIT, SYS_EXIT_GROUP,
    SYS_GETPGID, SYS_GETPID, SYS_GETPPID, SYS_GETSID, SYS_GETTID, SYS_POWER_OFF,
    SYS_SCHED_GETPARAM, SYS_SCHED_GETSCHEDULER, SYS_SCHED_SETPARAM, SYS_SCHED_SETSCHEDULER,
    SYS_SCHED_YIELD, SYS_SETPGID, SYS_SETSID, SYS_SET_TID_ADDRESS, SYS_WAIT, SYS_WAITID,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_SET_TID_ADDRESS => ctx.dispatcher.sys_set_tid_address(args, ctx.procs),
        SYS_SCHED_YIELD => ctx.dispatcher.sys_sched_yield(ctx.scheduler),
        SYS_SCHED_SETSCHEDULER => ctx.dispatcher.sys_sched_setscheduler(args, ctx.procs),
        SYS_SCHED_GETSCHEDULER => ctx.dispatcher.sys_sched_getscheduler(args, ctx.procs),
        SYS_SCHED_SETPARAM => ctx.dispatcher.sys_sched_setparam(args, ctx.procs),
        SYS_SCHED_GETPARAM => ctx.dispatcher.sys_sched_getparam(args, ctx.procs),
        SYS_EXIT => ctx
            .dispatcher
            .sys_exit(args, ctx.procs, ctx.scheduler, ctx.vfs, false),
        SYS_EXIT_GROUP => ctx
            .dispatcher
            .sys_exit(args, ctx.procs, ctx.scheduler, ctx.vfs, true),
        SYS_GETPID => ctx.dispatcher.sys_getpid(ctx.procs),
        SYS_GETPPID => ctx.dispatcher.sys_getppid(ctx.procs),
        SYS_GETTID => ctx.dispatcher.sys_gettid(ctx.procs),
        SYS_CLONE => ctx.dispatcher.sys_clone(args, ctx.procs, ctx.scheduler),
        SYS_EXECVE => ctx
            .dispatcher
            .sys_execve(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_WAIT => ctx.dispatcher.sys_wait(args, ctx.procs, ctx.scheduler),
        SYS_WAITID => ctx.dispatcher.sys_waitid(args, ctx.procs, ctx.scheduler),
        SYS_SETPGID => ctx.dispatcher.sys_setpgid(args, ctx.procs),
        SYS_GETPGID => ctx.dispatcher.sys_getpgid(args, ctx.procs),
        SYS_GETSID => ctx.dispatcher.sys_getsid(args, ctx.procs),
        SYS_SETSID => ctx.dispatcher.sys_setsid(ctx.procs),
        SYS_CLONE3 => ctx.dispatcher.sys_clone3(args, ctx.procs, ctx.scheduler),
        SYS_POWER_OFF => Ok(0),
        _ => return None,
    })
}
