"""Shared adapter helpers for Python benchmark methods."""

from __future__ import annotations

import argparse
import json
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, NoReturn

if TYPE_CHECKING:
    from collections.abc import Callable

MESSAGE_BYTE_MODULUS = 251
FIRST_BYTE_MODULUS = 256
MICROS_PER_MILLISECOND = 1_000.0
MICROS_PER_SECOND = 1_000_000.0
TARGET_BATCHES_PER_TRIAL = 100
MAX_BATCH_SIZE = 100


@dataclass
class BenchmarkConfig:
    """Configuration shared across Python benchmark methods."""

    message_count: int = 1000
    message_size: int = 1000
    warmup_count: int = 100
    trials: int = 3
    output_format: str = "text"
    role: str = "parent"

    def to_report(self) -> dict[str, object]:
        """Return a JSON-serializable representation of the configuration."""
        return {
            "message_count": self.message_count,
            "message_size": self.message_size,
            "warmup_count": self.warmup_count,
            "trials": self.trials,
            "output_format": self.output_format,
            "role": self.role,
        }


def _raise_config_error(message: str) -> NoReturn:
    raise SystemExit(message)


def parse_config() -> BenchmarkConfig:
    """Parse command-line flags into a benchmark configuration."""
    parser = argparse.ArgumentParser(prog=Path(sys.argv[0]).stem)
    parser.add_argument("-c", "--message-count", type=int, default=1000)
    parser.add_argument("-s", "--message-size", type=int, default=1000)
    parser.add_argument("-w", "--warmup-count", type=int, default=100)
    parser.add_argument("-t", "--trials", type=int, default=3)
    parser.add_argument("--format", choices=("text", "json"), default="text")
    parser.add_argument("--role", choices=("parent", "child"), default="parent")
    args = parser.parse_args()

    if args.message_count <= 0:
        _raise_config_error("message count must be greater than zero")
    if args.trials <= 0:
        _raise_config_error("trials must be greater than zero")
    if args.message_size < 0:
        _raise_config_error("message size must not be negative")
    if args.warmup_count < 0:
        _raise_config_error("warmup count must not be negative")

    return BenchmarkConfig(
        message_count=args.message_count,
        message_size=args.message_size,
        warmup_count=args.warmup_count,
        trials=args.trials,
        output_format=args.format,
        role=args.role,
    )


def make_payload(size: int) -> bytearray:
    """Create the deterministic payload used by benchmark rounds."""
    return bytearray(index % MESSAGE_BYTE_MODULUS for index in range(size))


def update_payload(outbound: bytearray, inbound: bytes | bytearray) -> None:
    """Update the outbound payload using the most recent response bytes."""
    if not outbound:
        return
    outbound[:] = inbound
    outbound[0] = (outbound[0] + 1) % FIRST_BYTE_MODULUS


def measurement_batch_size(message_count: int) -> int:
    """Pick a batch size that reduces timer overhead without collapsing each trial to one sample."""
    return max(
        1,
        min(MAX_BATCH_SIZE, (message_count + TARGET_BATCHES_PER_TRIAL - 1) // TARGET_BATCHES_PER_TRIAL),
    )


def run_benchmark(
    method: str,
    config: BenchmarkConfig,
    operation: Callable[[], None],
    *,
    child_ready: bool,
) -> dict[str, object]:
    """Run warmups and timed trials for a benchmark method."""
    for _ in range(config.warmup_count):
        operation()

    trials: list[dict[str, float | int]] = []
    batch_size = measurement_batch_size(config.message_count)
    for trial_index in range(1, config.trials + 1):
        batches: list[tuple[float, int]] = []
        remaining = config.message_count
        while remaining > 0:
            current_batch = min(batch_size, remaining)
            start = time.perf_counter_ns()
            for _ in range(current_batch):
                operation()
            elapsed_micros = (time.perf_counter_ns() - start) / MICROS_PER_MILLISECOND
            batches.append((elapsed_micros / current_batch, current_batch))
            remaining -= current_batch

        total_messages = sum(count for _, count in batches)
        total_micros = sum(batch_average_micros * count for batch_average_micros, count in batches)
        average_micros = total_micros / total_messages
        min_micros = min(batch_average_micros for batch_average_micros, _ in batches)
        max_micros = max(batch_average_micros for batch_average_micros, _ in batches)
        variance = sum(
            count * (batch_average_micros - average_micros) ** 2
            for batch_average_micros, count in batches
        ) / total_messages
        stddev_micros = variance**0.5
        message_rate = float("inf") if total_micros == 0 else total_messages / (total_micros / MICROS_PER_SECOND)
        trials.append(
            {
                "trial_index": trial_index,
                "total_micros": total_micros,
                "average_micros": average_micros,
                "min_micros": min_micros,
                "max_micros": max_micros,
                "stddev_micros": stddev_micros,
                "message_rate": message_rate,
            },
        )

    total_messages = config.message_count * len(trials)
    total_micros = sum(float(trial["total_micros"]) for trial in trials)
    average_micros = total_micros / total_messages
    variance = sum(
        config.message_count
        * (
            float(trial["stddev_micros"]) ** 2
            + (float(trial["average_micros"]) - average_micros) ** 2
        )
        for trial in trials
    ) / total_messages
    summary = {
        "total_micros": total_micros,
        "average_micros": average_micros,
        "min_micros": min(trial["min_micros"] for trial in trials),
        "max_micros": max(trial["max_micros"] for trial in trials),
        "stddev_micros": variance**0.5,
        "message_rate": float("inf") if total_micros == 0 else total_messages / (total_micros / MICROS_PER_SECOND),
    }

    return {
        "method": method,
        "child_ready": child_ready,
        "config": config.to_report(),
        "trials": trials,
        "summary": summary,
    }


def render_report(report: dict[str, object], output_format: str) -> str:
    """Render a benchmark report in either text or JSON form."""
    if output_format == "json":
        return json.dumps(report, indent=2)

    summary = report["summary"]
    config = report["config"]
    lines = [
        "============ RESULTS ================",
        f"Method:             {report['method']}",
        f"Child bootstrap:    {'ok' if report['child_ready'] else 'not used'}",
        f"Message size:       {config['message_size']}",
        f"Message count:      {config['message_count']}",
        f"Warmup count:       {config['warmup_count']}",
        f"Trial count:        {config['trials']}",
        f"Total duration:     {summary['total_micros'] / MICROS_PER_MILLISECOND:.3f}\tms",
        f"Average duration:   {summary['average_micros']:.3f}\tus",
        f"Minimum duration:   {summary['min_micros']:.3f}\tus",
        f"Maximum duration:   {summary['max_micros']:.3f}\tus",
        f"Standard deviation: {summary['stddev_micros']:.3f}\tus",
        f"Message rate:       {summary['message_rate']:.0f}\tmsg/s",
    ]
    lines.extend(
        (
            "Trial {trial_index:>2}: total {total_micros:.3f} us | avg "
            "{average_micros:.3f} us | rate {message_rate:.0f} msg/s"
        ).format(**trial)
        for trial in report["trials"]
    )
    lines.append("=====================================")
    return "\n".join(lines)


def print_report(report: dict[str, object], output_format: str) -> None:
    """Write a rendered benchmark report to standard output."""
    sys.stdout.write(f"{render_report(report, output_format)}\n")
