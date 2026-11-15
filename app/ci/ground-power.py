"""What the panes' backdrop costs in power: the GPU's milliwatts and the window server's CPU
with the grain still, then moving, then moving under streaming panes, in a real window in front.

cargo build --release --example ground_bench   (in app/)
python3 -I app/ci/ground-power.py [--bench PATH] [--seconds 30]

It asks for sudo once, as powermetrics needs root, and runs nothing else as root. Each case
opens ground_bench's window, waits for it to settle, samples
`powermetrics --samplers gpu_power,tasks -i 1000 -n SECONDS` and prints one table. Leave the
window in front and the Mac alone while it runs, on power, with nothing else busy.
"""

import argparse
import re
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CASES = ["still", "moving", "stream"]
SETTLE = 5
GPU_POWER = re.compile(r"^GPU (?:HW )?Power:\s*([\d.]+)\s*mW", re.M)
GPU_BUSY = re.compile(r"^GPU HW active residency:\s*([\d.]+)%", re.M)


def task_row(name, sample):
    """The CPU ms/s and, when shown, the GPU ms/s of the task whose name starts the row."""
    cpu, gpu = [], []
    for line in sample.splitlines():
        found = re.match(rf"^{re.escape(name)}\s+\d+\s+([\d.]+)\s.*?([\d.]+)\s*$", line)
        if found:
            cpu.append(float(found.group(1)))
            gpu.append(float(found.group(2)))
    return cpu, gpu


def mean(values):
    return sum(values) / len(values) if values else float("nan")


def run_case(bench, mode, seconds):
    app = subprocess.Popen([str(bench), mode, str(SETTLE + seconds + 3)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(SETTLE)
    if app.poll() is not None:
        sys.exit(f"ground_bench {mode} quit early: {app.stderr.read().strip()}")
    sample = subprocess.run(
        ["sudo", "-n", "powermetrics", "--samplers", "gpu_power,tasks", "--show-process-gpu", "-i", "1000", "-n", str(seconds)],
        capture_output=True, text=True,
    )
    if sample.returncode != 0:
        app.kill()
        sys.exit(f"powermetrics failed: {sample.stderr.strip()}")
    out, err = app.communicate(timeout=60)
    text = sample.stdout
    server_cpu, _ = task_row("WindowServer", text)
    bench_cpu, bench_gpu = task_row("ground_bench", text)
    ground = re.search(r"ground ([\d.]+) passes/s at ([\d.]+) ms GPU", out)
    return {
        "gpu_mw": mean([float(v) for v in GPU_POWER.findall(text)]),
        "gpu_busy": mean([float(v) for v in GPU_BUSY.findall(text)]),
        "server_cpu": mean(server_cpu),
        "bench_cpu": mean(bench_cpu),
        "bench_gpu": mean(bench_gpu),
        "passes": float(ground.group(1)) if ground else float("nan"),
        "pass_ms": float(ground.group(2)) if ground else float("nan"),
        "bench": (out or err).strip(),
    }


def main():
    options = argparse.ArgumentParser()
    options.add_argument("--bench", default=str(ROOT / "target/release/examples/ground_bench"))
    options.add_argument("--seconds", type=int, default=30)
    options = options.parse_args()
    bench = Path(options.bench)
    if not bench.exists():
        sys.exit(f"no {bench}: run `cargo build --release --example ground_bench` in app/ first")
    if subprocess.run(["sudo", "-v"]).returncode != 0:
        sys.exit("powermetrics needs sudo")
    results = {}
    for mode in CASES:
        print(f"{mode}: {options.seconds} s of samples after {SETTLE} s to settle", flush=True)
        results[mode] = run_case(bench, mode, options.seconds)
        print(f"  {results[mode]['bench']}", flush=True)
    print()
    print(f"{'':8} {'GPU mW':>8} {'GPU busy':>9} {'WindowServer CPU ms/s':>22} {'bench CPU ms/s':>15} {'bench GPU ms/s':>15} {'ground passes/s':>16} {'ms a pass':>10}")
    for mode, r in results.items():
        print(f"{mode:8} {r['gpu_mw']:8.0f} {r['gpu_busy']:8.1f}% {r['server_cpu']:22.1f} {r['bench_cpu']:15.1f} {r['bench_gpu']:15.1f} {r['passes']:16.1f} {r['pass_ms']:10.3f}")


if __name__ == "__main__":
    main()
