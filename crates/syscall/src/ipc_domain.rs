use crate::{DispatchContext, SyscallArgs, SYS_SHMAT, SYS_SHMCTL, SYS_SHMDT, SYS_SHMGET};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_SHMGET => ctx.dispatcher.sys_shmget(args),
        SYS_SHMAT => ctx.dispatcher.sys_shmat(args, ctx.procs),
        SYS_SHMCTL => ctx.dispatcher.sys_shmctl(args, ctx.procs),
        SYS_SHMDT => ctx.dispatcher.sys_shmdt(args),
        _ => return None,
    })
}
