# Signer latency log

Interactive 2-of-3 signing on `cggmp21` 0.6.3, no presignatures. This is where the design's 596 ms figure comes from.

## Figures

| Date | Case | Orders | p50 | p95 | Max |
|---|---|---|---|---|---|
| 2026-11-08, signer alone | 3 signers up | 50 | 596 ms | 611 ms | 621 ms |
| 2026-11-08, signer alone | 1 signer down | 20 | 593 ms | | 598 ms |
| 2026-11-08, wired | 3 signers up, real chain, direct `POST /sign` | 50 | 612 ms | 635 ms | 660 ms |
| 2026-11-08, wired | Coordinator `/prepare` plus `/intent` to the fake venue | 20 | 667 ms | | 1318 ms |
| 2026-11-08, after rulings, 3 runs | 3 signers up, real chain, direct `POST /sign` | 50 each | 606, 613, 590 ms | 632, 625, 599 ms | 633, 630, 607 ms |
| 2026-11-08, after rulings, 3 runs | Coordinator through `delegate_client.py` to the fake venue | 20 each | 657, 666, 651 ms | | 681, 679, 681 ms |

Run log lines, verbatim:

```
[INFO] latency 3 up, 50 orders ms: p50 596 p95 611 max 621
[INFO] latency s3 down, 20 orders ms: p50 593 max 598
```

The first two rows are the figure the design quotes. The two wired rows are from the last of the first four end-to-end runs; two earlier runs gave direct p50 588 ms.

## Method

- Three `lockbox-signer` release builds on one 12-core machine with other work running, all on loopback, reading a stub chain.
- One Python client, sequential, no concurrency. Each sample is one `POST /sign` of a fresh delegate-signed order on signer `s1`, timed around the call with `time.perf_counter()`.
- Percentiles are the sorted list at index 25 and 47 of 50 (index 10 of 20 for the second row).
- Wired rows: the same client and machine, 4 validators with 1 s blocks, real REST reads, `RecordDecision` transactions queueing behind each decision. A coordinator sample also covers its two HTTP calls, its own digest and recovery in Python, and the post to the fake venue. The maximum there is about two rounds long, which fits a sample that also re-armed the dead-man; per-sample re-arms were not recorded. The client's own ed25519 signing is outside the timed part of the direct row and inside the coordinator row.
- A round is 6 message rounds, so this is mostly compute and is a floor. Network delay across clouds adds to it per round and is not measured here.
