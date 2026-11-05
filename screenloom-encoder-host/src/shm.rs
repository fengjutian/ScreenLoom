//! 命名共享内存：父进程创建，子进程只读映射。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// 一块固定槽数的帧缓冲。
pub struct Shm {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    slot_size: usize,
    slots: u32,
}

impl Shm {
    /// 父进程创建（可写）。
    pub fn create(name: &str, slots: u32, slot_size: usize) -> Result<Shm, String> {
        let total = slots as usize * slot_size;
        let wname = wide(name);
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                total as u32,
                PCWSTR(wname.as_ptr()),
            )
        }
        .map_err(|e| format!("CreateFileMapping({name}) 失败: {e}"))?;
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, total) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err("MapViewOfFile 返回空指针".into());
        }
        Ok(Shm {
            handle,
            view,
            slot_size,
            slots,
        })
    }

    /// 子进程打开（只读）。
    pub fn open(name: &str, slots: u32, slot_size: usize) -> Result<Shm, String> {
        let wname = wide(name);
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(wname.as_ptr())) }
            .map_err(|e| format!("OpenFileMapping({name}) 失败: {e}"))?;
        let total = slots as usize * slot_size;
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, total) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err("MapViewOfFile 返回空指针".into());
        }
        Ok(Shm {
            handle,
            view,
            slot_size,
            slots,
        })
    }

    /// 父进程写入一个槽。
    pub fn write_slot(&self, slot: u32, data: &[u8]) {
        assert!((slot as usize) < self.slots as usize, "槽位越界");
        assert!(data.len() <= self.slot_size, "帧超过槽容量");
        // SAFETY: 槽位与长度都校验过，映射区至少 slots*slot_size 字节。
        unsafe {
            let dst = (self.view.Value as *mut u8).add(slot as usize * self.slot_size);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
    }

    /// 子进程读取一个槽的前 `len` 字节。
    pub fn slot(&self, slot: u32, len: usize) -> Result<&[u8], String> {
        if slot as usize >= self.slots as usize {
            return Err(format!("槽位越界: {slot}"));
        }
        if len > self.slot_size {
            return Err(format!("帧长度越界: {len} > {}", self.slot_size));
        }
        // SAFETY: 同上，长度校验过。
        Ok(unsafe {
            std::slice::from_raw_parts(
                (self.view.Value as *const u8).add(slot as usize * self.slot_size),
                len,
            )
        })
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
