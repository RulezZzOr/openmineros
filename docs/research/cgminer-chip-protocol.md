# cgminer (VNish 1.3.5 ramdisk): chip identification and reply handling

Status: **decompiled facts only; unverified on hardware.** Analysed with REA
6.3.0 and Ghidra 12.1.4. The binary and its extracted rootfs are outside the
repository. Addresses are Ghidra virtual addresses in `usr/bin/cgminer`.

## Chip-ID table (verified in the bytes)

Word array at `0x5b2654` (file offset `0x5a2654`), read by `FUN_000a63f0(i)`
for `i < 8`:

| index | chip ID |
|---|---|
| 0 | `0x1360` |
| 1 | `0x1362` |
| 2 | **`0x1398`** (BM1398) |
| 3 | `0x1366` |
| 4 | `0x1368` |
| 5 | `0x1370` |
| 6 | `0x1489` |
| 7 | `0x1491` |

The table is the list of chip families this build supports. BM1398 is
index 2. `0x1397` (BM1397) is not in the table.

## Reply reader

- `FUN_000d1c88` sends a request, waits 200 and then polls with 250 as the delay.
  The units are assumed to be milliseconds; this is not confirmed.
  It accepts a reply only when the reply's chain id equals the chain's
  `param_1+0x18` and its type byte is `0x04`.
- `FUN_000d676c` does the same with a register id, up to 3 attempts. It is
  called from `FUN_000d66ac`.
- Both use a lock and a 10-level busy check around the UART queue.

## Family dispatch

- `FUN_000d0f64(family, mode, …)` fills a chip-operations struct with function
  pointers. It switches on `family` (0 to 4) and then on `mode` (0 to ...).
  The struct offsets used for init and reply handling are visible in the
  decompiled code.
- `FUN_00073788` and `FUN_0007914c` read chip IDs per chain and mark chips in
  `param+0x230` by their family field. These are the detection and
  per-chain setup paths. They were started but not finished.

## Not established yet

- The exact bytes of the BM1398 command frame and the register that carries the
  chip ID. The reply reader is known; the request builder for this family is
  not yet traced.
- Which `family`/`mode` pair selects the BM1398 operations.
- The voltage and PSU path (PIC or dsPIC) and the frequency ramp registers.
- The register map inside the FPGA window. See `vnish-fpga-base.md`.

## Next steps

1. Trace the request builder for the BM1398 operations: find callers of the
   struct pointer stored at `DAT_0068c97c` and the operations for `family == 2`.
2. Decompile `FUN_00073788` to the end and record the chain-detection order.
3. Compare with the `bm1397` crate in <https://github.com/GPTechinno/bm13xx-rs>
   only as a reference for the frame layout. Do not copy code from it.
