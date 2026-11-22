# Miner 192.168.2.41: incident log, 2026-10-10

Status: **miner offline since about 20:23 CEST; cause unknown.**

## Unit

- Web UI title: `s19a`, firmware VNish / AnthillOS (`Antminer S19 Xilinx (1.3.5)`).
- Before the incident: 3 boards, 76 chips per board, all chips reported good,
  about 65 to 70 TH/s, 1900 W preset, autotune between 400 and 522 MHz.
- Kernel log (System tab, read earlier): `CPU1: failed to boot: -1`, so only
  one core was running. NAND is a Micron MT29F2G08 with the warning
  `pl35x-nand: the ECC used on your system is too weak`.

## Timeline (browser console, `read_console_messages`, Chrome on the miner page)

| Time (CEST) | Event |
|---|---|
| 20:20:41 | First `/summary` request times out after 15 s |
| 20:20:53 | `/status` times out after 12 s |
| 20:21:02 | `/perf-summary` times out |
| 20:21 to 20:23 | Repeated timeouts on `/summary`, `/status` |
| 20:23:29 | `Network Error` on `/perf-summary`, `/status`, `/summary` |
| 20:26:03 | Burst of `Network Error` and `EXCEPTION` entries (41 messages in total) |
| 20:26:21 | From the Mac: `ping` 100 % loss, `curl` to `/summary`, `/status`, `/perf-summary` fails (exit 7). ARP entry is `(incomplete)`. The gateway `192.168.2.1` answers normally. |

Reading: the miner was overloaded before it dropped off the network. The
single-core observation makes it plausible that the web and control process
starved during autotune, then the unit stopped responding at layer 2. This is a
hypothesis. Nothing here proves a crash, a reboot, or a power loss.

## What is not known

- Whether the unit was switched off, rebooted, or lost power.
- Whether the miner ran a watchdog reboot.
- The contents of the Status, Miner, HWscan and Autotune logs after 20:20.
  Those tabs are fetched from the miner, which is now offline.

## Checks to do on the unit

1. Power LED, PSU fan, and the Ethernet link LED on the control board.
2. If the unit is on but silent, power cycle it once and watch the first
   minute of boot.
3. When it answers again, run the read-only commands below, then the monitor
   script in `scratchpad` if you want continuous logging.

```sh
devmem 0x40000008 32
devmem 0x40000100 32
devmem 0x40000118 32
cat /proc/mtd
```

## Second unit (not the same miner)

- `192.168.2.47` is a different machine: web title `s19c`, AnthillOS, 3 boards,
  1100 W preset, about 378 MHz, 52.7 TH/s, mining. Ports 22 and 80 open.
- It is not the unit that dropped off. Do not mix their logs or settings.

## Not done

- No registers written.
- No firmware flashed.
- No settings changed in the miner web UI.
