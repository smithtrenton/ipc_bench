use std::{error::Error, io};

use harness::{BenchmarkConfig, ProcessRole, run_benchmark};

pub fn run_copy_roundtrip() -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    if config.role != ProcessRole::Parent {
        return Err("copy-roundtrip does not use a child role".into());
    }

    let mut outbound = vec![0_u8; config.message_size];
    let mut request = vec![0_u8; config.message_size];
    let mut scratch = vec![0_u8; config.message_size];
    let mut response = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("copy-roundtrip", &config, false, || {
        request.copy_from_slice(&outbound);
        scratch.copy_from_slice(&request);
        if !scratch.is_empty() {
            scratch[0] = scratch[0].wrapping_add(1);
        }
        response.copy_from_slice(&scratch);
        inbound.copy_from_slice(&response);
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    print!("{}", report.render(config.output_format)?);
    Ok(())
}
