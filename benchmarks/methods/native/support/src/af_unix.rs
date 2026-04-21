use std::{error::Error, fs, io, mem::MaybeUninit, path::PathBuf, thread, time::Duration};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, run_benchmark};
use socket2::{Domain, SockAddr, Socket, Type};

const ENV_SOCKET_PATH: &str = "IPC_BENCH_AF_UNIX_PATH";

pub fn run_af_unix() -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_parent(config),
        ProcessRole::Child => run_child(config),
    }
}

fn run_parent(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let socket_path = make_socket_path()?;
    let socket_path_string = socket_path.to_string_lossy().into_owned();

    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[(ENV_SOCKET_PATH, socket_path_string.clone())],
    )?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    let stream = connect_with_retry(&socket_path)?;
    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("af-unix", &config, true, || {
        send_all(&stream, &outbound).expect("AF_UNIX write should succeed");
        recv_exact(&stream, &mut inbound).expect("AF_UNIX read should succeed");
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    drop(stream);
    child.request_shutdown();
    let status = child.wait()?;
    let _ = fs::remove_file(&socket_path);
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_child(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let socket_path = PathBuf::from(std::env::var(ENV_SOCKET_PATH)?);
    let _ = fs::remove_file(&socket_path);
    let listener = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    let sockaddr = SockAddr::unix(&socket_path)?;
    listener.bind(&sockaddr)?;
    listener.listen(1)?;

    println!("ready");
    io::Write::flush(&mut io::stdout())?;

    let (stream, _) = listener.accept()?;
    let mut buf = vec![0_u8; config.message_size];

    loop {
        match recv_exact(&stream, &mut buf) {
            Ok(()) => {
                if !buf.is_empty() {
                    buf[0] = buf[0].wrapping_add(1);
                }
                send_all(&stream, &buf)?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                ) =>
            {
                let _ = fs::remove_file(&socket_path);
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn make_socket_path() -> io::Result<PathBuf> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "ipc-bench-{}.sock",
        crate::util::unique_name("af-unix")
    ));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(&path);
    Ok(path)
}

fn connect_with_retry(path: &PathBuf) -> io::Result<Socket> {
    let sockaddr = SockAddr::unix(path)?;
    let mut last_error = None;

    for _ in 0..200 {
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        match socket.connect(&sockaddr) {
            Ok(()) => return Ok(socket),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    Err(last_error.unwrap_or_else(io::Error::last_os_error))
}

fn send_all(socket: &Socket, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let written = socket.send(buf)?;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "zero bytes written to AF_UNIX socket",
            ));
        }
        buf = &buf[written..];
    }
    Ok(())
}

fn recv_exact(socket: &Socket, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        let uninit = unsafe {
            std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<MaybeUninit<u8>>(), buf.len())
        };
        let read = socket.recv(uninit)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AF_UNIX socket closed while reading",
            ));
        }
        let (_, rest) = buf.split_at_mut(read);
        buf = rest;
    }
    Ok(())
}
