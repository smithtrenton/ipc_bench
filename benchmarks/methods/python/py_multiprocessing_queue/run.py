"""Python multiprocessing queue benchmark."""

from __future__ import annotations

import multiprocessing as mp

from benchmarks.methods.python.benchmark_adapter import (
    make_payload,
    parse_config,
    print_report,
    run_benchmark,
    update_payload,
)


def _worker(
    requests: mp.Queue[bytearray | None],
    responses: mp.Queue[bytearray],
    ready: mp.Event,
) -> None:
    ready.set()
    while True:
        payload = requests.get()
        if payload is None:
            return
        if payload:
            payload[0] = (payload[0] + 1) % 256
        responses.put(payload)


def _main() -> None:
    config = parse_config()
    requests: mp.Queue[bytearray | None] = mp.Queue(maxsize=1)
    responses: mp.Queue[bytearray] = mp.Queue(maxsize=1)
    ready = mp.Event()
    process = mp.Process(target=_worker, args=(requests, responses, ready))
    process.start()
    if not ready.wait(5):
        raise TimeoutError("py-multiprocessing-queue worker failed to signal readiness")

    outbound = make_payload(config.message_size)
    inbound = bytearray(config.message_size)

    def operation() -> None:
        requests.put(outbound.copy())
        inbound[:] = responses.get()
        update_payload(outbound, inbound)

    report = run_benchmark("py-multiprocessing-queue", config, operation, child_ready=True)
    requests.put(None)
    process.join(timeout=5)
    print_report(report, config.output_format)


if __name__ == "__main__":
    mp.freeze_support()
    _main()
