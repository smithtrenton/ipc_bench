"""Python socket TCP loopback benchmark."""

from __future__ import annotations

import multiprocessing as mp
import socket
from queue import Empty

from benchmarks.methods.python.benchmark_adapter import (
    make_payload,
    parse_config,
    print_report,
    run_benchmark,
    update_payload,
)


def _recv_exact_into(stream: socket.socket, buffer: bytearray) -> None:
    view = memoryview(buffer)
    received = 0
    while received < len(buffer):
        chunk = stream.recv_into(view[received:])
        if chunk == 0:
            message = "socket closed"
            raise EOFError(message)
        received += chunk


def _worker(ports: mp.Queue[int], message_size: int) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
        server.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        server.bind(("127.0.0.1", 0))
        server.listen(1)
        ports.put(server.getsockname()[1])
        conn, _ = server.accept()
        with conn:
            conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            scratch = bytearray(message_size)
            while True:
                try:
                    _recv_exact_into(conn, scratch)
                except EOFError:
                    return
                if scratch:
                    scratch[0] = (scratch[0] + 1) % 256
                conn.sendall(scratch)


def _main() -> None:
    config = parse_config()
    ports: mp.Queue[int] = mp.Queue(maxsize=1)
    process = mp.Process(target=_worker, args=(ports, config.message_size))
    process.start()
    try:
        port = ports.get(timeout=5)
    except Empty as error:
        raise TimeoutError("py-socket-tcp-loopback worker failed to publish its port") from error

    stream = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    stream.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    stream.connect(("127.0.0.1", port))

    outbound = make_payload(config.message_size)
    inbound = bytearray(config.message_size)

    def operation() -> None:
        stream.sendall(outbound)
        _recv_exact_into(stream, inbound)
        update_payload(outbound, inbound)

    report = run_benchmark("py-socket-tcp-loopback", config, operation, child_ready=True)
    stream.close()
    process.join(timeout=5)
    print_report(report, config.output_format)


if __name__ == "__main__":
    mp.freeze_support()
    _main()
