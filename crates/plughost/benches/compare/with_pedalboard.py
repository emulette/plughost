"""Pedalboard counterpart of the compare benchmark: the same plugin, noise, sample rate, block
sizes, trials and reset before every timed render, in Pedalboard's in-process host.

usage: python with_pedalboard.py BUNDLE [--seconds 10] [--trials 5] [--blocks 64,512,4096]
"""
import argparse
import statistics
import time

import numpy as np
import pedalboard

RATE = 48_000


def noise(frames):
    """The compare benchmark's uniform white noise in [-0.5, 0.5), sample for sample."""
    state = 1
    channels = []
    for _ in range(2):
        samples = np.empty(frames, dtype=np.float32)
        for index in range(frames):
            state = (state * 1_664_525 + 1_013_904_223) & 0xFFFFFFFF
            samples[index] = (state >> 8) / (1 << 24) - 0.5
        channels.append(samples)
    return np.stack(channels)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("plugin")
    parser.add_argument("--seconds", type=float, default=10.0)
    parser.add_argument("--trials", type=int, default=5)
    parser.add_argument("--blocks", default="64,512,4096")
    options = parser.parse_args()

    start = time.perf_counter()
    plugin = pedalboard.load_plugin(options.plugin)
    load = time.perf_counter() - start
    print(f"plugin: {plugin.name} | pedalboard {pedalboard.__version__}")
    print(f"load: {load * 1e3:.1f} ms")
    audio = noise(int(options.seconds * RATE))
    for block in (int(value) for value in options.blocks.split(",")):
        times = []
        for trial in range(options.trials + 1):
            plugin.reset()
            start = time.perf_counter()
            output = plugin.process(audio, RATE, buffer_size=block, reset=False)
            seconds = time.perf_counter() - start
            assert output.shape == audio.shape
            # The first render warms caches and is not counted.
            if trial > 0:
                times.append(seconds)
        # The upper median, as the Rust side takes.
        median = sorted(times)[len(times) // 2]
        print(
            f"block {block}: {median * 1e3:.1f} ms ({options.seconds / median:.0f}x realtime)"
        )
    start = time.perf_counter()
    state = plugin.raw_state
    save = time.perf_counter() - start
    start = time.perf_counter()
    plugin.raw_state = state
    restore = time.perf_counter() - start
    print(f"state ({len(state)} bytes): save {save * 1e3:.2f} ms, restore {restore * 1e3:.2f} ms")


if __name__ == "__main__":
    main()
