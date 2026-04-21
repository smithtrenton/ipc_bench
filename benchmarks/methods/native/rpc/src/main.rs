use std::{
    error::Error,
    ffi::{CString, c_char, c_void},
    io::{self, Write},
    ptr,
};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, hold_until_stdin_closes, run_benchmark};

const ENV_RPC_ENDPOINT: &str = "IPC_BENCH_RPC_ENDPOINT";

unsafe extern "C" {
    fn rpc_server_start(endpoint: *const c_char) -> i32;
    fn rpc_server_stop() -> i32;
    fn rpc_client_connect(endpoint: *const c_char, binding: *mut *mut c_void) -> i32;
    fn rpc_client_disconnect(binding: *mut *mut c_void);
    fn rpc_client_roundtrip(
        binding: *mut c_void,
        request: *const u8,
        response: *mut u8,
        length: u32,
    ) -> i32;
}

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
        ProcessRole::Child => run_child(config),
    }
}

fn run_parent(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let endpoint = format!("ipc-bench-rpc-{}", std::process::id());
    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[(ENV_RPC_ENDPOINT, endpoint.clone())],
    )?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    let endpoint = CString::new(endpoint)?;
    let mut binding = ptr::null_mut();
    rpc_status(
        unsafe { rpc_client_connect(endpoint.as_ptr(), &mut binding) },
        "rpc_client_connect",
    )?;

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("rpc", &config, true, || {
        rpc_status(
            unsafe {
                rpc_client_roundtrip(
                    binding,
                    outbound.as_ptr(),
                    inbound.as_mut_ptr(),
                    config.message_size as u32,
                )
            },
            "rpc_client_roundtrip",
        )
        .expect("RPC roundtrip should succeed");
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    unsafe {
        rpc_client_disconnect(&mut binding);
    }
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_child(_config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let endpoint = CString::new(std::env::var(ENV_RPC_ENDPOINT)?)?;
    rpc_status(
        unsafe { rpc_server_start(endpoint.as_ptr()) },
        "rpc_server_start",
    )?;

    println!("ready");
    io::stdout().flush()?;
    hold_until_stdin_closes()?;
    rpc_status(unsafe { rpc_server_stop() }, "rpc_server_stop")?;
    Ok(())
}

fn rpc_status(status: i32, label: &str) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{label} failed with RPC status {status}"
        )))
    }
}
