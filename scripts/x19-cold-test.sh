#!/bin/sh
# Cold start of S19 hashboards by our own code, on a unit running Braiins OS.
#
# bosminer only provides the PSU: it is frozen (SIGSTOP) once its voltage has
# settled. Each chain is then reset through its GPIO and initialised from
# scratch by omo-chain-probe, using the chip register values bosminer had set
# (read just before the reset). The hashboard voltage controllers (PICs) are
# kept alive with our own heartbeat. bosminer is restarted at the end.
#
#   x19-cold-test.sh "<chains>" [probe-path]        e.g. x19-cold-test.sh "1 2 3"
#   MODE=mine POOL=host:port USER_NAME=addr.worker SECONDS_RUN=600 x19-cold-test.sh "1 2 3"
set -u
CHAINS=${1:?chains}
T=${2:-/tmp/omo-chain-probe}
P=$(pidof bosminer)
[ -n "$P" ] || { echo "bosminer not running"; exit 1; }
ALL=0x00,0x08,0x0c,0x10,0x14,0x18,0x1c,0x20,0x28,0x2c,0x3c,0x44,0x60,0x68,0x70,0x80,0x84,0xa4,0xa8
PICS=
for c in $CHAINS; do PICS="$PICS,$(printf '0x%02x' $((0x1f + c)))"; done
PICS=${PICS#,}

HB=
finish() {
    [ -n "$HB" ] && kill "$HB" 2>/dev/null
    kill -CONT "$P" 2>/dev/null
    /etc/init.d/bosminer restart >/dev/null 2>&1 &
    echo "== bosminer resumed and restarting"
}
trap finish EXIT INT TERM
kill -STOP "$P"
# Keep the board PICs alive while we initialise (the miner has its own thread).
( while true; do $T pic --addrs "$PICS" --cmd heartbeat >/dev/null 2>&1; sleep 1; done ) &
HB=$!

init_chain() {
    CH=$1
    echo "== chain $CH: dump bosminer config"
    $T dump-regs --chain "$CH" --regs $ALL > /tmp/omo-warm-$CH.json || return 2
    echo "== chain $CH: reset"
    $T fpga-write --chain "$CH" --off 0x08 --value 0x16 >/dev/null
    $T reset --chain "$CH" --hold-ms 300 --wait-ms 1500
    $T fpga-write --chain "$CH" --off 0x10 --value 0x6c >/dev/null
    $T fpga-write --chain "$CH" --off 0x08 --value 0x1e >/dev/null
    $T reset-fifos --chain "$CH"
    $T dump-regs --chain "$CH" --regs $ALL > /tmp/omo-cold0-$CH.json || return 3
    echo "== chain $CH: addresses"
    $T chain-inactive --chain "$CH"
    $T set-addr --chain "$CH" --count 76 --step 3
    echo "== chain $CH: configuration"
    $T apply-regs --chain "$CH" --file /tmp/omo-warm-$CH.json --regs 0x2c,0x20,0x1c,0x44,0x80,0x84
    $T write-reg --chain "$CH" --reg 0x3c --value 0x80008710
    $T write-reg --chain "$CH" --reg 0x3c --value 0x80008050
    $T apply-regs --chain "$CH" --file /tmp/omo-warm-$CH.json --regs 0x10,0x14,0xa4,0xa8,0x0c,0x70,0x08
    sleep 1
    echo "== chain $CH: fast UART"
    $T apply-regs --chain "$CH" --file /tmp/omo-warm-$CH.json --regs 0x68,0x28
    sleep 1
    $T apply-regs --chain "$CH" --file /tmp/omo-warm-$CH.json --regs 0x18
    sleep 1
    $T fpga-write --chain "$CH" --off 0x10 --value 0 >/dev/null
    $T reset-fifos --chain "$CH"
    $T dump-regs --chain "$CH" --regs 0x00 > /tmp/omo-cold1-$CH.json
    echo "chain $CH: $(grep -c 0x1398 /tmp/omo-cold1-$CH.json) chips answer after init"
}

LIST=
for c in $CHAINS; do
    init_chain "$c" || { echo "chain $c init failed"; exit 4; }
    LIST="$LIST,$c"
done
LIST=${LIST#,}

if [ "${MODE:-test}" = mine ]; then
    kill "$HB" 2>/dev/null; HB=
    echo "== mining chains $LIST on ${POOL:?} as ${USER_NAME:?} for ${SECONDS_RUN:-180} s (PIC heartbeat $PICS)"
    $T mine --chains "$LIST" --pic-heartbeat "$PICS" --pool "$POOL" --user "$USER_NAME" \
        --seconds "${SECONDS_RUN:-180}" > /tmp/omo-mine.json 2>/tmp/omo-mine.log
    cat /tmp/omo-mine.log
    grep -vE '^\s*"t=' /tmp/omo-mine.json | head -50
    exit 0
fi

for c in $CHAINS; do
    echo "== chain $c: work test"
    $T work-test --chain "$c" --ms 4000 ${WT_ARGS:---slots 8 --id-shift 3 --order reversed} > /tmp/omo-cold-wt-$c.json 2>&1
    grep -E '"(nonces|frames_sent|valid_diff1|matched_work_id|best_zero_bits|no_job_found)"' /tmp/omo-cold-wt-$c.json
done
echo "== done"
