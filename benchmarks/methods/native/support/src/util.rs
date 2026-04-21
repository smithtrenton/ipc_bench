use std::{
    ffi::CString,
    io, thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{ReadFile, WriteFile},
    System::Threading::{INFINITE, ReleaseSemaphore, ResetEvent, SetEvent, WaitForSingleObject},
};

pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub fn from_handle(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    pub fn from_file_handle(handle: HANDLE) -> io::Result<Self> {
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub fn unique_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{prefix}-{}-{nanos}", std::process::id())
}

pub fn c_string(value: &str) -> io::Result<CString> {
    CString::new(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains interior nul"))
}

pub fn retry_with_backoff<F, T>(attempts: usize, delay: Duration, mut action: F) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut last_error = None;

    for _ in 0..attempts {
        match action() {
            Ok(value) => return Ok(value),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(delay);
            }
        }
    }

    Err(last_error.unwrap_or_else(io::Error::last_os_error))
}

pub fn write_all_handle(handle: HANDLE, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let chunk_len = u32::try_from(buf.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "buffer too large"))?;
        let mut written = 0_u32;
        let ok = unsafe {
            WriteFile(
                handle,
                buf.as_ptr(),
                chunk_len,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "zero bytes written to handle",
            ));
        }
        buf = &buf[written as usize..];
    }

    Ok(())
}

pub fn read_exact_handle(handle: HANDLE, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let chunk_len = u32::try_from(buf.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "buffer too large"))?;
        let mut read = 0_u32;
        let ok = unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr(),
                chunk_len,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "handle closed while reading",
            ));
        }
        let (_, rest) = buf.split_at_mut(read as usize);
        buf = rest;
    }

    Ok(())
}

pub fn wait_for_signal(handle: HANDLE) -> io::Result<()> {
    let status = unsafe { WaitForSingleObject(handle, INFINITE) };
    match status {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_FAILED => Err(io::Error::last_os_error()),
        _ => Err(io::Error::other(format!("unexpected wait status {status}"))),
    }
}

pub fn set_event(handle: HANDLE) -> io::Result<()> {
    let ok = unsafe { SetEvent(handle) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn reset_event(handle: HANDLE) -> io::Result<()> {
    let ok = unsafe { ResetEvent(handle) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn release_semaphore(handle: HANDLE) -> io::Result<()> {
    let ok = unsafe { ReleaseSemaphore(handle, 1, std::ptr::null_mut()) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn is_pipe_closed(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(109 | 232 | 233))
}

pub fn win32_last_error() -> io::Error {
    let code = unsafe { GetLastError() } as i32;
    io::Error::from_raw_os_error(code)
}

pub unsafe fn slice_from_raw_parts_mut<'a>(ptr: *mut u8, len: usize) -> &'a mut [u8] {
    unsafe { std::slice::from_raw_parts_mut(ptr, len) }
}

pub unsafe fn slice_from_raw_parts<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

pub fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
