use std::{
    error::Error,
    io::{self, Write},
    mem::{size_of, zeroed},
    ptr::null_mut,
    sync::OnceLock,
};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, run_benchmark};
use ntapi::ntlpcapi::{
    ALPC_MSGFLG_RELEASE_MESSAGE, ALPC_PORFLG_ALLOW_LPC_REQUESTS, ALPC_PORT_ATTRIBUTES,
    LPC_CONNECTION_REQUEST, LPC_DATAGRAM, LPC_PORT_CLOSED, NtAlpcAcceptConnectPort,
    NtAlpcConnectPort, NtAlpcCreatePort, NtAlpcDisconnectPort, NtAlpcSendWaitReceivePort,
    PORT_MESSAGE,
};
use winapi::{
    shared::ntdef::{HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, OBJECT_ATTRIBUTES, UNICODE_STRING},
    um::winnt::{SECURITY_DYNAMIC_TRACKING, SECURITY_QUALITY_OF_SERVICE, SecurityImpersonation},
};

use crate::util::{OwnedHandle, unique_name};

const ENV_ALPC_PORT_NAME: &str = "IPC_BENCH_ALPC_PORT_NAME";
const LPC_CONNECTION_REPLY: u32 = 11;
const LPC_CANCELED: u32 = 12;
const LPC_CONTINUATION_REQUIRED: u32 = 0x2000;

macro_rules! trace {
    ($($arg:tt)*) => {
        if trace_enabled() {
            eprintln!("[alpc] {}", format_args!($($arg)*));
        }
    };
}

pub fn run_alpc() -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    validate_alpc_message_size(config.message_size)?;

    match config.role {
        ProcessRole::Parent => run_parent(config),
        ProcessRole::Child => run_child(config),
    }
}

fn validate_alpc_message_size(message_size: usize) -> io::Result<()> {
    let max_payload = (i16::MAX as usize).saturating_sub(size_of::<PORT_MESSAGE>());
    if message_size > max_payload {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "alpc supports message sizes up to {max_payload} bytes on this build, got {message_size}"
            ),
        ));
    }
    Ok(())
}

fn run_parent(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let port_name = format!(r"\RPC Control\{}", unique_name("ipc-bench-alpc"));
    trace!("spawning child");
    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[(ENV_ALPC_PORT_NAME, port_name.clone())],
    )?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    trace!("connecting client port");
    let client = connect_port(&port_name, config.message_size)?;
    trace!("waiting for connection reply");
    let mut connection_reply = MessageBuffer::new(0);
    receive_connection_reply(client.raw(), &mut connection_reply)?;
    trace!("connection established");
    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    let mut outbound_message = MessageBuffer::new(config.message_size);
    let mut inbound_message = MessageBuffer::new(config.message_size);
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark("alpc", &config, true, || {
        send_datagram(client.raw(), &outbound, &mut outbound_message)
            .expect("ALPC send should succeed");
        receive_datagram(client.raw(), &mut inbound, &mut inbound_message)
            .expect("ALPC receive should succeed");
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    send_datagram(client.raw(), &[], &mut outbound_message)?;

    disconnect_port(client.raw());
    drop(client);
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_child(config: BenchmarkConfig) -> Result<(), Box<dyn Error>> {
    let port_name = std::env::var(ENV_ALPC_PORT_NAME)?;
    trace!("creating connection port");
    let connection_port = create_connection_port(&port_name, config.message_size)?;

    println!("ready");
    io::stdout().flush()?;

    trace!("accepting connection");
    let mut connection_request = MessageBuffer::new(0);
    let communication_port = accept_connection(
        connection_port.raw(),
        config.message_size,
        &mut connection_request,
    )?;
    trace!("connection accepted");
    let mut inbound_message = MessageBuffer::new(config.message_size);
    let mut outbound_message = MessageBuffer::new(config.message_size);
    let mut response = vec![0_u8; config.message_size];
    loop {
        receive_message(connection_port.raw(), &mut inbound_message)?;
        match inbound_message.kind() {
            LPC_DATAGRAM => {
                let payload = inbound_message.payload();
                if payload.len() != config.message_size {
                    break;
                }
                if !response.is_empty() {
                    response.copy_from_slice(payload);
                    response[0] = response[0].wrapping_add(1);
                }
                send_datagram(communication_port.raw(), &response, &mut outbound_message)?;
            }
            LPC_PORT_CLOSED | LPC_CANCELED => break,
            kind => {
                return Err(
                    io::Error::other(format!("unexpected ALPC message type {kind}")).into(),
                );
            }
        }
    }

    disconnect_port(communication_port.raw());
    disconnect_port(connection_port.raw());
    Ok(())
}

fn create_connection_port(port_name: &str, message_size: usize) -> io::Result<OwnedHandle> {
    let mut port_name = OwnedUnicodeString::new(port_name)?;
    let mut object_attributes = new_object_attributes(port_name.as_mut_ptr());
    let mut port_attributes = new_port_attributes(message_size);
    let mut handle: HANDLE = null_mut();

    ntstatus_result(
        unsafe { NtAlpcCreatePort(&mut handle, &mut object_attributes, &mut port_attributes) },
        "NtAlpcCreatePort",
    )?;
    OwnedHandle::from_handle(handle.cast())
}

fn connect_port(port_name: &str, message_size: usize) -> io::Result<OwnedHandle> {
    let mut port_name = OwnedUnicodeString::new(port_name)?;
    let mut port_attributes = new_port_attributes(message_size);
    let mut handle: HANDLE = null_mut();

    ntstatus_result(
        unsafe {
            NtAlpcConnectPort(
                &mut handle,
                port_name.as_mut_ptr(),
                null_mut(),
                &mut port_attributes,
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        },
        "NtAlpcConnectPort",
    )?;
    OwnedHandle::from_handle(handle.cast())
}

fn accept_connection(
    connection_port: windows_sys::Win32::Foundation::HANDLE,
    message_size: usize,
    connection_request: &mut MessageBuffer,
) -> io::Result<OwnedHandle> {
    receive_message(connection_port, connection_request)?;
    if connection_request.kind() != LPC_CONNECTION_REQUEST {
        return Err(io::Error::other(format!(
            "expected ALPC connection request, got {}",
            connection_request.kind()
        )));
    }

    let reply_key = connection_request
        .reply_key()
        .ok_or_else(|| io::Error::other("connection request missing reply context"))?;
    let mut reply = MessageBuffer::new(0);
    reply.prepare_send(&[], Some(reply_key))?;

    let mut port_attributes = new_port_attributes(message_size);
    let mut handle: HANDLE = null_mut();
    ntstatus_result(
        unsafe {
            NtAlpcAcceptConnectPort(
                &mut handle,
                connection_port.cast(),
                0,
                null_mut(),
                &mut port_attributes,
                null_mut(),
                reply.as_mut_ptr(),
                null_mut(),
                1,
            )
        },
        "NtAlpcAcceptConnectPort",
    )?;
    OwnedHandle::from_handle(handle.cast())
}

fn send_datagram(
    port: windows_sys::Win32::Foundation::HANDLE,
    payload: &[u8],
    message: &mut MessageBuffer,
) -> io::Result<()> {
    trace!("sending datagram ({} bytes)", payload.len());
    message.prepare_send(payload, None)?;
    ntstatus_result(
        unsafe {
            NtAlpcSendWaitReceivePort(
                port.cast(),
                ALPC_MSGFLG_RELEASE_MESSAGE,
                message.as_mut_ptr(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        },
        "NtAlpcSendWaitReceivePort(send datagram)",
    )
}

fn receive_datagram(
    port: windows_sys::Win32::Foundation::HANDLE,
    output: &mut [u8],
    message: &mut MessageBuffer,
) -> io::Result<()> {
    receive_message(port, message)?;
    if message.kind() != LPC_DATAGRAM {
        return Err(io::Error::other(format!(
            "expected ALPC datagram, got {}",
            message.kind()
        )));
    }
    if message.payload().len() != output.len() {
        return Err(io::Error::other(format!(
            "expected reply length {}, got {}",
            output.len(),
            message.payload().len()
        )));
    }
    output.copy_from_slice(message.payload());
    Ok(())
}

fn receive_connection_reply(
    port: windows_sys::Win32::Foundation::HANDLE,
    message: &mut MessageBuffer,
) -> io::Result<()> {
    receive_message(port, message)?;
    if message.kind() != LPC_CONNECTION_REPLY {
        return Err(io::Error::other(format!(
            "expected ALPC connection reply, got {}",
            message.kind()
        )));
    }
    Ok(())
}

fn trace_enabled() -> bool {
    static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
    *TRACE_ENABLED.get_or_init(|| std::env::var_os("IPC_BENCH_TRACE").is_some())
}

fn receive_message(
    port: windows_sys::Win32::Foundation::HANDLE,
    message: &mut MessageBuffer,
) -> io::Result<()> {
    trace!(
        "waiting for message (capacity {} bytes)",
        message.payload_capacity()
    );
    let mut buffer_length = message.capacity_bytes();
    ntstatus_result(
        unsafe {
            NtAlpcSendWaitReceivePort(
                port.cast(),
                0,
                null_mut(),
                null_mut(),
                message.as_mut_ptr(),
                &mut buffer_length,
                null_mut(),
                null_mut(),
            )
        },
        "NtAlpcSendWaitReceivePort(receive)",
    )?;
    trace!(
        "received message type {} ({} bytes)",
        message.kind(),
        message.payload().len()
    );
    Ok(())
}

fn disconnect_port(port: windows_sys::Win32::Foundation::HANDLE) {
    let _ = ntstatus_result(
        unsafe { NtAlpcDisconnectPort(port.cast(), 0) },
        "NtAlpcDisconnectPort",
    );
}

fn new_port_attributes(message_size: usize) -> ALPC_PORT_ATTRIBUTES {
    let mut attributes = unsafe { zeroed::<ALPC_PORT_ATTRIBUTES>() };
    attributes.Flags = ALPC_PORFLG_ALLOW_LPC_REQUESTS;
    attributes.SecurityQos = SECURITY_QUALITY_OF_SERVICE {
        Length: size_of::<SECURITY_QUALITY_OF_SERVICE>() as u32,
        ImpersonationLevel: SecurityImpersonation,
        ContextTrackingMode: SECURITY_DYNAMIC_TRACKING,
        EffectiveOnly: 0,
    };
    attributes.MaxMessageLength = message_size + size_of::<PORT_MESSAGE>();
    attributes.MemoryBandwidth = usize::MAX;
    attributes.MaxPoolUsage = usize::MAX;
    attributes.MaxSectionSize = usize::MAX;
    attributes.MaxViewSize = usize::MAX;
    attributes.MaxTotalSectionSize = usize::MAX;
    attributes
}

fn new_object_attributes(object_name: *mut UNICODE_STRING) -> OBJECT_ATTRIBUTES {
    OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: null_mut(),
        ObjectName: object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: null_mut(),
        SecurityQualityOfService: null_mut(),
    }
}

fn ntstatus_result(status: NTSTATUS, label: &str) -> io::Result<()> {
    if status >= 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{label} failed with NTSTATUS 0x{status:08X}"
        )))
    }
}

#[derive(Clone, Copy)]
struct ReplyKey {
    message_id: u32,
    callback_id: u32,
}

struct OwnedUnicodeString {
    _buffer: Vec<u16>,
    string: UNICODE_STRING,
}

impl OwnedUnicodeString {
    fn new(value: &str) -> io::Result<Self> {
        let mut buffer: Vec<u16> = value.encode_utf16().collect();
        buffer.push(0);
        let length_bytes = (buffer.len() - 1)
            .checked_mul(size_of::<u16>())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "unicode string too long")
            })?;
        let max_length_bytes = buffer.len().checked_mul(size_of::<u16>()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "unicode string too long")
        })?;
        let string = UNICODE_STRING {
            Length: u16::try_from(length_bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "unicode string too long")
            })?,
            MaximumLength: u16::try_from(max_length_bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "unicode string too long")
            })?,
            Buffer: buffer.as_mut_ptr(),
        };
        Ok(Self {
            _buffer: buffer,
            string,
        })
    }

    fn as_mut_ptr(&mut self) -> *mut UNICODE_STRING {
        &mut self.string
    }
}

struct MessageBuffer {
    storage: Vec<usize>,
}

impl MessageBuffer {
    fn new(payload_capacity: usize) -> Self {
        let bytes = size_of::<PORT_MESSAGE>() + payload_capacity;
        let words = bytes.div_ceil(size_of::<usize>()).max(1);
        Self {
            storage: vec![0; words],
        }
    }

    fn capacity_bytes(&self) -> usize {
        self.storage.len() * size_of::<usize>()
    }

    fn payload_capacity(&self) -> usize {
        self.capacity_bytes().saturating_sub(size_of::<PORT_MESSAGE>())
    }

    fn as_mut_ptr(&mut self) -> *mut PORT_MESSAGE {
        self.storage.as_mut_ptr().cast()
    }

    fn as_ptr(&self) -> *const PORT_MESSAGE {
        self.storage.as_ptr().cast()
    }

    fn prepare_send(&mut self, payload: &[u8], reply_key: Option<ReplyKey>) -> io::Result<()> {
        if size_of::<PORT_MESSAGE>() + payload.len() > self.capacity_bytes() {
            *self = Self::new(payload.len());
        }
        self.storage.fill(0);
        let header = self.as_mut_ptr();
        let data_length = i16::try_from(payload.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "ALPC payload exceeds i16 size")
        })?;
        let total_length =
            i16::try_from(size_of::<PORT_MESSAGE>() + payload.len()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "ALPC message exceeds i16 size")
            })?;
        unsafe {
            (*header).u1.s.DataLength = data_length;
            (*header).u1.s.TotalLength = total_length;
            if let Some(reply_key) = reply_key {
                (*header).MessageId = reply_key.message_id;
                (*header).u4.CallbackId = reply_key.callback_id;
            }
        }
        if !payload.is_empty() {
            self.payload_mut(payload.len()).copy_from_slice(payload);
        }
        Ok(())
    }

    fn kind(&self) -> u32 {
        unsafe { (*self.as_ptr()).u2.s.Type as u32 & 0xff }
    }

    fn payload(&self) -> &[u8] {
        let len = unsafe { (*self.as_ptr()).u1.s.DataLength as usize };
        unsafe {
            std::slice::from_raw_parts(
                self.as_ptr().cast::<u8>().add(size_of::<PORT_MESSAGE>()),
                len,
            )
        }
    }

    fn payload_mut(&mut self, len: usize) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.as_mut_ptr()
                    .cast::<u8>()
                    .add(size_of::<PORT_MESSAGE>()),
                len,
            )
        }
    }

    fn reply_key(&self) -> Option<ReplyKey> {
        let header = self.as_ptr();
        let needs_reply = unsafe { (*header).u2.s.Type as u32 & LPC_CONTINUATION_REQUIRED != 0 };
        needs_reply.then(|| ReplyKey {
            message_id: unsafe { (*header).MessageId },
            callback_id: unsafe { (*header).u4.CallbackId },
        })
    }
}
