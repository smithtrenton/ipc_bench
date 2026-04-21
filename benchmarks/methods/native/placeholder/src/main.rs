use std::{
    error::Error,
    io::{self, Write},
};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, hold_until_stdin_closes, run_benchmark};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_parent(config),
        ProcessRole::Child => run_child(),
    }
}

fn run_parent(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let mut child = ManagedChild::spawn_self(&config.child_args())?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("placeholder", &config, true, || {
        if outbound.is_empty() {
            std::hint::spin_loop();
            return;
        }

        inbound.copy_from_slice(&outbound);
        outbound.copy_from_slice(&inbound);
        outbound[0] = outbound[0].wrapping_add(1);
    });

    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);

    Ok(())
}

fn run_child() -> Result<(), Box<dyn Error>> {
    println!("ready");
    io::stdout().flush()?;
    hold_until_stdin_closes()?;
    Ok(())
}
