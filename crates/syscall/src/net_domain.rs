use crate::{
    DispatchContext, SyscallArgs, SYS_ACCEPT, SYS_ACCEPT4, SYS_BIND, SYS_CONNECT, SYS_GETPEERNAME,
    SYS_GETSOCKNAME, SYS_GETSOCKOPT, SYS_LISTEN, SYS_RECVFROM, SYS_RECVMSG, SYS_SENDMSG,
    SYS_SENDTO, SYS_SETSOCKOPT, SYS_SHUTDOWN, SYS_SOCKET, SYS_SOCKETPAIR,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_SOCKET => ctx.dispatcher.sys_socket(args, ctx.procs, ctx.vfs),
        SYS_SOCKETPAIR => ctx.dispatcher.sys_socketpair(args, ctx.procs, ctx.vfs),
        SYS_BIND => ctx.dispatcher.sys_bind(args, ctx.procs, ctx.vfs),
        SYS_LISTEN => ctx.dispatcher.sys_listen(args, ctx.procs, ctx.vfs),
        SYS_ACCEPT | SYS_ACCEPT4 => {
            ctx.dispatcher
                .sys_accept(args, ctx.procs, ctx.scheduler, ctx.vfs)
        }
        SYS_CONNECT => ctx
            .dispatcher
            .sys_connect(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_GETSOCKNAME | SYS_GETPEERNAME => ctx.dispatcher.sys_getsockname(args, ctx.procs),
        SYS_SENDTO => ctx
            .dispatcher
            .sys_sendto(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_RECVFROM => ctx
            .dispatcher
            .sys_recvfrom(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_SETSOCKOPT => ctx.dispatcher.sys_setsockopt(args, ctx.procs, ctx.vfs),
        SYS_GETSOCKOPT => ctx.dispatcher.sys_getsockopt(args, ctx.procs, ctx.vfs),
        SYS_SHUTDOWN => ctx.dispatcher.sys_shutdown(args),
        SYS_SENDMSG => ctx
            .dispatcher
            .sys_sendmsg(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_RECVMSG => ctx
            .dispatcher
            .sys_recvmsg(args, ctx.procs, ctx.scheduler, ctx.vfs),
        _ => return None,
    })
}
