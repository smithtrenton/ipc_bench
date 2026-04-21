use std::{
    error::Error,
    io::{self, Write},
    net::UdpSocket,
};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, run_benchmark};

const ENV_PARENT_PORT: &str = "IPC_BENCH_UDP_PARENT_PORT";
const MAX_UDP_PAYLOAD: usize = 65_507;

pub fn run_udp_loopback() -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    validate_udp_message_size(config.message_size)?;

    match config.role {
        ProcessRole::Parent => run_parent(config),
        ProcessRole::Child => run_child(config),
    }
}

fn validate_udp_message_size(message_size: usize) -> io::Result<()> {
    if message_size > MAX_UDP_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "udp-loopback supports message sizes up to {MAX_UDP_PAYLOAD} bytes, got {message_size}"
            ),
        ));
    }
    Ok(())
}

fn run_parent(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let parent_socket = UdpSocket::bind(("127.0.0.1", 0))?;
    let parent_port = parent_socket.local_addr()?.port();

    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[(ENV_PARENT_PORT, parent_port.to_string())],
    )?;
    let readiness = child.wait_for_ready()?;
    let child_port = parse_ready_port(&readiness)?;

    parent_socket.connect(("127.0.0.1", child_port))?;

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("udp-loopback", &config, true, || {
        parent_socket
            .send(&outbound)
            .expect("UDP request send should succeed");
        parent_socket
            .recv(&mut inbound)
            .expect("UDP response receive should succeed");
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    parent_socket.send(&[0xFF])?;
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_child(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let parent_port = std::env::var(ENV_PARENT_PORT)?.parse::<u16>()?;
    let socket = UdpSocket::bind(("127.0.0.1", 0))?;
    let child_port = socket.local_addr()?.port();
    socket.connect(("127.0.0.1", parent_port))?;

    println!("ready:{child_port}");
    io::stdout().flush()?;

    let mut buf = vec![0_u8; config.message_size];
    loop {
        match socket.recv(&mut buf) {
            Ok(read) => {
                if read == 0 {
                    continue;
                }
                if read == 1 {
                    return Ok(());
                }
                if read != config.message_size {
                    continue;
                }
                if !buf.is_empty() {
                    buf[0] = buf[0].wrapping_add(1);
                }
                socket.send(&buf)?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn parse_ready_port(readiness: &str) -> Result<u16, Box<dyn Error>> {
    let Some(port) = readiness.strip_prefix("ready:") else {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    };
    Ok(port.parse::<u16>()?)
}
