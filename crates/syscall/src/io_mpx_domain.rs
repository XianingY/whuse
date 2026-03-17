use crate::{
    DispatchContext, SyscallArgs, SYS_EPOLL_CREATE1, SYS_EPOLL_CTL, SYS_EPOLL_PWAIT,
    SYS_EPOLL_PWAIT2, SYS_EVENTFD2, SYS_PIPE, SYS_PPOLL, SYS_PSELECT6,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_EVENTFD2 => ctx.dispatcher.sys_eventfd2(args, ctx.procs, ctx.vfs),
        SYS_EPOLL_CREATE1 => ctx.dispatcher.sys_epoll_create1(args, ctx.procs, ctx.vfs),
        SYS_EPOLL_CTL => ctx.dispatcher.sys_epoll_ctl(args, ctx.procs, ctx.vfs),
        SYS_EPOLL_PWAIT => ctx
            .dispatcher
            .sys_epoll_pwait(args, ctx.procs, ctx.vfs, ctx.scheduler),
        SYS_EPOLL_PWAIT2 => {
            ctx.dispatcher
                .sys_epoll_pwait2(args, ctx.procs, ctx.vfs, ctx.scheduler)
        }
        SYS_PIPE => ctx.dispatcher.sys_pipe(args, ctx.procs, ctx.vfs),
        SYS_PPOLL => ctx
            .dispatcher
            .sys_ppoll(args, ctx.procs, ctx.vfs, ctx.scheduler),
        SYS_PSELECT6 => ctx
            .dispatcher
            .sys_pselect6(args, ctx.procs, ctx.vfs, ctx.scheduler),
        _ => return None,
    })
}
