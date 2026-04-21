use std::{
    error::Error,
    io::{self, Write},
    mem::size_of,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

use harness::{BenchmarkConfig, ManagedChild, ProcessRole, run_benchmark};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Memory::{
            CreateFileMappingA, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            OpenFileMappingA, PAGE_READWRITE, UnmapViewOfFile,
        },
        Threading::{
            CreateEventA, CreateSemaphoreA, EVENT_ALL_ACCESS, OpenEventA, OpenSemaphoreW,
            SEMAPHORE_ALL_ACCESS,
        },
    },
};

use crate::util::{
    OwnedHandle, c_string, release_semaphore, set_event, slice_from_raw_parts,
    slice_from_raw_parts_mut, unique_name, wait_for_signal, wide_string,
};

const ENV_MAPPING: &str = "IPC_BENCH_MAPPING";
const ENV_REQ_A: &str = "IPC_BENCH_REQ_A";
const ENV_REQ_B: &str = "IPC_BENCH_REQ_B";
const ENV_REQ_C: &str = "IPC_BENCH_REQ_C";
const ENV_RESP_A: &str = "IPC_BENCH_RESP_A";
const ENV_RESP_B: &str = "IPC_BENCH_RESP_B";
const ENV_RESP_C: &str = "IPC_BENCH_RESP_C";

#[derive(Clone, Copy)]
pub enum WaitStrategy {
    Spin,
    Hybrid,
}

pub fn run_shm_events() -> Result<(), Box<dyn Error>> {
    run_mailbox("shm-events", MailboxMode::Events)
}

pub fn run_shm_semaphores() -> Result<(), Box<dyn Error>> {
    run_mailbox("shm-semaphores", MailboxMode::Semaphores)
}

pub fn run_shm_mailbox(strategy: WaitStrategy) -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_mailbox_wait_parent(config, strategy),
        ProcessRole::Child => run_mailbox_wait_child(config, strategy),
    }
}

pub fn run_shm_ring(strategy: WaitStrategy) -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_ring_parent(config, strategy),
        ProcessRole::Child => run_ring_child(config, strategy),
    }
}

#[derive(Clone, Copy)]
enum MailboxMode {
    Events,
    Semaphores,
}

fn run_mailbox(method: &str, mode: MailboxMode) -> Result<(), Box<dyn Error>> {
    let config = BenchmarkConfig::from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    match config.role {
        ProcessRole::Parent => run_mailbox_parent(config, mode, method),
        ProcessRole::Child => run_mailbox_child(config, mode),
    }
}

fn run_mailbox_parent(
    config: BenchmarkConfig,
    mode: MailboxMode,
    method: &str,
) -> Result<(), Box<dyn Error>> {
    let mapping_name = format!(r"Local\{}", unique_name("ipc-bench-map"));
    let req_name = format!(r"Local\{}", unique_name("ipc-bench-req"));
    let resp_name = format!(r"Local\{}", unique_name("ipc-bench-resp"));

    let mut mapping = SharedMailbox::create(&mapping_name, config.message_size)?;
    mapping.stop_flag().store(0, Ordering::Release);
    let request_sync = create_sync_object(mode, &req_name)?;
    let response_sync = create_sync_object(mode, &resp_name)?;

    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[
            (ENV_MAPPING, mapping_name.clone()),
            (ENV_REQ_A, req_name.clone()),
            (ENV_RESP_A, resp_name.clone()),
        ],
    )?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let report = run_benchmark(method, &config, true, || {
        mapping.request_mut().copy_from_slice(&outbound);
        signal(mode, request_sync.raw()).expect("request signal should succeed");
        wait(mode, response_sync.raw()).expect("response wait should succeed");
        inbound.copy_from_slice(mapping.response());
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    mapping.stop_flag().store(1, Ordering::Release);
    signal(mode, request_sync.raw())?;
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_mailbox_child(config: BenchmarkConfig, mode: MailboxMode) -> Result<(), Box<dyn Error>> {
    let mapping_name = std::env::var(ENV_MAPPING)?;
    let req_name = std::env::var(ENV_REQ_A)?;
    let resp_name = std::env::var(ENV_RESP_A)?;

    let mut mapping = SharedMailbox::open(&mapping_name, config.message_size)?;
    let request_sync = open_sync_object(mode, &req_name)?;
    let response_sync = open_sync_object(mode, &resp_name)?;

    println!("ready");
    io::stdout().flush()?;

    let mut scratch = vec![0_u8; config.message_size];
    loop {
        wait(mode, request_sync.raw())?;
        if mapping.stop_flag().load(Ordering::Acquire) != 0 {
            return Ok(());
        }
        scratch.copy_from_slice(mapping.request());
        if !scratch.is_empty() {
            scratch[0] = scratch[0].wrapping_add(1);
        }
        mapping.response_mut().copy_from_slice(&scratch);
        signal(mode, response_sync.raw())?;
    }
}

fn run_mailbox_wait_parent(
    config: BenchmarkConfig,
    strategy: WaitStrategy,
) -> Result<(), Box<dyn Error>> {
    let mapping_name = format!(r"Local\{}", unique_name("ipc-bench-mailbox"));
    let req_event = format!(r"Local\{}", unique_name("ipc-bench-mailbox-req"));
    let resp_event = format!(r"Local\{}", unique_name("ipc-bench-mailbox-resp"));

    let mut mapping = SharedMailbox::create(&mapping_name, config.message_size)?;
    mapping.stop_flag().store(0, Ordering::Release);
    mapping.request_ready().store(0, Ordering::Release);
    mapping.response_ready().store(0, Ordering::Release);

    let request_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(create_event(&req_event)?)
    } else {
        None
    };
    let response_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(create_event(&resp_event)?)
    } else {
        None
    };

    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[
            (ENV_MAPPING, mapping_name.clone()),
            (ENV_REQ_C, req_event.clone()),
            (ENV_RESP_C, resp_event.clone()),
        ],
    )?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let method = match strategy {
        WaitStrategy::Spin => "shm-mailbox-spin",
        WaitStrategy::Hybrid => "shm-mailbox-hybrid",
    };
    let report = run_benchmark(method, &config, true, || {
        mapping.request_mut().copy_from_slice(&outbound);
        mapping.request_ready().store(1, Ordering::Release);
        if let Some(event) = &request_event {
            set_event(event.raw()).expect("request event should signal");
        }
        wait_for_mailbox_value(
            mapping.response_ready(),
            1,
            strategy,
            response_event.as_ref(),
        )
        .expect("response wait should succeed");
        inbound.copy_from_slice(mapping.response());
        mapping.response_ready().store(0, Ordering::Release);
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    mapping.stop_flag().store(1, Ordering::Release);
    if let Some(event) = &request_event {
        set_event(event.raw())?;
    }
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_mailbox_wait_child(
    config: BenchmarkConfig,
    strategy: WaitStrategy,
) -> Result<(), Box<dyn Error>> {
    let mapping_name = std::env::var(ENV_MAPPING)?;
    let mut mapping = SharedMailbox::open(&mapping_name, config.message_size)?;
    let request_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(open_event(&std::env::var(ENV_REQ_C)?)?)
    } else {
        None
    };
    let response_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(open_event(&std::env::var(ENV_RESP_C)?)?)
    } else {
        None
    };

    println!("ready");
    io::stdout().flush()?;

    let mut scratch = vec![0_u8; config.message_size];
    loop {
        if !wait_for_mailbox_value_or_stop(
            mapping.request_ready(),
            1,
            mapping.stop_flag(),
            strategy,
            request_event.as_ref(),
        )? {
            return Ok(());
        }
        scratch.copy_from_slice(mapping.request());
        mapping.request_ready().store(0, Ordering::Release);
        if !scratch.is_empty() {
            scratch[0] = scratch[0].wrapping_add(1);
        }
        mapping.response_mut().copy_from_slice(&scratch);
        mapping.response_ready().store(1, Ordering::Release);
        if let Some(event) = &response_event {
            set_event(event.raw())?;
        }
    }
}

fn run_ring_parent(config: BenchmarkConfig, strategy: WaitStrategy) -> Result<(), Box<dyn Error>> {
    let mapping_name = format!(r"Local\{}", unique_name("ipc-bench-ring"));
    let req_event = format!(r"Local\{}", unique_name("ipc-bench-ring-req"));
    let resp_event = format!(r"Local\{}", unique_name("ipc-bench-ring-resp"));

    let mut ring = SharedRing::create(&mapping_name, config.message_size, 64)?;
    let request_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(create_event(&req_event)?)
    } else {
        None
    };
    let response_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(create_event(&resp_event)?)
    } else {
        None
    };

    let mut child = ManagedChild::spawn_self_with_env(
        &config.child_args(),
        &[
            (ENV_MAPPING, mapping_name.clone()),
            (ENV_REQ_B, req_event.clone()),
            (ENV_RESP_B, resp_event.clone()),
        ],
    )?;
    let readiness = child.wait_for_ready()?;
    if readiness != "ready" {
        return Err(format!("unexpected child readiness message `{readiness}`").into());
    }

    let mut outbound = vec![0_u8; config.message_size];
    let mut inbound = vec![0_u8; config.message_size];
    for (index, byte) in outbound.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    let method = match strategy {
        WaitStrategy::Spin => "shm-ring-spin",
        WaitStrategy::Hybrid => "shm-ring-hybrid",
    };
    let report = run_benchmark(method, &config, true, || {
        ring.push_request(&outbound);
        if let Some(event) = &request_event {
            set_event(event.raw()).expect("request event should signal");
        }
        ring.pop_response(&mut inbound, strategy, response_event.as_ref())
            .expect("response pop should succeed");
        if !outbound.is_empty() {
            outbound.copy_from_slice(&inbound);
            outbound[0] = outbound[0].wrapping_add(1);
        }
    });

    ring.stop_flag().store(1, Ordering::Release);
    if let Some(event) = &request_event {
        set_event(event.raw())?;
    }
    child.request_shutdown();
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("child exited with status {status}").into());
    }

    print!("{}", report.render(config.output_format)?);
    Ok(())
}

fn run_ring_child(config: BenchmarkConfig, strategy: WaitStrategy) -> Result<(), Box<dyn Error>> {
    let mapping_name = std::env::var(ENV_MAPPING)?;
    let mut ring = SharedRing::open(&mapping_name, config.message_size)?;
    let request_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(open_event(&std::env::var(ENV_REQ_B)?)?)
    } else {
        None
    };
    let response_event = if matches!(strategy, WaitStrategy::Hybrid) {
        Some(open_event(&std::env::var(ENV_RESP_B)?)?)
    } else {
        None
    };

    println!("ready");
    io::stdout().flush()?;

    let mut scratch = vec![0_u8; config.message_size];
    loop {
        if !ring.pop_request(&mut scratch, strategy, request_event.as_ref())? {
            return Ok(());
        }
        if !scratch.is_empty() {
            scratch[0] = scratch[0].wrapping_add(1);
        }
        ring.push_response(&scratch);
        if let Some(event) = &response_event {
            set_event(event.raw())?;
        }
    }
}

#[repr(C)]
struct MailboxHeader {
    stop: AtomicU32,
    request_ready: AtomicU32,
    response_ready: AtomicU32,
}

struct SharedMailbox {
    mapping: OwnedHandle,
    view: *mut u8,
    message_size: usize,
}

impl SharedMailbox {
    fn create(name: &str, message_size: usize) -> io::Result<Self> {
        let mapping_size = size_of::<MailboxHeader>() + (message_size * 2);
        let handle = create_mapping(name, mapping_size)?;
        let view = map_view(handle.raw(), mapping_size)?;
        let header = unsafe { &mut *(view.cast::<MailboxHeader>()) };
        header.stop.store(0, Ordering::Release);
        header.request_ready.store(0, Ordering::Release);
        header.response_ready.store(0, Ordering::Release);
        Ok(Self {
            mapping: handle,
            view,
            message_size,
        })
    }

    fn open(name: &str, message_size: usize) -> io::Result<Self> {
        let mapping_size = size_of::<MailboxHeader>() + (message_size * 2);
        let handle = open_mapping(name)?;
        let view = map_view(handle.raw(), mapping_size)?;
        Ok(Self {
            mapping: handle,
            view,
            message_size,
        })
    }

    fn header(&self) -> &MailboxHeader {
        unsafe { &*(self.view.cast::<MailboxHeader>()) }
    }

    fn stop_flag(&self) -> &AtomicU32 {
        &self.header().stop
    }

    fn request_ready(&self) -> &AtomicU32 {
        &self.header().request_ready
    }

    fn response_ready(&self) -> &AtomicU32 {
        &self.header().response_ready
    }

    fn request(&self) -> &[u8] {
        unsafe {
            slice_from_raw_parts(self.view.add(size_of::<MailboxHeader>()), self.message_size)
        }
    }

    fn request_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice_from_raw_parts_mut(self.view.add(size_of::<MailboxHeader>()), self.message_size)
        }
    }

    fn response(&self) -> &[u8] {
        unsafe {
            slice_from_raw_parts(
                self.view
                    .add(size_of::<MailboxHeader>() + self.message_size),
                self.message_size,
            )
        }
    }

    fn response_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice_from_raw_parts_mut(
                self.view
                    .add(size_of::<MailboxHeader>() + self.message_size),
                self.message_size,
            )
        }
    }
}

impl Drop for SharedMailbox {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
        }
        let _ = self.mapping.raw();
    }
}

#[repr(C)]
struct RingHeader {
    stop: AtomicU32,
    request_write: AtomicUsize,
    request_read: AtomicUsize,
    response_write: AtomicUsize,
    response_read: AtomicUsize,
    capacity: usize,
    message_size: usize,
}

struct SharedRing {
    mapping: OwnedHandle,
    view: *mut u8,
}

struct RingQueue<'a> {
    write_index: &'a AtomicUsize,
    read_index: &'a AtomicUsize,
    capacity: usize,
    message_size: usize,
    base: *mut u8,
}

impl SharedRing {
    fn create(name: &str, message_size: usize, capacity: usize) -> io::Result<Self> {
        let total_size = size_of::<RingHeader>() + (capacity * message_size * 2);
        let handle = create_mapping(name, total_size)?;
        let view = map_view(handle.raw(), total_size)?;
        let header = unsafe { &mut *(view.cast::<RingHeader>()) };
        header.stop.store(0, Ordering::Release);
        header.request_write.store(0, Ordering::Release);
        header.request_read.store(0, Ordering::Release);
        header.response_write.store(0, Ordering::Release);
        header.response_read.store(0, Ordering::Release);
        header.capacity = capacity;
        header.message_size = message_size;
        Ok(Self {
            mapping: handle,
            view,
        })
    }

    fn open(name: &str, message_size: usize) -> io::Result<Self> {
        let total_size = size_of::<RingHeader>() + (64 * message_size * 2);
        let handle = open_mapping(name)?;
        let view = map_view(handle.raw(), total_size)?;
        Ok(Self {
            mapping: handle,
            view,
        })
    }

    fn header(&self) -> &RingHeader {
        unsafe { &*(self.view.cast::<RingHeader>()) }
    }

    fn stop_flag(&self) -> &AtomicU32 {
        &self.header().stop
    }

    fn request_base(&self) -> *mut u8 {
        unsafe { self.view.add(size_of::<RingHeader>()) }
    }

    fn response_base(&self) -> *mut u8 {
        unsafe {
            self.request_base()
                .add(self.header().capacity * self.header().message_size)
        }
    }

    fn push_request(&mut self, payload: &[u8]) {
        push_ring(
            &self.header().request_write,
            &self.header().request_read,
            self.header().capacity,
            self.header().message_size,
            self.request_base(),
            payload,
        );
    }

    fn push_response(&mut self, payload: &[u8]) {
        push_ring(
            &self.header().response_write,
            &self.header().response_read,
            self.header().capacity,
            self.header().message_size,
            self.response_base(),
            payload,
        );
    }

    fn pop_request(
        &mut self,
        buffer: &mut [u8],
        strategy: WaitStrategy,
        event: Option<&OwnedHandle>,
    ) -> io::Result<bool> {
        let queue = RingQueue {
            write_index: &self.header().request_write,
            read_index: &self.header().request_read,
            capacity: self.header().capacity,
            message_size: self.header().message_size,
            base: self.request_base(),
        };
        pop_ring(&self.header().stop, queue, buffer, strategy, event)
    }

    fn pop_response(
        &mut self,
        buffer: &mut [u8],
        strategy: WaitStrategy,
        event: Option<&OwnedHandle>,
    ) -> io::Result<bool> {
        let queue = RingQueue {
            write_index: &self.header().response_write,
            read_index: &self.header().response_read,
            capacity: self.header().capacity,
            message_size: self.header().message_size,
            base: self.response_base(),
        };
        pop_ring(&self.header().stop, queue, buffer, strategy, event)
    }
}

impl Drop for SharedRing {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
        }
        let _ = self.mapping.raw();
    }
}

fn push_ring(
    write_index: &AtomicUsize,
    read_index: &AtomicUsize,
    capacity: usize,
    message_size: usize,
    base: *mut u8,
    payload: &[u8],
) {
    loop {
        let write = write_index.load(Ordering::Acquire);
        let read = read_index.load(Ordering::Acquire);
        if write.wrapping_sub(read) < capacity {
            let slot = write % capacity;
            unsafe {
                let slot_ptr = base.add(slot * message_size);
                slice_from_raw_parts_mut(slot_ptr, message_size).copy_from_slice(payload);
            }
            write_index.store(write.wrapping_add(1), Ordering::Release);
            return;
        }
        std::hint::spin_loop();
    }
}

fn pop_ring(
    stop_flag: &AtomicU32,
    queue: RingQueue<'_>,
    buffer: &mut [u8],
    strategy: WaitStrategy,
    event: Option<&OwnedHandle>,
) -> io::Result<bool> {
    let mut spins = 0_usize;
    loop {
        let write = queue.write_index.load(Ordering::Acquire);
        let read = queue.read_index.load(Ordering::Acquire);
        if write != read {
            let slot = read % queue.capacity;
            unsafe {
                let slot_ptr = queue.base.add(slot * queue.message_size);
                buffer.copy_from_slice(slice_from_raw_parts(slot_ptr, queue.message_size));
            }
            queue
                .read_index
                .store(read.wrapping_add(1), Ordering::Release);
            return Ok(true);
        }
        if stop_flag.load(Ordering::Acquire) != 0 {
            return Ok(false);
        }
        wait_with_strategy(strategy, event, &mut spins)?;
    }
}

fn wait_for_mailbox_value(
    flag: &AtomicU32,
    target: u32,
    strategy: WaitStrategy,
    event: Option<&OwnedHandle>,
) -> io::Result<()> {
    let mut spins = 0_usize;
    loop {
        if flag.load(Ordering::Acquire) == target {
            return Ok(());
        }
        wait_with_strategy(strategy, event, &mut spins)?;
    }
}

fn wait_for_mailbox_value_or_stop(
    flag: &AtomicU32,
    target: u32,
    stop_flag: &AtomicU32,
    strategy: WaitStrategy,
    event: Option<&OwnedHandle>,
) -> io::Result<bool> {
    let mut spins = 0_usize;
    loop {
        if flag.load(Ordering::Acquire) == target {
            return Ok(true);
        }
        if stop_flag.load(Ordering::Acquire) != 0 {
            return Ok(false);
        }
        wait_with_strategy(strategy, event, &mut spins)?;
    }
}

fn wait_with_strategy(
    strategy: WaitStrategy,
    event: Option<&OwnedHandle>,
    spins: &mut usize,
) -> io::Result<()> {
    match strategy {
        WaitStrategy::Spin => std::hint::spin_loop(),
        WaitStrategy::Hybrid => {
            if *spins < 256 {
                *spins += 1;
                std::hint::spin_loop();
            } else if let Some(event) = event {
                wait_for_signal(event.raw())?;
                *spins = 0;
            } else {
                std::hint::spin_loop();
            }
        }
    }
    Ok(())
}

fn create_mapping(name: &str, size: usize) -> io::Result<OwnedHandle> {
    let name = c_string(name)?;
    let handle = unsafe {
        CreateFileMappingA(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            ((size as u64) >> 32) as u32,
            size as u32,
            name.as_ptr().cast(),
        )
    };
    OwnedHandle::from_handle(handle)
}

fn open_mapping(name: &str) -> io::Result<OwnedHandle> {
    let name = c_string(name)?;
    let handle = unsafe { OpenFileMappingA(FILE_MAP_ALL_ACCESS, 0, name.as_ptr().cast()) };
    OwnedHandle::from_handle(handle)
}

fn map_view(mapping: HANDLE, size: usize) -> io::Result<*mut u8> {
    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, size) };
    if view.Value.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(view.Value.cast())
    }
}

fn create_event(name: &str) -> io::Result<OwnedHandle> {
    let name = c_string(name)?;
    let handle = unsafe { CreateEventA(std::ptr::null(), 0, 0, name.as_ptr().cast()) };
    OwnedHandle::from_handle(handle)
}

fn open_event(name: &str) -> io::Result<OwnedHandle> {
    let name = c_string(name)?;
    let handle = unsafe { OpenEventA(EVENT_ALL_ACCESS, 0, name.as_ptr().cast()) };
    OwnedHandle::from_handle(handle)
}

fn create_semaphore(name: &str) -> io::Result<OwnedHandle> {
    let name = c_string(name)?;
    let handle = unsafe { CreateSemaphoreA(std::ptr::null(), 0, 1, name.as_ptr().cast()) };
    OwnedHandle::from_handle(handle)
}

fn open_semaphore(name: &str) -> io::Result<OwnedHandle> {
    let name = wide_string(name);
    let handle = unsafe { OpenSemaphoreW(SEMAPHORE_ALL_ACCESS, 0, name.as_ptr()) };
    OwnedHandle::from_handle(handle)
}

fn create_sync_object(mode: MailboxMode, name: &str) -> io::Result<OwnedHandle> {
    match mode {
        MailboxMode::Events => create_event(name),
        MailboxMode::Semaphores => create_semaphore(name),
    }
}

fn open_sync_object(mode: MailboxMode, name: &str) -> io::Result<OwnedHandle> {
    match mode {
        MailboxMode::Events => open_event(name),
        MailboxMode::Semaphores => open_semaphore(name),
    }
}

fn signal(mode: MailboxMode, handle: HANDLE) -> io::Result<()> {
    match mode {
        MailboxMode::Events => set_event(handle),
        MailboxMode::Semaphores => release_semaphore(handle),
    }
}

fn wait(mode: MailboxMode, handle: HANDLE) -> io::Result<()> {
    match mode {
        MailboxMode::Events | MailboxMode::Semaphores => wait_for_signal(handle),
    }
}
