use std::{
    error::Error,
    io::{self, BufReader, Read, Write},
    process::{Command, Stdio},
};

use harness::{BenchmarkConfig, ProcessRole, run_benchmark};

pub fn run_anon_pipe() -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_parent(config),
        ProcessRole::Child => run_child(config),
    }
}

fn run_parent(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let mut child = Command::new(std::env::current_exe()?)
        .args(config.child_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("failed to capture child stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("failed to capture child stdout"))?;
    let mut stdout = BufReader::new(stdout);

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("anon-pipe", &config, false, || {
        stdin
            .write_all(&outbound)
            .expect("parent should write full request");
        stdin.flush().expect("parent should flush request");
        stdout
            .read_exact(&mut inbound)
            .expect("parent should read full response");
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    drop(stdin);
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_child(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut buf = vec![0_u8; config.message_size];

    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                if !buf.is_empty() {
                    buf[0] = buf[0].wrapping_add(1);
                }
                writer.write_all(&buf)?;
                writer.flush()?;
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
}
