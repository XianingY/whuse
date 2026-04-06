use crate::{
    DispatchContext, SyscallArgs, SYS_MSGCTL, SYS_MSGGET, SYS_MSGRCV, SYS_MSGSND, SYS_SEMCTL,
    SYS_SEMGET, SYS_SEMOP, SYS_SEMTIMEDOP, SYS_SHMAT, SYS_SHMCTL, SYS_SHMDT, SYS_SHMGET,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_MSGGET => ctx.dispatcher.sys_msgget(args, ctx.procs),
        SYS_MSGSND => ctx.dispatcher.sys_msgsnd(args, ctx.procs, ctx.scheduler),
        SYS_MSGRCV => ctx.dispatcher.sys_msgrcv(args, ctx.procs, ctx.scheduler),
        SYS_MSGCTL => ctx.dispatcher.sys_msgctl(args, ctx.procs, ctx.scheduler),
        SYS_SEMGET => ctx.dispatcher.sys_semget(args, ctx.procs),
        SYS_SEMCTL => ctx.dispatcher.sys_semctl(args, ctx.procs, ctx.scheduler),
        SYS_SEMOP => ctx.dispatcher.sys_semop(args, ctx.procs, ctx.scheduler),
        SYS_SEMTIMEDOP => ctx
            .dispatcher
            .sys_semtimedop(args, ctx.procs, ctx.scheduler),
        SYS_SHMGET => ctx.dispatcher.sys_shmget(args),
        SYS_SHMAT => ctx.dispatcher.sys_shmat(args, ctx.procs),
        SYS_SHMCTL => ctx.dispatcher.sys_shmctl(args, ctx.procs),
        SYS_SHMDT => ctx.dispatcher.sys_shmdt(args, ctx.procs),
        _ => return None,
    })
}
