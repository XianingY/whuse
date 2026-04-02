use crate::{
    DispatchContext, SyscallArgs, SYS_CHDIR, SYS_CHROOT, SYS_CLOSE, SYS_CLOSE_RANGE,
    SYS_COPY_FILE_RANGE, SYS_DUP, SYS_DUP2, SYS_FACCESSAT, SYS_FACCESSAT2, SYS_FALLOCATE,
    SYS_FCHDIR, SYS_FCHMOD, SYS_FCHMODAT, SYS_FCHMODAT2, SYS_FCHOWN, SYS_FCHOWNAT, SYS_FCNTL,
    SYS_FDATASYNC, SYS_FLOCK, SYS_FSTAT, SYS_FSTATAT, SYS_FSTATFS, SYS_FSYNC, SYS_FTRUNCATE,
    SYS_GETCWD, SYS_GETDENTS64, SYS_IOCTL, SYS_LINKAT, SYS_LSEEK, SYS_MKDIR, SYS_MKNODAT,
    SYS_MOUNT, SYS_OPENAT, SYS_PREAD64, SYS_PREADV, SYS_PREADV2, SYS_PWRITE64, SYS_PWRITEV,
    SYS_PWRITEV2, SYS_READ, SYS_READLINKAT, SYS_READV, SYS_RENAMEAT, SYS_RENAMEAT2,
    SYS_SENDFILE, SYS_SPLICE, SYS_STATFS, SYS_STATX, SYS_SYMLINKAT, SYS_SYNC, SYS_TRUNCATE,
    SYS_UMOUNT2, SYS_UNLINKAT, SYS_UTIMENSAT, SYS_WRITE, SYS_WRITEV,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_GETCWD => ctx.dispatcher.sys_getcwd(args, ctx.procs),
        SYS_DUP => ctx.dispatcher.sys_dup(args, ctx.procs),
        SYS_DUP2 => ctx.dispatcher.sys_dup3(args, ctx.procs),
        SYS_FCNTL => ctx
            .dispatcher
            .sys_fcntl(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_IOCTL => ctx.dispatcher.sys_ioctl(args, ctx.procs, ctx.vfs),
        SYS_FLOCK => ctx.dispatcher.sys_flock(args, ctx.procs),
        SYS_MKNODAT => ctx.dispatcher.sys_mknodat(args, ctx.procs, ctx.vfs),
        SYS_MKDIR => ctx.dispatcher.sys_mkdir(args, ctx.procs, ctx.vfs),
        SYS_UNLINKAT => ctx.dispatcher.sys_unlinkat(args, ctx.procs, ctx.vfs),
        SYS_SYMLINKAT => ctx.dispatcher.sys_symlinkat(args, ctx.procs, ctx.vfs),
        SYS_LINKAT => ctx.dispatcher.sys_linkat(args, ctx.procs, ctx.vfs),
        SYS_RENAMEAT => ctx.dispatcher.sys_renameat(args, ctx.procs, ctx.vfs),
        SYS_RENAMEAT2 => ctx.dispatcher.sys_renameat2(args, ctx.procs, ctx.vfs),
        SYS_SYNC => ctx.dispatcher.sys_sync(),
        SYS_MOUNT => ctx.dispatcher.sys_mount(args, ctx.procs, ctx.vfs),
        SYS_UMOUNT2 => ctx.dispatcher.sys_umount(args, ctx.procs, ctx.vfs),
        SYS_STATFS => ctx.dispatcher.sys_statfs(args, ctx.procs, ctx.vfs),
        SYS_FSTATFS => ctx.dispatcher.sys_fstatfs(args, ctx.procs, ctx.vfs),
        SYS_TRUNCATE => ctx.dispatcher.sys_truncate(args, ctx.procs, ctx.vfs),
        SYS_FTRUNCATE => ctx.dispatcher.sys_ftruncate(args, ctx.procs, ctx.vfs),
        SYS_FALLOCATE => ctx.dispatcher.sys_fallocate(args, ctx.procs, ctx.vfs),
        SYS_UTIMENSAT => ctx.dispatcher.sys_utimensat(args, ctx.procs, ctx.vfs),
        SYS_FACCESSAT | SYS_FACCESSAT2 => ctx.dispatcher.sys_faccessat(args, ctx.procs, ctx.vfs),
        SYS_OPENAT => ctx
            .dispatcher
            .sys_openat(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_CLOSE => ctx
            .dispatcher
            .sys_close(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_CLOSE_RANGE => ctx
            .dispatcher
            .sys_close_range(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_GETDENTS64 => ctx.dispatcher.sys_getdents64(args, ctx.procs, ctx.vfs),
        SYS_LSEEK => ctx.dispatcher.sys_lseek(args, ctx.procs, ctx.vfs),
        SYS_READ => ctx
            .dispatcher
            .sys_read(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_WRITE => ctx
            .dispatcher
            .sys_write(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_READV => ctx
            .dispatcher
            .sys_readv(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_WRITEV => ctx
            .dispatcher
            .sys_writev(args, ctx.procs, ctx.scheduler, ctx.vfs),
        SYS_PREAD64 => ctx.dispatcher.sys_pread64(args, ctx.procs, ctx.vfs),
        SYS_PWRITE64 => ctx.dispatcher.sys_pwrite64(args, ctx.procs, ctx.vfs),
        SYS_PREADV => ctx.dispatcher.sys_preadv(args, ctx.procs, ctx.vfs),
        SYS_PWRITEV => ctx.dispatcher.sys_pwritev(args, ctx.procs, ctx.vfs),
        SYS_PREADV2 => ctx.dispatcher.sys_preadv(args, ctx.procs, ctx.vfs),
        SYS_PWRITEV2 => ctx.dispatcher.sys_pwritev(args, ctx.procs, ctx.vfs),
        SYS_SENDFILE => ctx.dispatcher.sys_sendfile(args, ctx.procs, ctx.vfs),
        SYS_SPLICE => ctx.dispatcher.sys_splice(args, ctx.procs, ctx.vfs),
        SYS_READLINKAT => ctx.dispatcher.sys_readlinkat(args, ctx.procs, ctx.vfs),
        SYS_FSTATAT => ctx.dispatcher.sys_fstatat(args, ctx.procs, ctx.vfs),
        SYS_FSTAT => ctx.dispatcher.sys_fstat(args, ctx.procs, ctx.vfs),
        SYS_CHDIR => ctx.dispatcher.sys_chdir(args, ctx.procs, ctx.vfs),
        SYS_FCHDIR => ctx.dispatcher.sys_fchdir(args, ctx.procs, ctx.vfs),
        SYS_CHROOT => ctx.dispatcher.sys_chroot(args, ctx.procs, ctx.vfs),
        SYS_FCHMOD => ctx.dispatcher.sys_fchmod(args, ctx.procs, ctx.vfs),
        SYS_FCHMODAT => {
            let legacy_args = SyscallArgs([args.0[0], args.0[1], args.0[2], 0, args.0[4], args.0[5]]);
            ctx.dispatcher.sys_fchmodat(legacy_args, ctx.procs, ctx.vfs)
        }
        SYS_FCHMODAT2 => ctx.dispatcher.sys_fchmodat(args, ctx.procs, ctx.vfs),
        SYS_FCHOWNAT => ctx.dispatcher.sys_fchownat(args, ctx.procs, ctx.vfs),
        SYS_FCHOWN => ctx.dispatcher.sys_fchown(args, ctx.procs),
        SYS_FSYNC | SYS_FDATASYNC => ctx.dispatcher.sys_fsync(args, ctx.procs),
        SYS_STATX => ctx.dispatcher.sys_statx(args, ctx.procs, ctx.vfs),
        SYS_COPY_FILE_RANGE => ctx.dispatcher.sys_copy_file_range(args, ctx.procs, ctx.vfs),
        _ => return None,
    })
}
