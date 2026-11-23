//! Stratum V1 mining on X19 chains whose chips are already initialised.
//!
//! One frame = 8 midstates of the same header with 8 rolled versions
//! (BIP310 version rolling), so each frame needs a new extranonce2.
//! Every nonce is verified in software before it is submitted.
//!
//! A single helper thread owns the hashboard I2C bus: PIC heartbeat every
//! second, PCB temperatures every 10 s and the fan regulation.

#[cfg(unix)]
pub use imp::{FanControl, MineOptions, MineReport, mine};

/// How many days of best-share history are kept.
pub const HISTORY_DAYS: usize = 730;

/// Folds a share into the per-day best-share history (`(unix day, best)`,
/// oldest first). Returns true when the history changed and must be saved.
/// A day older than the newest entry is ignored (pool clock went backwards).
pub fn update_daily_best(days: &mut Vec<(u64, f64)>, day: u64, d: f64) -> bool {
    match days.last_mut() {
        Some((last, best)) if *last == day => {
            if d > *best {
                *best = d;
                true
            } else {
                false
            }
        }
        Some((last, _)) if *last > day => false,
        _ => {
            days.push((day, d));
            if days.len() > HISTORY_DAYS {
                days.remove(0);
            }
            true
        }
    }
}

/// Difficulty of a hash (internal byte order digest), relative to diff 1.
pub fn share_difficulty(digest: &[u8; 32]) -> f64 {
    // Value of the hash as a number (digest is little-endian).
    let mut v = 0.0f64;
    for b in digest.iter().rev() {
        v = v * 256.0 + f64::from(*b);
    }
    if v == 0.0 {
        return f64::MAX;
    }
    // diff-1 target = 0xFFFF * 2^208
    65535.0 * 2f64.powi(208) / v
}

/// Network difficulty from the compact `nbits` target.
pub fn network_difficulty(nbits: u32) -> f64 {
    let exp = (nbits >> 24) as i32;
    let mant = f64::from(nbits & 0x00FF_FFFF);
    if mant == 0.0 {
        return 0.0;
    }
    // target = mant * 256^(exp-3); diff1 = 0xFFFF * 256^(0x1d-3)
    65535.0 / mant * 256f64.powi(0x1d - exp)
}

#[cfg(unix)]
mod imp {
    use super::{network_difficulty, share_difficulty, update_daily_best};
    use crate::x19::Chain;
    use crate::x19_board::{Fans, Pics};
    use crate::x19_work::{self as work, MidstateOrder};
    use openmineros_miner_core::{
        hash::{double_sha256, meets_target},
        header::BlockHeader,
        stratum::{Notify, Subscription},
        target::{target_from_bits, target_from_pool_difficulty},
    };
    use serde::Serialize;
    use serde_json::{Value, json};
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        io::{self, BufRead, BufReader, Write},
        net::{TcpStream, ToSocketAddrs},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed},
        },
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    const SLOTS: usize = 8;
    const ID_SHIFT: u32 = 3;
    const RING: u32 = 1 << (16 - ID_SHIFT);
    const DEFAULT_MASK: u32 = 0x1fff_e000;

    #[derive(Clone, Debug)]
    pub struct FanControl {
        pub auto: bool,
        pub target_temp: f64,
        pub min_percent: u32,
        pub manual_percent: u32,
    }

    pub struct MineOptions {
        pub host: String,
        pub user: String,
        pub password: String,
        /// 0 = until stopped.
        pub seconds: u64,
        pub frame_interval: Duration,
        /// PIC I2C addresses, index-aligned with the chains.
        pub pic_heartbeat: Vec<u8>,
        pub max_pcb_temp: f64,
        pub fan: FanControl,
        pub stop: Arc<AtomicBool>,
        /// Where to write the live status JSON (every 5 s).
        pub status_path: Option<String>,
        /// Static fields merged into the status (frequency, voltage, …).
        pub status_extra: Value,
        /// File that keeps the best share across restarts (JSON).
        pub best_path: Option<String>,
    }

    #[derive(Debug, Default, Serialize)]
    pub struct MineReport {
        pub seconds: f64,
        pub frames: u64,
        pub nonces: u64,
        pub valid_diff1: u64,
        pub invalid: u64,
        pub duplicates: u64,
        pub stale_unknown_id: u64,
        pub shares_submitted: u64,
        pub shares_accepted: u64,
        pub shares_rejected: u64,
        pub reject_reasons: Vec<String>,
        pub version_mask: String,
        pub pool_difficulty: f64,
        pub hashrate_ths: f64,
        pub per_chain_valid: Vec<(u8, u64)>,
        pub heartbeats_ok: u64,
        pub heartbeats_failed: u64,
        pub max_pcb_temp_seen: f64,
        pub best_share: f64,
        pub best_share_all_time: f64,
        pub network_difficulty: f64,
        pub blocks_found: u64,
        pub stop_reason: String,
    }

    struct Slot {
        job_id: String,
        extranonce2: Vec<u8>,
        base: BlockHeader,
        versions: [u32; SLOTS],
    }

    struct Pool {
        reader: BufReader<TcpStream>,
        writer: TcpStream,
        next_id: u64,
    }

    impl Pool {
        fn connect(host: &str) -> io::Result<Self> {
            let addr = host
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))?;
            let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
            stream.set_nodelay(true)?;
            // Non-blocking: a read timeout is rounded up to the kernel tick
            // (10 ms at HZ=100), which starved the chains.
            stream.set_nonblocking(true)?;
            Ok(Self {
                reader: BufReader::new(stream.try_clone()?),
                writer: stream,
                next_id: 10,
            })
        }

        fn send(&mut self, method: &str, params: Value) -> io::Result<u64> {
            let id = self.next_id;
            self.next_id += 1;
            let mut line = json!({ "id": id, "method": method, "params": params }).to_string();
            line.push('\n');
            let mut rest = line.as_bytes();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !rest.is_empty() {
                match self.writer.write(rest) {
                    Ok(n) => rest = &rest[n..],
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(id)
        }

        fn poll(&mut self, buf: &mut String) -> io::Result<Vec<Value>> {
            let mut out = Vec::new();
            loop {
                match self.reader.read_line(buf) {
                    Ok(0) => {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "pool closed the connection",
                        ));
                    }
                    Ok(_) => {
                        if buf.ends_with('\n') {
                            if let Ok(v) = serde_json::from_str::<Value>(buf.trim()) {
                                out.push(v);
                            }
                            buf.clear();
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        return Ok(out);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    struct ChainState<'a> {
        chain: &'a Chain,
        slots: HashMap<u32, Slot>,
        order: VecDeque<u32>,
        seen: HashSet<(u32, u32, u8)>,
        next_ext: u32,
        last_frame: Instant,
        valid: u64,
        window: VecDeque<(Instant, u64)>,
    }

    /// Shared with the I2C thread.
    #[derive(Default)]
    struct Board {
        temps: [AtomicU64; 3],
        fan_percent: AtomicU32,
        hb_ok: AtomicU64,
        hb_err: AtomicU64,
    }

    fn f(a: &AtomicU64) -> f64 {
        f64::from_bits(a.load(Relaxed))
    }

    fn spawn_i2c_thread(
        opt: &MineOptions,
        board: Arc<Board>,
        stop: Arc<AtomicBool>,
    ) -> Result<std::thread::JoinHandle<()>, String> {
        let addrs = opt.pic_heartbeat.clone();
        let fan = opt.fan.clone();
        let ext_stop = opt.stop.clone();
        let mut pics = Pics::open()?;
        let fans = Fans::open()?;
        Ok(std::thread::spawn(move || {
            let mut tick: u64 = 0;
            let mut pct: f64 = if fan.auto {
                70.0
            } else {
                f64::from(fan.manual_percent)
            };
            fans.set_pwm(pct as u32);
            board.fan_percent.store(pct as u32, Relaxed);
            while !stop.load(Relaxed) && !ext_stop.load(Relaxed) {
                for &a in &addrs {
                    match pics.heartbeat(a) {
                        Ok(()) => board.hb_ok.fetch_add(1, Relaxed),
                        Err(_) => board.hb_err.fetch_add(1, Relaxed),
                    };
                }
                if tick % 10 == 0 {
                    let mut hottest = 0.0f64;
                    for (i, &a) in addrs.iter().enumerate().take(3) {
                        let mut m = 0.0f64;
                        for sensor in 0x48..=0x4b {
                            if let Ok(t) = pics.temperature(a, sensor) {
                                m = m.max(t);
                            }
                        }
                        board.temps[i].store(m.to_bits(), Relaxed);
                        hottest = hottest.max(m);
                    }
                    if fan.auto {
                        // Proportional step towards the target, bounded.
                        let err = hottest - fan.target_temp;
                        pct = (pct + err * 2.0).clamp(f64::from(fan.min_percent.max(20)), 100.0);
                        if hottest == 0.0 {
                            pct = 100.0; // no reading: fail safe
                        }
                    } else {
                        pct = f64::from(fan.manual_percent);
                    }
                    fans.set_pwm(pct.round() as u32);
                    board.fan_percent.store(pct.round() as u32, Relaxed);
                }
                tick += 1;
                std::thread::sleep(Duration::from_millis(1000));
            }
        }))
    }

    pub fn mine(chains: &[Chain], opt: &MineOptions) -> Result<MineReport, String> {
        let mut r = MineReport::default();
        let stop = Arc::new(AtomicBool::new(false));
        let board = Arc::new(Board::default());
        let i2c = if opt.pic_heartbeat.is_empty() {
            None
        } else {
            Some(spawn_i2c_thread(opt, board.clone(), stop.clone())?)
        };
        let result = run(chains, opt, &mut r, &board);
        stop.store(true, Relaxed);
        if let Some(t) = i2c {
            let _ = t.join();
        }
        r.heartbeats_ok = board.hb_ok.load(Relaxed);
        r.heartbeats_failed = board.hb_err.load(Relaxed);
        match result {
            Ok(()) => Ok(r),
            Err(e) => {
                r.stop_reason = e.clone();
                write_status(opt, &r, &board, &[], "pool-error", &e, Instant::now());
                Err(e)
            }
        }
    }

    fn load_best(path: &str) -> f64 {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["best_share"].as_f64())
            .unwrap_or(0.0)
    }

    fn save_best(path: &str, d: f64) {
        let tmp = format!("{path}.tmp");
        if std::fs::write(
            &tmp,
            json!({ "best_share": d, "time": now_s() }).to_string(),
        )
        .is_ok()
        {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// `best-history.json` next to `best.json`: `{"days": [[unix_day, best], ...]}`.
    fn history_path(best_path: &str) -> std::path::PathBuf {
        std::path::Path::new(best_path).with_file_name("best-history.json")
    }

    fn load_history(path: &std::path::Path) -> Vec<(u64, f64)> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| serde_json::from_value(v["days"].clone()).ok())
            .unwrap_or_default()
    }

    fn save_history(path: &std::path::Path, days: &[(u64, f64)]) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json!({ "days": days }).to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    fn now_s() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn write_status(
        opt: &MineOptions,
        r: &MineReport,
        board: &Board,
        per_chain: &[Value],
        state: &str,
        msg: &str,
        start: Instant,
    ) {
        let Some(path) = &opt.status_path else { return };
        let mut v = json!({
            "state": state,
            "message": msg,
            "updated": now_s(),
            "mining_seconds": start.elapsed().as_secs(),
            "pool": opt.host,
            "user": opt.user,
            "hashrate_ths": r.hashrate_ths,
            "chains": per_chain,
            "shares": { "submitted": r.shares_submitted, "accepted": r.shares_accepted, "rejected": r.shares_rejected },
            "pool_difficulty": r.pool_difficulty,
            "best_share": r.best_share,
            "best_share_all_time": r.best_share_all_time,
            "network_difficulty": r.network_difficulty,
            "blocks_found": r.blocks_found,
            "nonces": { "valid": r.valid_diff1, "invalid": r.invalid, "duplicates": r.duplicates },
            "temps_pcb": [f(&board.temps[0]), f(&board.temps[1]), f(&board.temps[2])],
            "fan_percent": board.fan_percent.load(Relaxed),
            "heartbeats": { "ok": board.hb_ok.load(Relaxed), "failed": board.hb_err.load(Relaxed) },
        });
        if let (Some(o), Some(extra)) = (v.as_object_mut(), opt.status_extra.as_object()) {
            for (k, x) in extra {
                o.insert(k.clone(), x.clone());
            }
        }
        let tmp = format!("{path}.tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    fn run(
        chains: &[Chain],
        opt: &MineOptions,
        r: &mut MineReport,
        board: &Board,
    ) -> Result<(), String> {
        let mut pool =
            Pool::connect(&opt.host).map_err(|e| format!("connect {}: {e}", opt.host))?;
        let ioe = |e: io::Error| format!("pool {}: {e}", opt.host);
        let id_configure = pool
            .send(
                "mining.configure",
                json!([["version-rolling"], {"version-rolling.mask": format!("{DEFAULT_MASK:08x}"), "version-rolling.min-bit-count": 3}]),
            )
            .map_err(ioe)?;
        let id_subscribe = pool
            .send("mining.subscribe", json!(["openmineros/0.1.0"]))
            .map_err(ioe)?;
        let id_authorize = pool
            .send("mining.authorize", json!([opt.user, opt.password]))
            .map_err(ioe)?;

        let mut buf = String::new();
        let mut mask = 0u32;
        let mut sub: Option<Subscription> = None;
        let mut job: Option<Notify> = None;
        let mut net_target: Option<[u8; 32]> = None;
        let mut target = target_from_pool_difficulty(1.0).unwrap();
        let diff1 = target_from_pool_difficulty(1.0).unwrap();
        let mut en2: u64 = 0;
        let mut submits: HashSet<u64> = HashSet::new();
        let mut states: Vec<ChainState> = chains
            .iter()
            .map(|c| ChainState {
                chain: c,
                slots: HashMap::new(),
                order: VecDeque::new(),
                seen: HashSet::new(),
                next_ext: 0,
                last_frame: Instant::now(),
                valid: 0,
                window: VecDeque::new(),
            })
            .collect();
        for st in &states {
            st.chain.init_work_tx_threshold();
            st.chain.reset_fifos();
        }
        if let Some(p) = &opt.best_path {
            r.best_share_all_time = r.best_share_all_time.max(load_best(p));
        }
        let hist_path = opt.best_path.as_deref().map(history_path);
        let mut daily = hist_path.as_deref().map(load_history).unwrap_or_default();
        let start = Instant::now();
        let mut last_status = Instant::now() - Duration::from_secs(10);
        let mut last_notify = Instant::now();
        let mut authorized = false;

        loop {
            if opt.seconds != 0 && start.elapsed() >= Duration::from_secs(opt.seconds) {
                r.stop_reason = "time limit".into();
                break;
            }
            if opt.stop.load(Relaxed) {
                r.stop_reason = "stopped".into();
                break;
            }
            let hottest = (0..3).map(|i| f(&board.temps[i])).fold(0.0, f64::max);
            r.max_pcb_temp_seen = r.max_pcb_temp_seen.max(hottest);
            if opt.max_pcb_temp > 0.0 && hottest > opt.max_pcb_temp {
                return Err(format!(
                    "overheat: PCB {hottest:.1} °C above {:.1} °C",
                    opt.max_pcb_temp
                ));
            }
            if board.hb_err.load(Relaxed) > 10 {
                return Err("hashboard voltage controllers stopped answering".into());
            }
            if last_notify.elapsed() > Duration::from_secs(180) {
                return Err(format!("pool {}: no new job for 3 minutes", opt.host));
            }
            let progress_before = r.frames + r.nonces;

            for msg in pool.poll(&mut buf).map_err(ioe)? {
                let id = msg.get("id").and_then(Value::as_u64);
                match msg.get("method").and_then(Value::as_str) {
                    Some("mining.notify") => {
                        let n = Notify::from_params(&msg["params"]).map_err(|e| e.to_string())?;
                        net_target = target_from_bits(n.nbits);
                        r.network_difficulty = network_difficulty(n.nbits);
                        if n.clean_jobs {
                            for st in &mut states {
                                st.slots.clear();
                                st.order.clear();
                            }
                        }
                        job = Some(n);
                        last_notify = Instant::now();
                    }
                    Some("mining.set_difficulty") => {
                        if let Some(d) = msg["params"].get(0).and_then(Value::as_f64) {
                            r.pool_difficulty = d;
                            if let Some(t) = target_from_pool_difficulty(d) {
                                target = t;
                            }
                        }
                    }
                    Some("mining.set_version_mask") => {
                        if let Some(m) = msg["params"].get(0).and_then(Value::as_str) {
                            mask = u32::from_str_radix(m, 16).unwrap_or(0);
                        }
                    }
                    Some(_) => {}
                    None if id == Some(id_configure) => {
                        if msg["result"]["version-rolling"].as_bool() == Some(true) {
                            mask = msg["result"]["version-rolling.mask"]
                                .as_str()
                                .and_then(|m| u32::from_str_radix(m, 16).ok())
                                .unwrap_or(0);
                        }
                    }
                    None if id == Some(id_subscribe) => {
                        sub = Some(
                            Subscription::from_result(&msg["result"])
                                .map_err(|e| format!("subscribe: {e}"))?,
                        );
                    }
                    None if id == Some(id_authorize) => {
                        if msg["result"].as_bool() != Some(true) {
                            return Err(format!(
                                "pool {} rejected worker {}: {}",
                                opt.host, opt.user, msg["error"]
                            ));
                        }
                        authorized = true;
                    }
                    None => {
                        if let Some(i) = id {
                            if submits.remove(&i) {
                                if msg["result"].as_bool() == Some(true) {
                                    r.shares_accepted += 1;
                                } else {
                                    r.shares_rejected += 1;
                                    if r.reject_reasons.len() < 10 {
                                        r.reject_reasons.push(msg["error"].to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            for st in &mut states {
                if let (Some(s), Some(n)) = (&sub, &job) {
                    if st.last_frame.elapsed() >= opt.frame_interval {
                        let mut e2 = en2.to_be_bytes().to_vec();
                        e2 = e2[e2.len().saturating_sub(s.extranonce2_size)..].to_vec();
                        while e2.len() < s.extranonce2_size {
                            e2.insert(0, 0);
                        }
                        let base = n.header(&s.extranonce1, &e2).map_err(|e| e.to_string())?;
                        let mut versions = [0u32; SLOTS];
                        let mut headers = [[0u8; 80]; SLOTS];
                        for (i, v) in versions.iter_mut().enumerate() {
                            *v = if mask == 0 {
                                base.version
                            } else {
                                work::rolled_version(base.version, mask, i as u32)
                            };
                            let mut h = base;
                            h.version = *v;
                            headers[i] = h.to_bytes();
                        }
                        let ext = st.next_ext % RING;
                        let frame = work::frame_versions(
                            ext << ID_SHIFT,
                            &headers,
                            MidstateOrder::Reversed,
                        );
                        if st.chain.push_work(&frame) {
                            st.slots.insert(
                                ext,
                                Slot {
                                    job_id: n.job_id.clone(),
                                    extranonce2: e2,
                                    base,
                                    versions,
                                },
                            );
                            st.order.push_back(ext);
                            if st.order.len() > (RING as usize) / 2 {
                                if let Some(old) = st.order.pop_front() {
                                    st.slots.remove(&old);
                                }
                            }
                            st.next_ext = st.next_ext.wrapping_add(1);
                            en2 = en2.wrapping_add(1);
                            r.frames += 1;
                            st.last_frame = Instant::now();
                        }
                    }
                }

                while let Some((w0, w1)) = st.chain.pop_nonce() {
                    r.nonces += 1;
                    let nw = work::decode_nonce(w0, w1);
                    let ext = u32::from(nw.work_id) >> ID_SHIFT;
                    let slot_idx = (nw.work_id & 7) as usize;
                    let Some(slot) = st.slots.get(&ext) else {
                        r.stale_unknown_id += 1;
                        continue;
                    };
                    if !st.seen.insert((ext, nw.nonce, slot_idx as u8)) {
                        r.duplicates += 1;
                        continue;
                    }
                    let mut h = slot.base;
                    h.version = slot.versions[slot_idx];
                    h.nonce = nw.nonce;
                    let digest = double_sha256(&h.to_bytes());
                    if !meets_target(&digest, &diff1) {
                        r.invalid += 1;
                        continue;
                    }
                    r.valid_diff1 += 1;
                    st.valid += 1;
                    let d = share_difficulty(&digest);
                    if d > r.best_share {
                        r.best_share = d;
                    }
                    // The board has no RTC or NTP: the pool's ntime is the clock.
                    if let Some(hp) = &hist_path {
                        if update_daily_best(&mut daily, u64::from(h.time) / 86_400, d) {
                            save_history(hp, &daily);
                        }
                    }
                    if d > r.best_share_all_time {
                        r.best_share_all_time = d;
                        if let Some(p) = &opt.best_path {
                            save_best(p, d);
                        }
                    }
                    let is_block = net_target.is_some_and(|t| meets_target(&digest, &t));
                    if is_block {
                        r.blocks_found += 1;
                    }
                    if authorized && (is_block || meets_target(&digest, &target)) {
                        let mut params = vec![
                            json!(opt.user),
                            json!(slot.job_id),
                            json!(openmineros_miner_core::hex::encode(&slot.extranonce2)),
                            json!(format!("{:08x}", h.time)),
                            json!(format!("{:08x}", h.nonce)),
                        ];
                        if mask != 0 {
                            params.push(json!(format!("{:08x}", h.version & mask)));
                        }
                        let id = pool
                            .send("mining.submit", Value::Array(params))
                            .map_err(ioe)?;
                        submits.insert(id);
                        r.shares_submitted += 1;
                    }
                }
                if st.seen.len() > 200_000 {
                    st.seen.clear();
                }
            }

            if last_status.elapsed() >= Duration::from_secs(5) {
                let now = Instant::now();
                let mut per_chain = Vec::new();
                let mut total = 0.0;
                for (i, st) in states.iter_mut().enumerate() {
                    st.window.push_back((now, st.valid));
                    while st.window.len() > 1
                        && now.duration_since(st.window[0].0) > Duration::from_secs(60)
                    {
                        st.window.pop_front();
                    }
                    let (t0, v0) = st.window[0];
                    let dt = now.duration_since(t0).as_secs_f64();
                    let ths = if dt > 1.0 {
                        (st.valid - v0) as f64 * 4294967296.0 / dt / 1e12
                    } else {
                        0.0
                    };
                    total += ths;
                    per_chain.push(json!({
                        "chain": st.chain.index,
                        "hashrate_ths": ths,
                        "valid": st.valid,
                        "pcb_temp": f(&board.temps[i.min(2)]),
                    }));
                }
                r.hashrate_ths = total;
                r.seconds = start.elapsed().as_secs_f64();
                r.version_mask = format!("{mask:08x}");
                write_status(opt, r, board, &per_chain, "mining", "", start);
                last_status = now;
            }

            if r.frames + r.nonces == progress_before {
                std::thread::sleep(Duration::from_micros(200));
            }
        }
        r.seconds = start.elapsed().as_secs_f64();
        r.per_chain_valid = states.iter().map(|st| (st.chain.index, st.valid)).collect();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::share_difficulty;

    #[test]
    fn daily_best_keeps_one_maximum_per_day() {
        let mut days = Vec::new();
        assert!(super::update_daily_best(&mut days, 100, 5.0));
        assert!(!super::update_daily_best(&mut days, 100, 3.0));
        assert!(super::update_daily_best(&mut days, 100, 9.0));
        assert!(super::update_daily_best(&mut days, 101, 1.0));
        assert!(!super::update_daily_best(&mut days, 99, 50.0));
        assert_eq!(days, vec![(100, 9.0), (101, 1.0)]);
        for day in 0..(super::HISTORY_DAYS as u64 + 5) {
            super::update_daily_best(&mut days, 200 + day, 1.0);
        }
        assert_eq!(days.len(), super::HISTORY_DAYS);
    }

    #[test]
    fn network_difficulty_from_bits() {
        assert!((super::network_difficulty(0x1d00_ffff) - 1.0).abs() < 1e-9);
        // Block 840000: bits 0x17034219, difficulty ~86.39 T
        let d = super::network_difficulty(0x1703_4219);
        assert!((d / 86.388_558_925_171e12 - 1.0).abs() < 1e-6, "{d}");
    }

    #[test]
    fn difficulty_of_known_hashes() {
        // Hash with exactly the diff-1 target value.
        let mut d = [0u8; 32];
        d[26] = 0xff;
        d[27] = 0xff;
        assert!((share_difficulty(&d) - 1.0).abs() < 1e-9);
        // Half the value = difficulty 2.
        let mut h = [0u8; 32];
        h[26] = 0xff;
        h[27] = 0x7f;
        h[25] = 0x80;
        assert!((share_difficulty(&h) - 2.0).abs() < 1e-6);
    }
}
