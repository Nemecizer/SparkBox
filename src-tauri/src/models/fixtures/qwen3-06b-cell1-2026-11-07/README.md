# Recorded cell1 Qwen serving measurements

Captured November 7, 2026 through the signed `2464e0c` SparkBox app's normal
Admin-gated Fleet Command and Benchmarks controls. This is real **single-node**
evidence, not a simulated scrape or proof of other engines/layouts.

- Qwen/Qwen3-0.6B, revision `c1899de289a04d12100db370d81485cdf75e47ca`.
- Existing vLLM image `vllm/vllm-openai:glm53-flash-arm64-cu130`; its exact ID is
  in `node-samples.json`. Kernel `7.0.0-1025-nvidia`, GB10 driver `580.178.04`.
- TP1/PP1, context 4096, GPU memory share 0.20, default KV type, prefix caching
  on, qwen3 reasoning parser, API key required. No speculative/MTP recipe,
  overrides, remote code, cache drops, keep-running, image pulls or node updates.
- The 100-second window contains 26 samples, four seconds apart. Its one-at-a-time
  batch completed four 512-input/2048-output requests; the benchmark also makes
  a warm-up request. Background readiness probes remain enabled. Therefore raw
  engine counters include more than the measured benchmark requests.

`idle.prom`, `generating.prom` and `after.prom` are the engine's vLLM metric
families at 0, 32.001 and 100.001 seconds. HELP/TYPE text, names, values, buckets
and label structure are preserved. Non-vLLM families and `cache_config_info`
were omitted. `model_name` is the public served name, other label values are
redacted except `engine`, `le` and `finished_reason`. `_created` sample values
are normalized to zero. No authentication header, environment or key is read.

`node-samples.json` contains the corresponding raw NVIDIA CSV, `/proc/meminfo`,
memory pressure, selected VM counters and cgroup-v2 `cpu.stat`, `memory.current`
and `memory.stat`. The GPU process PID is replaced with 4242. Container/instance
IDs, node paths and absolute capture times are omitted. `elapsedMs` retains
the actual relative timing; the timeline keeps the small original metric subset
for all 26 points. The full private evidence capture's SHA-256 is recorded.

The distinction matters: GPU allocation stays at 23,717 MiB even when the KV
occupancy returns to zero. The cgroup reports about 4.61 GiB, not the whole
unified-memory footprint. The engine logs report 21.22 GiB available for KV,
198,656 cache tokens and 1.12 GiB to load weights. These are observations for
this image and configuration, not new fit constants.

Generation readings reach 96% GPU utilization at 2444–2457 MHz without an active
clock-event reason. Idle-before-load had SW power cap at 4.12 W/P8; there was no
busy SW-power-cap observation. Existing warning rules are unchanged. Corruption
and speculative counters are absent, not measured zeroes or proof of MTP.

The first 90-second capture exceeded its 60,000-byte encoded-output guard and
discarded its result; that failure is retained in the local evidence. A second,
smaller capture produced these complete fixtures. Both workload batches, fresh
Chat, normal unload and all 16 final checks passed. See
`docs/continuation/2026-11-07-serving-facts.md` for results, preservation and limits.
