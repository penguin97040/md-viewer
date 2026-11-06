//! Windows: COMPILE-ONLY. A named pipe per user and session, open to this user only, refusing remote
//! clients. This file has never run on a Windows machine.
//!
//! A local process that creates the pipe name first could read launch paths. `lock_path` is in the
//! user's private AppData and decides ownership, and an owner that cannot create the first instance
//! runs standalone. A client still cannot tell a squatter from the owner; that gap remains.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree, INVALID_HANDLE_VALUE};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

use super::sanitise;

fn session() -> String {
    let mut id = 0u32;
    // SAFETY: `id` is a valid out-pointer for the call.
    let _ = unsafe { ProcessIdToSessionId(std::process::id(), &mut id) };
    let user = std::env::var("USERNAME").unwrap_or_default();
    format!("{}-{id}", sanitise(&user))
}

fn pipe_name() -> Vec<u16> {
    format!(r"\\.\pipe\md-viewer-1-{}", session()).encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn lock_path() -> io::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no LOCALAPPDATA"))?;
    let dir = PathBuf::from(base).join("md-viewer-1");
    fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("{}.lock", session())))
}

fn create_instance(first: bool) -> io::Result<HANDLE> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // Protected DACL: full access for the owner, nobody else.
    // SAFETY: the SDDL string is static and `descriptor` is a valid out-pointer.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(w!("D:P(A;;GA;;;OW)"), SDDL_REVISION_1, &mut descriptor, None)
    }
    .map_err(io::Error::from)?;
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let name = pipe_name();
    let mode = if first { PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_DUPLEX };
    // SAFETY: `name` is NUL-terminated and `attrs` outlives the call.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            Some(&attrs),
        )
    };
    // SAFETY: the descriptor came from the conversion above and is not used again.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

pub struct Listener {
    next: std::sync::Mutex<Option<isize>>,
}

impl Listener {
    pub fn bind() -> io::Result<Self> {
        let first = create_instance(true)?;
        Ok(Self { next: std::sync::Mutex::new(Some(first.0 as isize)) })
    }

    pub fn accept(&self) -> io::Result<Conn> {
        let pending = self.next.lock().unwrap_or_else(|e| e.into_inner()).take();
        let handle = match pending {
            Some(h) => HANDLE(h as *mut _),
            None => create_instance(false)?,
        };
        // SAFETY: `handle` is a live pipe instance, used synchronously.
        if let Err(e) = unsafe { ConnectNamedPipe(handle, None) } {
            if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                // SAFETY: the handle is ours and is not used after this.
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(io::Error::from(e));
            }
        }
        Ok(Conn::Server(handle.0 as isize))
    }
}

pub enum Conn {
    Server(isize),
    Client(File),
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Client(f) => f.read(buf),
            Conn::Server(h) => {
                let mut n = 0u32;
                // SAFETY: the handle is live for the life of `Conn`, and the buffer is valid.
                unsafe { ReadFile(HANDLE(*h as *mut _), Some(buf), Some(&mut n), None) }.map_err(io::Error::from)?;
                Ok(n as usize)
            }
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Client(f) => f.write(buf),
            Conn::Server(h) => {
                let mut n = 0u32;
                // SAFETY: the handle is live for the life of `Conn`, and the buffer is valid.
                unsafe { WriteFile(HANDLE(*h as *mut _), Some(buf), Some(&mut n), None) }.map_err(io::Error::from)?;
                Ok(n as usize)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Client(f) => f.flush(),
            Conn::Server(_) => Ok(()),
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Conn::Server(h) = self {
            let handle = HANDLE(*h as *mut _);
            // SAFETY: the handle is ours and is not used after this.
            unsafe {
                let _ = DisconnectNamedPipe(handle);
                let _ = CloseHandle(handle);
            }
        }
    }
}

pub fn connect() -> io::Result<Conn> {
    let name = String::from_utf16_lossy(&pipe_name()[..pipe_name().len() - 1]);
    Ok(Conn::Client(OpenOptions::new().read(true).write(true).open(name)?))
}

/// Windows only lets the foreground process hand over the foreground, and that is this launch.
pub fn allow_foreground(owner_pid: u32) {
    // SAFETY: plain value; failure only means the owner's window flashes instead of coming forward.
    unsafe {
        let _ = AllowSetForegroundWindow(owner_pid);
    }
}

pub fn path_bytes(p: &Path) -> Vec<u8> {
    p.to_string_lossy().into_owned().into_bytes()
}

pub fn path_from_bytes(b: &[u8]) -> io::Result<PathBuf> {
    std::str::from_utf8(b).map(PathBuf::from).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "path is not UTF-8"))
}

pub fn is_remote_path(p: &Path) -> bool {
    // Either slash counts, as it does for Windows: `/\\server\share` is a network path too.
    let s = p.as_os_str().to_string_lossy().replace('/', "\\");
    s.starts_with(r"\\")
}
