use std::{
    error::Error,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, run_benchmark};

pub fn run_tcp_loopback() -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_parent(config),
        ProcessRole::Child => run_child(config),
    }
}

fn run_parent(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let mut child = ManagedChild::spawn_self(&config.child_args())?;
    let readiness = child.wait_for_ready()?;
    let port = parse_ready_port(&readiness)?;
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_nodelay(true)?;

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("tcp-loopback", &config, true, || {
        stream
            .write_all(&outbound)
            .expect("parent should write full request");
        stream.flush().expect("parent should flush request");
        stream
            .read_exact(&mut inbound)
            .expect("parent should read full response");
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    drop(stream);
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_child(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    println!("ready:{port}");
    io::stdout().flush()?;

    let (mut stream, _) = listener.accept()?;
    stream.set_nodelay(true)?;
    let mut buf = vec![0_u8; config.message_size];

    loop {
        match stream.read_exact(&mut buf) {
            Ok(()) => {
                if !buf.is_empty() {
                    buf[0] = buf[0].wrapping_add(1);
                }
                stream.write_all(&buf)?;
                stream.flush()?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
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
