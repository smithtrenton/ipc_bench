"""Python multiprocessing pipe benchmark."""

from __future__ import annotations

import multiprocessing as mp
from typing import TYPE_CHECKING

from benchmarks.methods.python.benchmark_adapter import (
    make_payload,
    parse_config,
    print_report,
    run_benchmark,
    update_payload,
)

if TYPE_CHECKING:
    from multiprocessing.connection import Connection


def _worker(connection: Connection, ready: mp.Event) -> None:
    ready.set()
    while True:
        try:
            payload = connection.recv_bytes()
        except (EOFError, BrokenPipeError):
            return
        response = bytearray(payload)
        if response:
            response[0] = (response[0] + 1) % 256
        connection.send_bytes(response)


def _main() -> None:
    config = parse_config()
    parent, child = mp.Pipe(duplex=True)
    ready = mp.Event()
    process = mp.Process(target=_worker, args=(child, ready))
    process.start()
    child.close()
    if not ready.wait(5):
        raise TimeoutError("py-multiprocessing-pipe worker failed to signal readiness")

    outbound = make_payload(config.message_size)
    inbound = bytearray(config.message_size)

    def operation() -> None:
        parent.send_bytes(outbound)
        response = parent.recv_bytes()
        inbound[:] = response
        update_payload(outbound, inbound)

    report = run_benchmark("py-multiprocessing-pipe", config, operation, child_ready=True)
    parent.close()
    process.join(timeout=5)
    print_report(report, config.output_format)


if __name__ == "__main__":
    mp.freeze_support()
    _main()
