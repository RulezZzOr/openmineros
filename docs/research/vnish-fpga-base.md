# VNish 1.3.5 (S19 Xilinx): FPGA mapping base address

Status: **hypothesis derived from disassembly; unverified on hardware.**

## Source and method

- Subject: `usr/bin/hwscan` from `uramdisk.image.gz` in the VNish 1.3.5 NAND
  package. ARM ELF, SHA-256 `199eb6e4fe3e85731e8f78e75c4dd2f603692dbe3cece3688ef44c671f83e8db`.
- Tool: REA 6.3.0 with Ghidra 12.1.4 (headless, read-only, ephemeral project).
  The binary and extracted rootfs stay outside the repository.
- No code is copied here. Only addresses, call relations and string
  references are recorded.

## Findings

1. **Device access.** The code opens `/dev/mem` and calls `mmap64` on it.
   The string `Failed to open /dev/mem device` is at `0x48188d`/`0x481896`.
   The initialiser is `FUN_00109048`.
2. **Mapping that matters.** `FUN_00109048` maps two regions:
   - `0x1400` bytes at the physical offset passed in by the caller (its first
     argument, `r0`, saved into `r4`).
   - `0x1000000` bytes at an offset read from a global.
3. **Base address.** The caller `FUN_0010b1d0` (in the `platform.c` path)
   calls `thunk_FUN_00109048(0x40000000)`. So the stock register window is
   **physical `0x40000000`, length `0x1400`**.
4. **Difference from our code.** `crates/board-xil/src/fpga.rs` sets
   `FPGA_BASE_ADDRESS = 0x43C0_0000`, the base used by the Braiins/DCENT
   bitstream. The stock VNish image does not use that base.
5. **Strings from the same build** (useful for locating the next functions):
   `libbitmain/src/xil/fpga.c`, `libbitmain/src/xil/platform.c`,
   `Failed to reset FPGA UART speed`, `Failed to start hardware queue`,
   `Failed to init nonce queue`, `Failed to init reg queue`.

## Register access (same binary)

- Register read/write is `*(base + index * 4)`. The index comes from
  `FUN_0010addc(id)`, which reads a per-variant table at file offset
  `0x4719c4 + variant * 0x2e8` (virtual `0x4819c4`; file offset = vaddr - `0x10000`).
- Variant selector: `FUN_0010ae08` sets `variant` from `FUN_00109c60(2)`,
  which reads word 2 directly (`base + 0x08`). Variant is 1 when that value is
  non-zero and bit 31 is clear, otherwise 0. Reads are under a lock with memory
  barriers and have no writes.
- Control register, logical id 35 (`0x23`): word `70` (`base + 0x118`) in
  variant 0, word `64` (`base + 0x100`) in variant 1. The init code sets bit
  `0x20` or `0x100` on this register when it changes state.
- Logical id 18 (`0x12`): word `33` (`base + 0x84`) in variant 0, word `32`
  (`base + 0x80`) in variant 1.

## Initialisation order (`FUN_0010b1d0`, `platform.c`)

1. Map the FPGA window at physical `0x40000000` (`FUN_00109048`).
2. Select the register variant (`FUN_0010ae08`, see above).
3. Write the control register (`FUN_00109360`).
4. Start the hardware queue (`FUN_00113054`).
5. Set the FPGA UART speed (`FUN_0010a0ac`, argument in baud).
6. Initialise the register queue (`FUN_00105f0c`) and the nonce queue
   (`FUN_00112e5c`).

Step 5 takes a baud value and writes a rate code to register id 15 (`0xf`).
Values accepted by the code, as literals in `FUN_0010a0ac`: 115200, 921600,
1500000, 1562500, 3000000, 3125000, 6000000, 10000000, 12000000, 20000000,
25000000. Only the two codes below were read with certainty: 115200 → `0x1a`,
921600 → `2`. The remaining codes are not recorded here.

Nothing here has been run. The order is what the stock firmware does; it is
not a tested sequence for our board.

## Open questions

- The second region's physical offset is loaded from a global and is not yet
  resolved.
- Logical ids beyond 35 and 18 are not named yet.
- Whether a stock S19 control board actually answers at `0x40000000` and which
  variant it reports is unknown until read on hardware.
- Reads at `0x08`, `0x100` and `0x118` are believed to have no side effects,
  but that is inferred from the code, not measured.

## Next step

Read-only check on the miner, with VNish running. The stock busybox includes
the `devmem` applet; if the bare name is missing, use `busybox devmem`:

```sh
devmem 0x40000008 32    # variant selector
devmem 0x40000100 32    # control, variant 1 offset
devmem 0x40000118 32    # control, variant 0 offset
```

Send the output. Do not write anything to these registers yet.

The same five words can be read with our own read-only probe, which is built
into the s19 ramdisk as `omo-fpga-probe-armv7`:

```sh
omo-fpga-probe --raw 0x40000000   # stock VNish base
omo-fpga-probe --raw 0x43C00000   # Braiins / D-Central base
```

It prints offsets `0x08`, `0x80`, `0x84`, `0x100` and `0x118` as JSON and
writes nothing. On a VNish unit `devmem` gives the same values, so the probe
only helps on our own image. It was checked under qemu for its usage and error
paths only. Reading `/dev/mem` on real hardware has not been run.
