use crate::{
    DispatchContext, SyscallArgs, SYS_FUTEX, SYS_GET_ROBUST_LIST, SYS_KILL, SYS_RT_SIGPENDING,
    SYS_RT_SIGRETURN, SYS_RT_SIGSUSPEND, SYS_RT_SIGTIMEDWAIT, SYS_SET_ROBUST_LIST,
    SYS_SIGACTION, SYS_SIGALTSTACK, SYS_SIGPROCMASK, SYS_TGKILL,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_FUTEX => ctx.dispatcher.sys_futex(args, ctx.procs, ctx.scheduler),
        SYS_SET_ROBUST_LIST => ctx.dispatcher.sys_set_robust_list(args, ctx.procs),
        SYS_GET_ROBUST_LIST => ctx.dispatcher.sys_get_robust_list(args, ctx.procs),
        SYS_KILL | SYS_TGKILL => ctx.dispatcher.sys_kill(args, ctx.procs),
        SYS_SIGALTSTACK => ctx.dispatcher.sys_sigaltstack(args, ctx.procs),
        SYS_RT_SIGSUSPEND => ctx.dispatcher.sys_rt_sigsuspend(args, ctx.procs),
        SYS_SIGACTION => ctx.dispatcher.sys_sigaction(args, ctx.procs),
        SYS_SIGPROCMASK => ctx.dispatcher.sys_sigprocmask(args, ctx.procs),
        SYS_RT_SIGPENDING => ctx.dispatcher.sys_rt_sigpending(args, ctx.procs),
        SYS_RT_SIGTIMEDWAIT => ctx.dispatcher.sys_rt_sigtimedwait(args, ctx.procs),
        SYS_RT_SIGRETURN => ctx.dispatcher.sys_rt_sigreturn(),
        _ => return None,
    })
}
