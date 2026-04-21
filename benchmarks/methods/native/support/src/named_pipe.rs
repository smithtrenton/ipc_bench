use std::{
    error::Error,
    io::{self, Write},
    mem::zeroed,
    time::Duration,
};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, run_benchmark};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE},
    Storage::FileSystem::{
        CreateFileA, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ,
        FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    },
    System::{
        IO::{GetOverlappedResult, OVERLAPPED},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeA, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_TYPE_MESSAGE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        },
        Threading::CreateEventA,
    },
};

use crate::util::{
    OwnedHandle, c_string, is_pipe_closed, read_exact_handle, reset_event, retry_with_backoff,
    unique_name, wait_for_signal, win32_last_error, write_all_handle,
};

const ENV_PIPE_NAME: &str = "IPC_BENCH_PIPE_NAME";

#[derive(Clone, Copy)]
pub enum NamedPipeKind {
    ByteSync,
    MessageSync,
    Overlapped,
}

pub fn run_named_pipe(kind: NamedPipeKind) -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_parent(config, kind),
        ProcessRole::Child => run_child(config, kind),
    }
}

fn run_parent(config: BenchmarkConfig, kind: NamedPipeKind) -> Result<(), Box<dyn Error>> {
    let pipe_name = format!(r"\\.\pipe\{}", unique_name("ipc-bench"));
    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[(ENV_PIPE_NAME, pipe_name.clone())],
    )?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    let client = open_client(&pipe_name, kind)?;
    let method_name = match kind {
        NamedPipeKind::ByteSync => "named-pipe-byte-sync",
        NamedPipeKind::MessageSync => "named-pipe-message-sync",
        NamedPipeKind::Overlapped => "named-pipe-overlapped",
    };

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let client_handle = client.raw();
    let report = match kind {
        NamedPipeKind::ByteSync | NamedPipeKind::MessageSync => {
            run_benchmark(method_name, &config, true, || {
                write_all_handle(client_handle, &outbound).expect("pipe write should succeed");
                read_exact_handle(client_handle, &mut inbound).expect("pipe read should succeed");
                if !outbound.is_empty() {
                    outbound.copy_from_slice(&inbound);
                    outbound[0] = outbound[0].wrapping_add(1);
                }
            })
        }
        NamedPipeKind::Overlapped => {
            let mut write_ctx = OverlappedContext::new()?;
            let mut read_ctx = OverlappedContext::new()?;
            run_benchmark(method_name, &config, true, || {
                write_all_overlapped(client_handle, &mut write_ctx, &outbound)
                    .expect("overlapped pipe write should succeed");
                read_exact_overlapped(client_handle, &mut read_ctx, &mut inbound)
                    .expect("overlapped pipe read should succeed");
                if !outbound.is_empty() {
                    outbound.copy_from_slice(&inbound);
                    outbound[0] = outbound[0].wrapping_add(1);
                }
            })
        }
    };

    drop(client);
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_child(config: BenchmarkConfig, kind: NamedPipeKind) -> Result<(), Box<dyn Error>> {
    let pipe_name = std::env::var(ENV_PIPE_NAME)?;
    let server = create_server(&pipe_name, kind)?;
    println!("ready");
    io::stdout().flush()?;

    match kind {
        NamedPipeKind::ByteSync | NamedPipeKind::MessageSync => {
            connect_sync(server.raw())?;
            echo_loop_sync(server.raw(), config.message_size)?;
        }
        NamedPipeKind::Overlapped => {
            let mut connect_ctx = OverlappedContext::new()?;
            connect_overlapped(server.raw(), &mut connect_ctx)?;
            echo_loop_overlapped(server.raw(), config.message_size)?;
        }
    }

    unsafe {
        DisconnectNamedPipe(server.raw());
    }
    Ok(())
}

fn create_server(pipe_name: &str, kind: NamedPipeKind) -> io::Result<OwnedHandle> {
    let pipe_name = c_string(pipe_name)?;
    let (access_mode, pipe_mode) = match kind {
        NamedPipeKind::ByteSync => (
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
        ),
        NamedPipeKind::MessageSync => (
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
        ),
        NamedPipeKind::Overlapped => (
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
        ),
    };
    let handle = unsafe {
        CreateNamedPipeA(
            pipe_name.as_ptr().cast(),
            access_mode,
            pipe_mode,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            std::ptr::null(),
        )
    };
    OwnedHandle::from_file_handle(handle)
}

fn open_client(pipe_name: &str, kind: NamedPipeKind) -> io::Result<OwnedHandle> {
    let pipe_name = pipe_name.to_owned();
    retry_with_backoff(200, Duration::from_millis(10), || {
        let pipe_name = c_string(&pipe_name)?;
        let flags = match kind {
            NamedPipeKind::Overlapped => FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            _ => FILE_ATTRIBUTE_NORMAL,
        };
        let handle = unsafe {
            CreateFileA(
                pipe_name.as_ptr().cast(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                FILE_SHARE_NONE,
                std::ptr::null(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            )
        };
        OwnedHandle::from_file_handle(handle)
    })
}

fn connect_sync(server: HANDLE) -> io::Result<()> {
    let ok = unsafe { ConnectNamedPipe(server, std::ptr::null_mut()) };
    if ok != 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) {
            Ok(())
        } else {
            Err(error)
        }
    }
}

fn echo_loop_sync(server: HANDLE, message_size: usize) -> io::Result<()> {
    let mut buf = vec![0_u8; message_size];
    loop {
        match read_exact_handle(server, &mut buf) {
            Ok(()) => {
                if !buf.is_empty() {
                    buf[0] = buf[0].wrapping_add(1);
                }
                write_all_handle(server, &buf)?;
            }
            Err(error)
                if is_pipe_closed(&error) || error.kind() == io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
}

fn echo_loop_overlapped(server: HANDLE, message_size: usize) -> io::Result<()> {
    let mut buf = vec![0_u8; message_size];
    let mut read_ctx = OverlappedContext::new()?;
    let mut write_ctx = OverlappedContext::new()?;

    loop {
        match read_exact_overlapped(server, &mut read_ctx, &mut buf) {
            Ok(()) => {
                if !buf.is_empty() {
                    buf[0] = buf[0].wrapping_add(1);
                }
                write_all_overlapped(server, &mut write_ctx, &buf)?;
            }
            Err(error)
                if is_pipe_closed(&error) || error.kind() == io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
}

struct OverlappedContext {
    event: OwnedHandle,
    overlapped: OVERLAPPED,
}

impl OverlappedContext {
    fn new() -> io::Result<Self> {
        let event = unsafe { CreateEventA(std::ptr::null(), 1, 0, std::ptr::null()) };
        let event = OwnedHandle::from_handle(event)?;
        let mut overlapped = unsafe { zeroed::<OVERLAPPED>() };
        overlapped.hEvent = event.raw();
        Ok(Self { event, overlapped })
    }

    fn reset(&mut self) -> io::Result<()> {
        reset_event(self.event.raw())?;
        self.overlapped = unsafe { zeroed::<OVERLAPPED>() };
        self.overlapped.hEvent = self.event.raw();
        Ok(())
    }
}

fn connect_overlapped(server: HANDLE, ctx: &mut OverlappedContext) -> io::Result<()> {
    ctx.reset()?;
    let ok = unsafe { ConnectNamedPipe(server, &mut ctx.overlapped) };
    if ok != 0 {
        return Ok(());
    }

    match win32_last_error().raw_os_error() {
        Some(code) if code == ERROR_PIPE_CONNECTED as i32 => Ok(()),
        Some(code) if code == ERROR_IO_PENDING as i32 => {
            wait_for_signal(ctx.event.raw())?;
            let mut transferred = 0_u32;
            let ok = unsafe { GetOverlappedResult(server, &ctx.overlapped, &mut transferred, 1) };
            if ok == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
        _ => Err(io::Error::last_os_error()),
    }
}

fn write_all_overlapped(
    handle: HANDLE,
    ctx: &mut OverlappedContext,
    mut buf: &[u8],
) -> io::Result<()> {
    while !buf.is_empty() {
        ctx.reset()?;
        let len = u32::try_from(buf.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "buffer too large"))?;
        let mut transferred = 0_u32;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::WriteFile(
                handle,
                buf.as_ptr(),
                len,
                std::ptr::null_mut(),
                &mut ctx.overlapped,
            )
        };
        if ok == 0 {
            let error = win32_last_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            wait_for_signal(ctx.event.raw())?;
            let ok = unsafe { GetOverlappedResult(handle, &ctx.overlapped, &mut transferred, 1) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
        } else {
            let ok = unsafe { GetOverlappedResult(handle, &ctx.overlapped, &mut transferred, 1) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if transferred == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "zero bytes written to overlapped pipe",
            ));
        }
        buf = &buf[transferred as usize..];
    }

    Ok(())
}

fn read_exact_overlapped(
    handle: HANDLE,
    ctx: &mut OverlappedContext,
    mut buf: &mut [u8],
) -> io::Result<()> {
    while !buf.is_empty() {
        ctx.reset()?;
        let len = u32::try_from(buf.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "buffer too large"))?;
        let mut transferred = 0_u32;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::ReadFile(
                handle,
                buf.as_mut_ptr(),
                len,
                std::ptr::null_mut(),
                &mut ctx.overlapped,
            )
        };
        if ok == 0 {
            let error = win32_last_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            wait_for_signal(ctx.event.raw())?;
            let ok = unsafe { GetOverlappedResult(handle, &ctx.overlapped, &mut transferred, 1) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
        } else {
            let ok = unsafe { GetOverlappedResult(handle, &ctx.overlapped, &mut transferred, 1) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if transferred == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "overlapped pipe closed while reading",
            ));
        }
        let (_, rest) = buf.split_at_mut(transferred as usize);
        buf = rest;
    }

    Ok(())
}
