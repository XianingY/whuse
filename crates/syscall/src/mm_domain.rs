use crate::{
    DispatchContext, EINVAL, SyscallArgs, SYS_BRK, SYS_MADVISE, SYS_MINCORE, SYS_MLOCK,
    SYS_MLOCK2, SYS_MLOCKALL, SYS_MMAP, SYS_MPROTECT, SYS_MREMAP, SYS_MSYNC, SYS_MUNLOCKALL,
    SYS_MUNMAP,
};

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_BRK => ctx.dispatcher.sys_brk(args, ctx.procs),
        SYS_MREMAP => ctx.dispatcher.sys_mremap(args, ctx.procs),
        SYS_MMAP => ctx.dispatcher.sys_mmap(args, ctx.procs, ctx.vfs),
        SYS_MUNMAP => ctx.dispatcher.sys_munmap(args, ctx.procs),
        SYS_MPROTECT => ctx.dispatcher.sys_mprotect(args, ctx.procs),
        SYS_MSYNC => ctx.dispatcher.sys_msync(args, ctx.procs),
        SYS_MLOCK => ctx.dispatcher.sys_mlock(args, ctx.procs),
        SYS_MLOCK2 => ctx.dispatcher.sys_mlock2(args, ctx.procs),
        SYS_MLOCKALL => ctx.dispatcher.sys_mlockall(args, ctx.procs),
        SYS_MUNLOCKALL => ctx.dispatcher.sys_munlockall(ctx.procs),
        SYS_MADVISE => {
            // madvise(addr, length, advice)
            let advice = args.0[2] as i32;
            const MADV_NORMAL: i32 = 0;
            const MADV_DONTNEED: i32 = 6;
            const MADV_FREE: i32 = 8;
            match advice {
                MADV_NORMAL | MADV_DONTNEED | MADV_FREE => Ok(0),
                _ => Err(EINVAL),
            }
        }
        SYS_MINCORE => {
            // mincore(addr, length, vec) - returns which pages are resident
            let addr = args.0[0];
            let len = args.0[1];
            let vec_addr = args.0[2];
            let process = ctx.procs.current_mut().ok()?;

            // Calculate number of pages
            let page_size = 4096;
            let num_pages = (len + page_size - 1) / page_size;

            // For each page, check if it's mapped and resident
            for i in 0..num_pages {
                let page_addr = addr + i * page_size;
                // Check if page is accessible (mapped)
                let resident = if process.read_user_bytes(page_addr, 1).is_ok() {
                    1u8
                } else {
                    0u8
                };
                // Write the resident byte
                let _ = process.write_user_bytes(vec_addr + i, &[resident]);
            }
            Ok(0)
        }
        _ => return None,
    })
}
