use alloc::vec::Vec;

use axerrno::{LinuxError, LinuxResult};
use axfs_ng_vfs::{Location, Metadata, NodePermission, NodeType};
use axtask::current;
use linux_raw_sys::general::{R_OK, W_OK, X_OK};
use starry_core::task::{AsThread, Credentials};

mod ctl;
mod event;
mod fd_ops;
mod io;
mod memfd;
mod mount;
mod pidfd;
mod pipe;
mod stat;

#[derive(Debug, Clone, Copy)]
pub(super) struct FsIdentity {
    uid: u32,
    gid: u32,
    is_root: bool,
}

fn current_credentials() -> Credentials {
    current().as_thread().proc_data.credentials()
}

pub(super) fn effective_identity() -> FsIdentity {
    let creds = current_credentials();
    FsIdentity {
        uid: creds.euid,
        gid: creds.egid,
        is_root: creds.euid == 0,
    }
}

pub(super) fn real_identity() -> FsIdentity {
    let creds = current_credentials();
    FsIdentity {
        uid: creds.ruid,
        gid: creds.rgid,
        is_root: creds.ruid == 0,
    }
}

pub(super) fn caller_has_group(gid: u32) -> bool {
    current().as_thread().proc_data.has_group(gid)
}

pub(super) fn caller_is_owner_or_root(meta: &Metadata) -> bool {
    let caller = effective_identity();
    caller.is_root || caller.uid == meta.uid
}

pub(super) fn require_access(
    meta: &Metadata,
    identity: FsIdentity,
    requested_mode: u32,
) -> LinuxResult<()> {
    if requested_mode == 0 {
        return Ok(());
    }

    if identity.is_root {
        if requested_mode & X_OK == 0 || meta.node_type == NodeType::Directory {
            return Ok(());
        }
        let any_exec = meta.mode.intersects(
            NodePermission::OWNER_EXEC | NodePermission::GROUP_EXEC | NodePermission::OTHER_EXEC,
        );
        return if any_exec {
            Ok(())
        } else {
            Err(LinuxError::EACCES)
        };
    }

    let (read_bit, write_bit, exec_bit) = if identity.uid == meta.uid {
        (
            NodePermission::OWNER_READ,
            NodePermission::OWNER_WRITE,
            NodePermission::OWNER_EXEC,
        )
    } else if identity.gid == meta.gid {
        (
            NodePermission::GROUP_READ,
            NodePermission::GROUP_WRITE,
            NodePermission::GROUP_EXEC,
        )
    } else {
        (
            NodePermission::OTHER_READ,
            NodePermission::OTHER_WRITE,
            NodePermission::OTHER_EXEC,
        )
    };

    let granted = (requested_mode & R_OK == 0 || meta.mode.contains(read_bit))
        && (requested_mode & W_OK == 0 || meta.mode.contains(write_bit))
        && (requested_mode & X_OK == 0 || meta.mode.contains(exec_bit));

    if granted {
        Ok(())
    } else {
        Err(LinuxError::EACCES)
    }
}

pub(super) fn require_search_path(
    loc: &Location,
    identity: FsIdentity,
    include_target: bool,
) -> LinuxResult<()> {
    let mut chain = Vec::new();
    let mut cursor = if include_target {
        Some(loc.clone())
    } else {
        loc.parent()
    };
    while let Some(dir) = cursor {
        chain.push(dir.clone());
        cursor = dir.parent();
    }
    for dir in chain.iter().rev() {
        let meta = dir.metadata()?;
        if meta.node_type != NodeType::Directory {
            return Err(LinuxError::ENOTDIR);
        }
        require_access(&meta, identity, X_OK)?;
    }
    Ok(())
}

pub(super) fn require_parent_write_search(
    parent: &Location,
    identity: FsIdentity,
) -> LinuxResult<()> {
    require_search_path(parent, identity, true)?;
    require_access(&parent.metadata()?, identity, W_OK)
}

pub(super) fn created_file_gid(parent: &Location, fallback_gid: u32) -> LinuxResult<u32> {
    let meta = parent.metadata()?;
    Ok(if meta.mode.contains(NodePermission::SET_GID) {
        meta.gid
    } else {
        fallback_gid
    })
}

pub(super) fn inherited_dir_attrs(
    parent: &Location,
    fallback_gid: u32,
    mut mode: NodePermission,
) -> LinuxResult<(u32, NodePermission)> {
    let meta = parent.metadata()?;
    let gid = if meta.mode.contains(NodePermission::SET_GID) {
        mode.insert(NodePermission::SET_GID);
        meta.gid
    } else {
        fallback_gid
    };
    Ok((gid, mode))
}

pub(super) fn require_sticky_unlink(parent: &Metadata, target: &Metadata) -> LinuxResult<()> {
    let caller = effective_identity();
    if caller.is_root
        || !parent.mode.contains(NodePermission::STICKY)
        || caller.uid == parent.uid
        || caller.uid == target.uid
    {
        Ok(())
    } else {
        Err(LinuxError::EPERM)
    }
}

pub(crate) use self::io::DummyFd;
pub use self::{
    ctl::*, event::*, fd_ops::*, io::*, memfd::*, mount::*, pidfd::*, pipe::*, stat::*,
};
