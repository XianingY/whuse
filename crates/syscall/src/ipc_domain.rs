use crate::{
    has_unmasked_pending_signal, ipc_access_allowed, wake_tasks_by_tid, DispatchContext, MsgEntry,
    MsgQueue, SemBufOp, SemSet, ShmAttachment, ShmSegment, ShmidDs, SyscallArgs, EACCES, EAGAIN,
    EEXIST, EFAULT, EIDRM, EINTR, EINVAL, ENOENT, ENOMSG, EPERM, IPC_CREAT, IPC_EXCL, IPC_INFO,
    IPC_NOWAIT, IPC_PRIVATE, IPC_RMID, IPC_SET, IPC_STAT, MSG_INFO, MSG_STAT, MSG_STATE,
    SEMCTL_SETVAL, SEMMSL_LIMIT, SEM_STATE, SHM_LOCK, SHM_STATE, SHM_UNLOCK, SYS_MSGCTL,
    SYS_MSGGET, SYS_MSGRCV, SYS_MSGSND, SYS_SEMCTL, SYS_SEMGET, SYS_SEMOP, SYS_SEMTIMEDOP,
    SYS_SHMAT, SYS_SHMCTL, SYS_SHMDT, SYS_SHMGET,
};
use alloc::collections::{BTreeSet, VecDeque};
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use proc::ProcessTable;
use task::Scheduler;

pub(crate) fn sys_msgget(args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
    let key = args.0[0] as i32;
    let flags = args.0[1];
    let caller_uid = procs.current()?.euid;
    let req_read = (flags & 0o400) != 0;
    let req_write = (flags & 0o200) != 0;
    let mut state = MSG_STATE.lock();

    if key != IPC_PRIVATE {
        if let Some(id) = state.keys.get(&key).copied() {
            let queue = state.queues.get(&id).ok_or(EINVAL)?;
            if queue.destroyed {
                return Err(EIDRM);
            }
            if (flags & IPC_CREAT) != 0 && (flags & IPC_EXCL) != 0 {
                return Err(EEXIST);
            }
            if !ipc_access_allowed(queue.mode, queue.owner_uid, caller_uid, req_read, req_write) {
                return Err(EACCES);
            }
            return Ok(id);
        }
        if (flags & IPC_CREAT) == 0 {
            return Err(ENOENT);
        }
    }

    let id = state.next_id;
    state.next_id += 1;
    let mode = (flags & 0o777) as u16;
    state.queues.insert(
        id,
        MsgQueue {
            key,
            owner_uid: caller_uid,
            mode,
            entries: VecDeque::new(),
            waiters: BTreeSet::new(),
            destroyed: false,
        },
    );
    if key != IPC_PRIVATE {
        state.keys.insert(key, id);
    }
    Ok(id)
}

pub(crate) fn sys_msgsnd(
    args: SyscallArgs,
    procs: &mut ProcessTable,
    scheduler: &mut Scheduler,
) -> Result<usize, i32> {
    let id = args.0[0];
    let msgp = args.0[1];
    let msgsz = args.0[2];
    let process = procs.current()?;
    let mtype_raw = process
        .read_user_bytes(msgp, size_of::<isize>())
        .map_err(|_| EFAULT)?;
    let mut mtype_bytes = [0u8; size_of::<isize>()];
    mtype_bytes.copy_from_slice(&mtype_raw[..size_of::<isize>()]);
    let mtype = isize::from_le_bytes(mtype_bytes);
    if mtype <= 0 {
        return Err(EINVAL);
    }
    let payload = process
        .read_user_bytes(msgp + size_of::<isize>(), msgsz)
        .map_err(|_| EFAULT)?;
    let caller_uid = process.euid;
    let waiters = {
        let mut state = MSG_STATE.lock();
        let queue = state.queues.get_mut(&id).ok_or(EINVAL)?;
        if queue.destroyed {
            return Err(EIDRM);
        }
        if !ipc_access_allowed(queue.mode, queue.owner_uid, caller_uid, false, true) {
            return Err(EACCES);
        }
        queue.entries.push_back(MsgEntry { mtype, payload });
        queue.waiters.clone()
    };
    wake_tasks_by_tid(&waiters, scheduler);
    Ok(0)
}

pub(crate) fn sys_msgrcv(
    args: SyscallArgs,
    procs: &mut ProcessTable,
    scheduler: &mut Scheduler,
) -> Result<usize, i32> {
    let id = args.0[0];
    let msgp = args.0[1];
    let msgsz = args.0[2];
    let msgtyp = args.0[3] as isize;
    let msgflg = args.0[4];
    let tid = procs.current_tid()?;

    loop {
        if has_unmasked_pending_signal(procs.current()?) {
            let mut state = MSG_STATE.lock();
            if let Some(queue) = state.queues.get_mut(&id) {
                queue.waiters.remove(&tid);
            }
            return Err(EINTR);
        }

        let mut maybe_msg = None;
        let mut removed = false;
        {
            let mut state = MSG_STATE.lock();
            let queue = state.queues.get_mut(&id).ok_or(EINVAL)?;
            if queue.destroyed {
                queue.waiters.remove(&tid);
                removed = true;
            } else {
                let idx = if msgtyp == 0 {
                    if queue.entries.is_empty() {
                        None
                    } else {
                        Some(0)
                    }
                } else if msgtyp > 0 {
                    queue.entries.iter().position(|msg| msg.mtype == msgtyp)
                } else {
                    let limit = -msgtyp;
                    queue.entries.iter().position(|msg| msg.mtype <= limit)
                };
                if let Some(i) = idx {
                    maybe_msg = queue.entries.remove(i);
                    queue.waiters.remove(&tid);
                } else if (msgflg & IPC_NOWAIT) != 0 {
                    queue.waiters.remove(&tid);
                    return Err(ENOMSG);
                } else {
                    queue.waiters.insert(tid);
                }
            }
        }

        if removed {
            return Err(EIDRM);
        }
        if let Some(msg) = maybe_msg {
            let copy_len = msg.payload.len().min(msgsz);
            let mut out = Vec::with_capacity(size_of::<isize>() + copy_len);
            out.extend_from_slice(&msg.mtype.to_le_bytes());
            out.extend_from_slice(&msg.payload[..copy_len]);
            procs
                .current_mut()?
                .write_user_bytes(msgp, &out)
                .map_err(|_| EFAULT)?;
            return Ok(copy_len);
        }
        let _ = scheduler.block_current();
        return Err(EAGAIN);
    }
}

pub(crate) fn sys_msgctl(
    args: SyscallArgs,
    procs: &mut ProcessTable,
    scheduler: &mut Scheduler,
) -> Result<usize, i32> {
    let id = args.0[0];
    let cmd = args.0[1] as i32;
    let caller_uid = procs.current()?.euid;
    match cmd {
        IPC_RMID => {
            let waiters = {
                let mut state = MSG_STATE.lock();
                let key = {
                    let queue = state.queues.get_mut(&id).ok_or(EINVAL)?;
                    queue.destroyed = true;
                    queue.waiters.clone()
                };
                let key_to_remove = state
                    .queues
                    .get(&id)
                    .and_then(|queue| (queue.key != IPC_PRIVATE).then_some(queue.key));
                if let Some(key) = key_to_remove {
                    state.keys.remove(&key);
                }
                key
            };
            wake_tasks_by_tid(&waiters, scheduler);
            Ok(0)
        }
        IPC_INFO | MSG_INFO => {
            let state = MSG_STATE.lock();
            if !state.queues.contains_key(&id) {
                return Err(EINVAL);
            }
            Ok(id)
        }
        MSG_STAT | IPC_STAT => {
            let state = MSG_STATE.lock();
            let queue = state.queues.get(&id).ok_or(EINVAL)?;
            if queue.destroyed {
                return Err(EIDRM);
            }
            if !ipc_access_allowed(queue.mode, queue.owner_uid, caller_uid, true, false) {
                return Err(EACCES);
            }
            Ok(id)
        }
        IPC_SET => Ok(0),
        _ => Err(EINVAL),
    }
}

pub(crate) fn sys_semget(args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
    let key = args.0[0] as i32;
    let nsems = args.0[1] as i64;
    let flags = args.0[2];
    let caller_uid = procs.current()?.euid;
    let req_read = (flags & 0o400) != 0;
    let req_write = (flags & 0o200) != 0;

    if nsems < 0 {
        return Err(EINVAL);
    }
    let nsems = nsems as usize;
    let mut state = SEM_STATE.lock();
    if key != IPC_PRIVATE {
        if let Some(id) = state.keys.get(&key).copied() {
            let sem = state.sets.get(&id).ok_or(EINVAL)?;
            if sem.destroyed {
                return Err(EIDRM);
            }
            if (flags & IPC_CREAT) != 0 && (flags & IPC_EXCL) != 0 {
                return Err(EEXIST);
            }
            if nsems > 0 && nsems > sem.values.len() {
                return Err(EINVAL);
            }
            if !ipc_access_allowed(sem.mode, sem.owner_uid, caller_uid, req_read, req_write) {
                return Err(EACCES);
            }
            return Ok(id);
        }
        if (flags & IPC_CREAT) == 0 {
            return Err(ENOENT);
        }
    }

    if nsems == 0 || nsems > SEMMSL_LIMIT {
        return Err(EINVAL);
    }
    let id = state.next_id;
    state.next_id += 1;
    let mode = (flags & 0o777) as u16;
    state.sets.insert(
        id,
        SemSet {
            key,
            owner_uid: caller_uid,
            mode,
            values: vec![0; nsems],
            waiters: BTreeSet::new(),
            destroyed: false,
        },
    );
    if key != IPC_PRIVATE {
        state.keys.insert(key, id);
    }
    Ok(id)
}

pub(crate) fn sys_semctl(
    args: SyscallArgs,
    procs: &mut ProcessTable,
    scheduler: &mut Scheduler,
) -> Result<usize, i32> {
    let id = args.0[0];
    let semnum = args.0[1];
    let cmd = args.0[2] as i32;
    let arg = args.0[3] as i32;
    let caller_uid = procs.current()?.euid;
    match cmd {
        IPC_RMID => {
            let waiters = {
                let mut state = SEM_STATE.lock();
                let waiters = {
                    let sem = state.sets.get_mut(&id).ok_or(EINVAL)?;
                    sem.destroyed = true;
                    sem.waiters.clone()
                };
                let key_to_remove = state
                    .sets
                    .get(&id)
                    .and_then(|sem| (sem.key != IPC_PRIVATE).then_some(sem.key));
                if let Some(key) = key_to_remove {
                    state.keys.remove(&key);
                }
                waiters
            };
            wake_tasks_by_tid(&waiters, scheduler);
            Ok(0)
        }
        SEMCTL_SETVAL => {
            let waiters = {
                let mut state = SEM_STATE.lock();
                let sem = state.sets.get_mut(&id).ok_or(EINVAL)?;
                if sem.destroyed {
                    return Err(EIDRM);
                }
                if semnum >= sem.values.len() {
                    return Err(EINVAL);
                }
                sem.values[semnum] = arg;
                sem.waiters.clone()
            };
            wake_tasks_by_tid(&waiters, scheduler);
            Ok(0)
        }
        IPC_STAT => {
            let state = SEM_STATE.lock();
            let sem = state.sets.get(&id).ok_or(EINVAL)?;
            if sem.destroyed {
                return Err(EIDRM);
            }
            if !ipc_access_allowed(sem.mode, sem.owner_uid, caller_uid, true, false) {
                return Err(EACCES);
            }
            Ok(0)
        }
        IPC_INFO | MSG_INFO => Ok(0),
        _ => Err(EINVAL),
    }
}

fn read_semop_entries(
    procs: &mut ProcessTable,
    addr: usize,
    count: usize,
) -> Result<Vec<SemBufOp>, i32> {
    let mut ops = Vec::with_capacity(count);
    for i in 0..count {
        let off = addr + i * 6;
        let raw = procs
            .current()?
            .read_user_bytes(off, 6)
            .map_err(|_| EFAULT)?;
        let sem_num = u16::from_le_bytes([raw[0], raw[1]]);
        let sem_op = i16::from_le_bytes([raw[2], raw[3]]);
        let sem_flg = i16::from_le_bytes([raw[4], raw[5]]);
        ops.push(SemBufOp {
            sem_num,
            sem_op,
            sem_flg,
        });
    }
    Ok(ops)
}

fn sys_semop_common(
    args: SyscallArgs,
    procs: &mut ProcessTable,
    scheduler: &mut Scheduler,
) -> Result<usize, i32> {
    let id = args.0[0];
    let sops = args.0[1];
    let nsops = args.0[2];
    if nsops == 0 || nsops > 64 {
        return Err(EINVAL);
    }
    let ops = read_semop_entries(procs, sops, nsops)?;
    let tid = procs.current_tid()?;
    loop {
        if has_unmasked_pending_signal(procs.current()?) {
            let mut state = SEM_STATE.lock();
            if let Some(sem) = state.sets.get_mut(&id) {
                sem.waiters.remove(&tid);
            }
            return Err(EINTR);
        }

        let mut should_block = false;
        let mut removed = false;
        {
            let mut state = SEM_STATE.lock();
            let sem = state.sets.get_mut(&id).ok_or(EINVAL)?;
            if sem.destroyed {
                sem.waiters.remove(&tid);
                removed = true;
            } else {
                let mut can_apply = true;
                let mut nowait = false;
                for op in &ops {
                    let idx = op.sem_num as usize;
                    if idx >= sem.values.len() {
                        return Err(EINVAL);
                    }
                    let cur = sem.values[idx];
                    if op.sem_op == 0 {
                        if cur != 0 {
                            can_apply = false;
                        }
                    } else if op.sem_op < 0 && cur < -(op.sem_op as i32) {
                        can_apply = false;
                    }
                    if (op.sem_flg as usize & IPC_NOWAIT) != 0 {
                        nowait = true;
                    }
                }
                if can_apply {
                    for op in &ops {
                        let idx = op.sem_num as usize;
                        sem.values[idx] = sem.values[idx].saturating_add(op.sem_op as i32);
                    }
                    sem.waiters.remove(&tid);
                    return Ok(0);
                }
                if nowait {
                    sem.waiters.remove(&tid);
                    return Err(EAGAIN);
                }
                sem.waiters.insert(tid);
                should_block = true;
            }
        }
        if removed {
            return Err(EIDRM);
        }
        if !should_block {
            return Ok(0);
        }
        let _ = scheduler.block_current();
        return Err(EAGAIN);
    }
}

pub(crate) fn sys_semop(
    args: SyscallArgs,
    procs: &mut ProcessTable,
    scheduler: &mut Scheduler,
) -> Result<usize, i32> {
    sys_semop_common(args, procs, scheduler)
}

pub(crate) fn sys_semtimedop(
    args: SyscallArgs,
    procs: &mut ProcessTable,
    scheduler: &mut Scheduler,
) -> Result<usize, i32> {
    sys_semop_common(args, procs, scheduler)
}

pub(crate) fn sys_shmget(args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
    let key = args.0[0] as i32;
    let size = args.0[1];
    let flags = args.0[2];
    let caller_uid = procs.current()?.euid;
    let req_read = (flags & 0o400) != 0;
    let req_write = (flags & 0o200) != 0;
    let mut state = SHM_STATE.lock();

    if key != IPC_PRIVATE {
        if let Some(id) = state.keys.get(&key).copied() {
            let segment = state.segments.get(&id).ok_or(EINVAL)?;
            if segment.destroyed {
                return Err(EINVAL);
            }
            if (flags & IPC_CREAT) != 0 && (flags & IPC_EXCL) != 0 {
                return Err(EEXIST);
            }
            if !ipc_access_allowed(
                segment.mode,
                segment.owner_uid,
                caller_uid,
                req_read,
                req_write,
            ) {
                return Err(EACCES);
            }
            return Ok(id);
        }
        if (flags & IPC_CREAT) == 0 {
            return Err(ENOENT);
        }
    }

    let id = state.next_id;
    state.next_id += 1;
    let mode = (flags & 0o777) as u16;
    let segment = ShmSegment::new(key, size, caller_uid, mode, 0);
    state.segments.insert(id, segment);
    if key != 0 {
        state.keys.insert(key, id);
    }
    Ok(id)
}

pub(crate) fn sys_shmat(args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
    let id = args.0[0];
    let _addr = args.0[1];
    let _flags = args.0[2];

    let state = SHM_STATE.lock();
    let segment = state.segments.get(&id).ok_or(ENOENT)?;

    if segment.destroyed && segment.attach_count == 0 {
        return Err(EIDRM);
    }

    let data_arc = segment.data.clone();
    let data_len = data_arc.lock().len();
    drop(state);

    let addr =
        procs
            .current_mut()?
            .address_space
            .map_shared_existing(data_len, data_arc.clone(), 0b11)?;

    let mut state = SHM_STATE.lock();
    if let Some(segment) = state.segments.get_mut(&id) {
        segment.attach_count += 1;
        segment.attachments.push(ShmAttachment { addr, id });
    }
    drop(state);

    Ok(addr)
}

pub(crate) fn sys_shmctl(args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
    let id = args.0[0];
    let cmd = args.0[1] as i32;
    let buf = args.0[2];
    let caller_uid = procs.current()?.euid;

    let mut state = SHM_STATE.lock();
    match cmd {
        IPC_RMID => {
            let (key, attach_count) = {
                let segment = match state.segments.get(&id) {
                    Some(s) if !s.destroyed => s,
                    _ => return Err(EINVAL),
                };
                if caller_uid != 0 && caller_uid != segment.owner_uid {
                    return Err(EPERM);
                }
                (segment.key, segment.attach_count)
            };
            let segment = match state.segments.get_mut(&id) {
                Some(s) if !s.destroyed => s,
                _ => return Err(EINVAL),
            };
            segment.destroyed = true;
            if key != 0 {
                let _ = segment;
                state.keys.remove(&key);
                if attach_count == 0 {
                    state.segments.remove(&id);
                }
            } else if attach_count == 0 {
                state.segments.remove(&id);
            }
        }
        IPC_SET => {
            if buf == 0 {
                return Err(EFAULT);
            }
            let segment = match state.segments.get(&id) {
                Some(s) if !s.destroyed => s,
                _ => return Err(EINVAL),
            };
            if caller_uid != 0 && caller_uid != segment.owner_uid {
                return Err(EPERM);
            }
            procs
                .current_mut()?
                .read_user_bytes(buf, core::mem::size_of::<ShmidDs>())
                .map_err(|_| EFAULT)?;
        }
        IPC_STAT => {
            if buf == 0 {
                return Err(EFAULT);
            }
            let segment = match state.segments.get(&id) {
                Some(s) if !s.destroyed => s,
                _ => return Err(EINVAL),
            };
            if !ipc_access_allowed(segment.mode, segment.owner_uid, caller_uid, true, false) {
                return Err(EACCES);
            }

            let info = ShmidDs {
                shm_segsz: segment.data.lock().len(),
                shm_nattch: segment.attach_count,
                shm_cpid: segment.creator_pid,
                shm_lpid: 0,
                shm_atime: 0,
                shm_dtime: 0,
                shm_ctime: 0,
                _pad: [0; 3],
            };

            let bytes: &[u8] = unsafe {
                core::slice::from_raw_parts(
                    &info as *const ShmidDs as *const u8,
                    core::mem::size_of::<ShmidDs>(),
                )
            };
            procs
                .current_mut()?
                .write_user_bytes(buf, bytes)
                .map_err(|_| EFAULT)?;
        }
        SHM_LOCK | SHM_UNLOCK => {
            let segment = match state.segments.get(&id) {
                Some(s) if !s.destroyed => s,
                _ => return Err(EINVAL),
            };
            if caller_uid != 0 && caller_uid != segment.owner_uid {
                return Err(EPERM);
            }
            return Err(EPERM);
        }
        _ => {
            return Err(EINVAL);
        }
    }
    Ok(0)
}

pub(crate) fn sys_shmdt(args: SyscallArgs, procs: &mut ProcessTable) -> Result<usize, i32> {
    let addr = args.0[0];

    let segment_info = {
        let state = SHM_STATE.lock();
        state
            .segments
            .values()
            .find(|s| s.attachments.iter().any(|a| a.addr == addr))
            .map(|s| s.data.lock().len())
    };

    if let Some(len) = segment_info {
        procs.current_mut()?.address_space.unmap(addr, len)?;
    }

    let mut state = SHM_STATE.lock();
    for id in state.segments.keys().copied().collect::<Vec<_>>() {
        let should_remove = {
            let segment = match state.segments.get_mut(&id) {
                Some(segment) => segment,
                None => continue,
            };
            if let Some(pos) = segment.attachments.iter().position(|a| a.addr == addr) {
                segment.attachments.remove(pos);
                segment.attach_count = segment.attach_count.saturating_sub(1);
                segment.destroyed && segment.attach_count == 0
            } else {
                continue;
            }
        };
        if should_remove {
            state.segments.remove(&id);
        }
        return Ok(0);
    }

    Err(EINVAL)
}

pub(crate) fn dispatch(
    ctx: &mut DispatchContext<'_>,
    sysno: usize,
    args: SyscallArgs,
) -> Option<Result<usize, i32>> {
    Some(match sysno {
        SYS_MSGGET => sys_msgget(args, ctx.procs),
        SYS_MSGSND => sys_msgsnd(args, ctx.procs, ctx.scheduler),
        SYS_MSGRCV => sys_msgrcv(args, ctx.procs, ctx.scheduler),
        SYS_MSGCTL => sys_msgctl(args, ctx.procs, ctx.scheduler),
        SYS_SEMGET => sys_semget(args, ctx.procs),
        SYS_SEMCTL => sys_semctl(args, ctx.procs, ctx.scheduler),
        SYS_SEMOP => sys_semop(args, ctx.procs, ctx.scheduler),
        SYS_SEMTIMEDOP => sys_semtimedop(args, ctx.procs, ctx.scheduler),
        SYS_SHMGET => sys_shmget(args, ctx.procs),
        SYS_SHMAT => sys_shmat(args, ctx.procs),
        SYS_SHMCTL => sys_shmctl(args, ctx.procs),
        SYS_SHMDT => sys_shmdt(args, ctx.procs),
        _ => return None,
    })
}
