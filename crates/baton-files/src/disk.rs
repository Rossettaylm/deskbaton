//! 接收端的磁盘操作：默认接收位置、剩余空间、顶层条目占名。

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use baton_core::files::{EntryKind, numbered};
use baton_core::{Error, Result};

/// 同名候选最多试这么多个。
const MAX_CANDIDATES: u32 = 10_000;

/// 系统「下载」文件夹。
pub fn downloads_dir() -> Result<PathBuf> {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .ok_or_else(|| Error::File("找不到「下载」文件夹".into()))
}

pub fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

fn occupied(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// 在 `dir` 里占下一个不冲突的文件名：正式名与 `.part` 都不存在时，以「不存在才创建」打开 `.part`。
/// 返回正式路径与已打开的 `.part`。
pub fn claim_file(dir: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    for n in 0..MAX_CANDIDATES {
        let path = dir.join(numbered(name, n, EntryKind::File));
        if occupied(&path) {
            continue;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(part_path(&path))
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(ErrorKind::AlreadyExists, "同名文件过多"))
}

/// 在 `dir` 里新建一个不冲突的文件夹。
pub fn claim_dir(dir: &Path, name: &str) -> io::Result<PathBuf> {
    for n in 0..MAX_CANDIDATES {
        let path = dir.join(numbered(name, n, EntryKind::Dir));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(ErrorKind::AlreadyExists, "同名文件夹过多"))
}

/// `path` 所在卷的剩余可用字节；读不到时为 `None`。
pub fn free_space(path: &Path) -> Option<u64> {
    imp::free_space(path)
        .inspect_err(|e| log::info!("读不到 {} 的剩余空间：{e}", path.display()))
        .ok()
}

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub fn free_space(path: &Path) -> io::Result<u64> {
        let c = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "路径含空字节"))?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `c` 为以 NUL 结尾的路径，`stat` 指向足够大的可写内存。
        let rc = unsafe { libc::statvfs(c.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `statvfs` 成功时已填满 `stat`。
        let stat = unsafe { stat.assume_init() };
        #[allow(clippy::unnecessary_cast)]
        Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;

    pub fn free_space(path: &Path) -> io::Result<u64> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut available = 0u64;
        // SAFETY: `wide` 以 NUL 结尾并在调用期间有效；只写入 `available`。
        unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&raw mut available), None, None) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(available)
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
mod imp {
    use std::io;
    use std::path::Path;

    pub fn free_space(_: &Path) -> io::Result<u64> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "当前平台不支持"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_numbered_names_and_skips_parts() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("报告.pdf"), b"old").unwrap();
        fs::write(dir.path().join("报告 (1).pdf.part"), b"").unwrap();
        let (path, _file) = claim_file(dir.path(), "报告.pdf").unwrap();
        assert_eq!(path, dir.path().join("报告 (2).pdf"));
        assert!(dir.path().join("报告 (2).pdf.part").exists());
        let (again, _file) = claim_file(dir.path(), "报告.pdf").unwrap();
        assert_eq!(
            again,
            dir.path().join("报告 (3).pdf"),
            "占过的 .part 不再给出"
        );
    }

    #[test]
    fn claims_numbered_dirs() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            claim_dir(dir.path(), "照片").unwrap(),
            dir.path().join("照片")
        );
        assert_eq!(
            claim_dir(dir.path(), "照片").unwrap(),
            dir.path().join("照片 (1)")
        );
    }

    #[test]
    fn free_space_of_temp_dir_is_known() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_space(dir.path()).is_some_and(|n| n > 0));
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("/x/a.txt")),
            PathBuf::from("/x/a.txt.part")
        );
    }
}
