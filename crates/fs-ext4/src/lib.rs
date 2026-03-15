#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::{max, min};
use core::error::Error;
use core::fmt::{self, Display, Formatter};
use ext4_view::{Ext4, Ext4Error, Ext4Read, FileType};
use hal_api::HalBlockDevice;

const EIO: i32 = 5;
const ENOENT: i32 = 2;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const ENODEV: i32 = 19;
const ENOTSUP: i32 = 95;
const MAX_RANGE_READ: usize = 256 * 1024;

#[derive(Clone)]
pub struct Ext4Mount {
    fs: Ext4,
    label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ext4NodeKind {
    Regular,
    Directory,
    Symlink,
    Special,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ext4FileStat {
    pub mode: u32,
    pub size: u64,
    pub nlink: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ext4DirEntry {
    pub name: String,
    pub stat: Ext4FileStat,
    pub kind: Ext4NodeKind,
}

impl Ext4Mount {
    pub fn probe(device: &'static dyn HalBlockDevice) -> Result<Self, i32> {
        device.init().map_err(normalize_device_error)?;
        if !device.is_ready() {
            return Err(ENODEV);
        }
        if device.sector_size() == 0 || device.sector_count() == 0 {
            return Err(ENODEV);
        }
        let fs = load_fs(device).map_err(map_ext4_error)?;
        let label = format!("{}", fs.label().display());
        Ok(Self { fs, label })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn stat(&self, path: &str) -> Result<Ext4FileStat, i32> {
        let metadata = self.fs.metadata(path).map_err(map_ext4_error)?;
        Ok(metadata_to_stat(&metadata))
    }

    pub fn is_dir(&self, path: &str) -> Result<bool, i32> {
        let metadata = self.fs.metadata(path).map_err(map_ext4_error)?;
        Ok(metadata.is_dir())
    }

    pub fn exists(&self, path: &str) -> Result<bool, i32> {
        self.fs.exists(path).map_err(map_ext4_error)
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, i32> {
        self.fs.read(path).map_err(map_ext4_error)
    }

    pub fn read_detailed(&self, path: &str) -> Result<Vec<u8>, String> {
        self.fs.read(path).map_err(|err| err.to_string())
    }

    pub fn read_range(&self, path: &str, offset: usize, len: usize) -> Result<Vec<u8>, i32> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let mut file = self.fs.open(path).map_err(map_ext4_error)?;
        if let Err(err) = file.seek_to(offset as u64) {
            let code = map_ext4_error(err);
            return if code == EINVAL {
                Ok(Vec::new())
            } else {
                Err(code)
            };
        }
        let mut out = Vec::new();
        while out.len() < len {
            let chunk_len = (len - out.len()).min(MAX_RANGE_READ);
            let mut chunk = vec![0u8; chunk_len];
            let mut filled = 0usize;
            while filled < chunk_len {
                let read = file
                    .read_bytes(&mut chunk[filled..chunk_len])
                    .map_err(map_ext4_error)?;
                if read == 0 {
                    chunk.truncate(filled);
                    out.extend_from_slice(&chunk);
                    return Ok(out);
                }
                filled += read;
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    pub fn read_link(&self, path: &str) -> Result<String, i32> {
        let target = self.fs.read_link(path).map_err(map_ext4_error)?;
        Ok(format!("{}", target.display()))
    }

    pub fn read_dir(&self, path: &str) -> Result<Vec<Ext4DirEntry>, i32> {
        let mut out = Vec::new();
        for entry in self.fs.read_dir(path).map_err(map_ext4_error)? {
            let entry = entry.map_err(map_ext4_error)?;
            let metadata = entry.metadata().map_err(map_ext4_error)?;
            let kind = file_type_to_kind(entry.file_type().map_err(map_ext4_error)?);
            out.push(Ext4DirEntry {
                name: format!("{}", entry.file_name().display()),
                stat: metadata_to_stat(&metadata),
                kind,
            });
        }
        Ok(out)
    }
}

fn load_fs(device: &'static dyn HalBlockDevice) -> Result<Ext4, Ext4Error> {
    Ext4::load(Box::new(BlockDeviceReader::new(device)))
}

fn metadata_to_stat(metadata: &ext4_view::Metadata) -> Ext4FileStat {
    let mode = match metadata.file_type() {
        FileType::Directory => 0o040000,
        FileType::Regular => 0o100000,
        FileType::Symlink => 0o120000,
        FileType::CharacterDevice => 0o020000,
        FileType::BlockDevice => 0o060000,
        FileType::Fifo => 0o010000,
        FileType::Socket => 0o140000,
    } | u32::from(metadata.mode());
    Ext4FileStat {
        mode,
        size: metadata.len(),
        nlink: 1,
    }
}

fn file_type_to_kind(file_type: FileType) -> Ext4NodeKind {
    match file_type {
        FileType::Regular => Ext4NodeKind::Regular,
        FileType::Directory => Ext4NodeKind::Directory,
        FileType::Symlink => Ext4NodeKind::Symlink,
        FileType::CharacterDevice | FileType::BlockDevice | FileType::Fifo | FileType::Socket => {
            Ext4NodeKind::Special
        }
    }
}

fn map_ext4_error(err: Ext4Error) -> i32 {
    match err {
        Ext4Error::NotFound => ENOENT,
        Ext4Error::NotADirectory => ENOTDIR,
        Ext4Error::IsADirectory => EISDIR,
        Ext4Error::MalformedPath
        | Ext4Error::NotAbsolute
        | Ext4Error::NotASymlink
        | Ext4Error::PathTooLong
        | Ext4Error::TooManySymlinks
        | Ext4Error::FileTooLarge
        | Ext4Error::IsASpecialFile => EINVAL,
        Ext4Error::Io(_) => EIO,
        Ext4Error::Corrupt(_)
        | Ext4Error::Incompatible(_)
        | Ext4Error::Encrypted
        | Ext4Error::NotUtf8 => ENOTSUP,
        _ => ENOTSUP,
    }
}

fn normalize_device_error(err: i32) -> i32 {
    if err == 0 {
        EIO
    } else {
        err
    }
}

struct BlockDeviceReader {
    device: &'static dyn HalBlockDevice,
    scratch: Vec<u8>,
}

impl BlockDeviceReader {
    fn new(device: &'static dyn HalBlockDevice) -> Self {
        let scratch_len = device.sector_size().max(512);
        Self {
            device,
            scratch: vec![0; scratch_len],
        }
    }
}

impl Ext4Read for BlockDeviceReader {
    fn read(
        &mut self,
        start_byte: u64,
        dst: &mut [u8],
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let sector_size = self.device.sector_size();
        if sector_size == 0 || self.scratch.len() != sector_size {
            return Err(Box::new(BlockIoError::InvalidGeometry));
        }

        let start = usize::try_from(start_byte).map_err(|_| {
            Box::new(BlockIoError::AddressOverflow) as Box<dyn Error + Send + Sync + 'static>
        })?;
        let end = start.checked_add(dst.len()).ok_or_else(|| {
            Box::new(BlockIoError::AddressOverflow) as Box<dyn Error + Send + Sync + 'static>
        })?;
        let first_sector = start / sector_size;
        let last_sector = end.saturating_sub(1) / sector_size;

        if start % sector_size == 0 && dst.len() % sector_size == 0 {
            self.device
                .read_sectors(first_sector, dst)
                .map_err(|code| {
                    Box::new(BlockIoError::Device {
                        code,
                        sector: first_sector,
                        start_byte,
                        read_len: dst.len(),
                    }) as Box<dyn Error + Send + Sync + 'static>
                })?;
            return Ok(());
        }

        for sector in first_sector..=last_sector {
            self.device
                .read_sector(sector, &mut self.scratch)
                .map_err(|code| {
                    Box::new(BlockIoError::Device {
                        code,
                        sector,
                        start_byte,
                        read_len: dst.len(),
                    }) as Box<dyn Error + Send + Sync + 'static>
                })?;
            let sector_start = sector * sector_size;
            let copy_start = max(start, sector_start);
            let copy_end = min(end, sector_start + sector_size);
            let dst_start = copy_start - start;
            let src_start = copy_start - sector_start;
            let len = copy_end - copy_start;
            dst[dst_start..dst_start + len]
                .copy_from_slice(&self.scratch[src_start..src_start + len]);
        }

        Ok(())
    }
}

#[derive(Debug)]
enum BlockIoError {
    AddressOverflow,
    InvalidGeometry,
    Device {
        code: i32,
        sector: usize,
        start_byte: u64,
        read_len: usize,
    },
}

impl Display for BlockIoError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::AddressOverflow => write!(f, "block read offset overflow"),
            Self::InvalidGeometry => write!(f, "invalid block device geometry"),
            Self::Device {
                code,
                sector,
                start_byte,
                read_len,
            } => write!(
                f,
                "block device read failed with {} at sector {} (byte offset {}, len {})",
                code, sector, start_byte, read_len
            ),
        }
    }
}

impl Error for BlockIoError {}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::{Ext4Mount, Ext4NodeKind};
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicBool, Ordering};
    use hal_api::HalBlockDevice;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct VecBlockDevice {
        ready: AtomicBool,
        data: Vec<u8>,
        sector_size: usize,
    }

    impl VecBlockDevice {
        fn from_image(path: &Path) -> Self {
            Self {
                ready: AtomicBool::new(false),
                data: fs::read(path).unwrap(),
                sector_size: 512,
            }
        }
    }

    impl HalBlockDevice for VecBlockDevice {
        fn name(&self) -> &'static str {
            "vecblk0"
        }

        fn init(&self) -> Result<(), i32> {
            self.ready.store(true, Ordering::Relaxed);
            Ok(())
        }

        fn is_ready(&self) -> bool {
            self.ready.load(Ordering::Relaxed)
        }

        fn sector_size(&self) -> usize {
            self.sector_size
        }

        fn sector_count(&self) -> usize {
            self.data.len() / self.sector_size
        }

        fn read_sector(&self, sector: usize, buf: &mut [u8]) -> Result<(), i32> {
            let start = sector * self.sector_size;
            let end = start + buf.len();
            buf.copy_from_slice(self.data.get(start..end).ok_or(5)?);
            Ok(())
        }

        fn write_sector(&self, _sector: usize, _buf: &[u8]) -> Result<(), i32> {
            Err(95)
        }
    }

    fn fresh_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("whuse-{}-{}-{}", name, std::process::id(), stamp));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn build_test_image() -> PathBuf {
        let base = fresh_dir("ext4");
        let stage = base.join("stage");
        fs::create_dir_all(stage.join("bin")).unwrap();
        fs::create_dir_all(stage.join("etc")).unwrap();
        fs::write(stage.join("bin/hello"), b"hello ext4").unwrap();
        fs::write(stage.join("etc/issue"), b"whuse ext4").unwrap();
        symlink("/etc/issue", stage.join("etc/issue.link")).unwrap();

        let image = base.join("rootfs.ext4");
        let status = Command::new("truncate")
            .args(["-s", "8M", image.to_str().unwrap()])
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("mke2fs")
            .args([
                "-t",
                "ext4",
                "-d",
                stage.to_str().unwrap(),
                "-F",
                image.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        image
    }

    #[test]
    fn probe_and_read_file_from_ext4_image() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));
        let mount = Ext4Mount::probe(device).unwrap();

        assert!(mount.exists("/bin/hello").unwrap());
        assert_eq!(mount.read("/bin/hello").unwrap(), b"hello ext4");
        assert_eq!(mount.read_range("/bin/hello", 6, 4).unwrap(), b"ext4");
        assert!(mount.label().is_empty() || !mount.label().contains('\0'));
    }

    #[test]
    fn read_dir_and_symlink_from_ext4_image() {
        let image = build_test_image();
        let device =
            std::boxed::Box::leak(std::boxed::Box::new(VecBlockDevice::from_image(&image)));
        let mount = Ext4Mount::probe(device).unwrap();

        let entries = mount.read_dir("/etc").unwrap();
        assert!(entries.iter().any(|entry| entry.name == "issue"));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == Ext4NodeKind::Symlink));
        assert_eq!(mount.read_link("/etc/issue.link").unwrap(), "/etc/issue");
        assert!(mount.is_dir("/etc").unwrap());
    }
}
