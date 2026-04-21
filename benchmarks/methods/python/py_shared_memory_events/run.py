"""Python shared-memory benchmark using multiprocessing events."""

from __future__ import annotations

import multiprocessing as mp
from multiprocessing import shared_memory

from benchmarks.methods.python.benchmark_adapter import (
    make_payload,
    parse_config,
    print_report,
    run_benchmark,
    update_payload,
)


def _worker(
    name: str,
    message_size: int,
    request: mp.Event,
    response: mp.Event,
    stop: mp.Event,
    ready: mp.Event,
) -> None:
    shm = shared_memory.SharedMemory(name=name)
    try:
        request_buffer = shm.buf[:message_size]
        response_buffer = shm.buf[message_size : message_size * 2]
        scratch = bytearray(message_size)
        ready.set()
        while True:
            request.wait()
            request.clear()
            if stop.is_set():
                return
            scratch[:] = request_buffer
            if scratch:
                scratch[0] = (scratch[0] + 1) % 256
            response_buffer[:] = scratch
            response.set()
    finally:
        del request_buffer
        del response_buffer
        shm.close()


def _main() -> None:
    config = parse_config()
    shm = shared_memory.SharedMemory(create=True, size=config.message_size * 2)
    request = mp.Event()
    response = mp.Event()
    stop = mp.Event()
    ready = mp.Event()
    process = mp.Process(
        target=_worker,
        args=(shm.name, config.message_size, request, response, stop, ready),
    )
    process.start()
    if not ready.wait(5):
        raise TimeoutError("py-shared-memory-events worker failed to signal readiness")

    outbound = make_payload(config.message_size)
    inbound = bytearray(config.message_size)
    request_buffer = shm.buf[: config.message_size]
    response_buffer = shm.buf[config.message_size : config.message_size * 2]

    def operation() -> None:
        request_buffer[:] = outbound
        request.set()
        response.wait()
        response.clear()
        inbound[:] = response_buffer
        update_payload(outbound, inbound)

    try:
        report = run_benchmark("py-shared-memory-events", config, operation, child_ready=True)
    finally:
        stop.set()
        request.set()
        process.join(timeout=5)
        del operation
        del request_buffer
        del response_buffer
        shm.close()
        shm.unlink()

    print_report(report, config.output_format)


if __name__ == "__main__":
    mp.freeze_support()
    _main()
