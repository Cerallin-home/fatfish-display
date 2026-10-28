//! 采样线程：对齐到整秒醒来一次，读 /proc、/sys、statvfs、systemctl。
//! CPU 占用不再自己睡 120ms 采两次，而是拿相邻两次采样的 /proc/stat 差值，
//! 这样既是真·1 秒平均，也不用额外占着线程。
use std::fs;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::{NetState, ServiceState, SystemState};

/// (显示名, systemd unit) —— glances 没有 user unit，改成探端口
/// 只盯这两条，屏幕就这么点大，别的看日志去。
const SERVICES: [(&str, &str); 2] = [
    ("dsh", "dsh-qqbot.service"),
    ("glances", ""),
];
const GLANCES_PORT: u16 = 61208;

pub struct Sampler {
    pub rx: Receiver<SystemState>,
}

impl Sampler {
    pub fn spawn() -> Sampler {
        let (tx, rx): (Sender<SystemState>, Receiver<SystemState>) = channel();
        thread::Builder::new()
            .name("sampler".into())
            .spawn(move || run(tx))
            .expect("spawn sampler");
        Sampler { rx }
    }
}

/// 睡到下一个整秒（按墙钟对齐，误差累积为零）
fn sleep_to_next_second() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let target_ns = ((now.as_nanos() / 1_000_000_000) + 1) * 1_000_000_000;
    let wait = target_ns.saturating_sub(now.as_nanos());
    let ts = libc::timespec {
        tv_sec: (wait / 1_000_000_000) as libc::time_t,
        tv_nsec: (wait % 1_000_000_000) as libc::c_long,
    };
    unsafe {
        libc::nanosleep(&ts, std::ptr::null_mut());
    }
}

fn cpu_times() -> (u64, u64) {
    let s = fs::read_to_string("/proc/stat").unwrap_or_default();
    let line = s.lines().next().unwrap_or("");
    let mut idle = 0u64;
    let mut total = 0u64;
    for (i, f) in line.split_whitespace().skip(1).enumerate() {
        let v: u64 = f.parse().unwrap_or(0);
        total += v;
        if i == 3 || i == 4 {
            idle += v;
        }
    }
    (idle, total)
}

fn meminfo() -> (f32, f32) {
    let s = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0.0;
    let mut avail = 0.0;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = rest.trim().split_whitespace().next().unwrap_or("0").parse::<f32>().unwrap_or(0.0) / 1024.0 / 1024.0;
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            avail = rest.trim().split_whitespace().next().unwrap_or("0").parse::<f32>().unwrap_or(0.0) / 1024.0 / 1024.0;
        }
    }
    (total - avail, total)
}

/// 列出物理网口（有线在上、WiFi 在下），每个都带上自己的 IPv4。
/// 没连上的网口也照样留着，界面标红“未连接”，别让它凭空消失。
fn net_states() -> Vec<NetState> {
    use std::collections::HashMap;
    let mut addrs: HashMap<String, String> = HashMap::new();
    if let Ok(out) = Command::new("ip").args(["-4", "-o", "addr", "show"]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            // `2: eth0    inet 192.168.1.5/24 brd ... scope global dynamic eth0`
            let mut it = line.split_whitespace();
            let _idx = it.next();
            let iface = it.next().unwrap_or("").trim_end_matches(':').to_string();
            let mut ip = String::new();
            let mut scope = "";
            while let Some(tok) = it.next() {
                match tok {
                    "inet" => ip = it.next().unwrap_or("").split('/').next().unwrap_or("").to_string(),
                    "scope" => scope = it.next().unwrap_or(""),
                    _ => {}
                }
            }
            if scope == "global" && !ip.starts_with("169.254.") {
                addrs.entry(iface).or_insert(ip);
            }
        }
    }

    let mut wired: Vec<NetState> = Vec::new();
    let mut wifi: Vec<NetState> = Vec::new();
    for entry in fs::read_dir("/sys/class/net").into_iter().flatten().flatten() {
        let iface = entry.file_name().to_string_lossy().to_string();
        if iface == "lo" {
            continue;
        }
        let dir = entry.path();
        // 只认有 device 的（物理网口）；docker0 / veth 这些虚拟口不看
        if !dir.join("device").exists() {
            continue;
        }
        let wireless = dir.join("wireless").exists();
        let ipv4 = addrs.get(&iface).cloned();
        let st = NetState { kind: if wireless { "WiFi" } else { "有线" }, iface, ipv4 };
        if wireless {
            wifi.push(st);
        } else {
            wired.push(st);
        }
    }
    wired.sort_by(|a, b| a.iface.cmp(&b.iface));
    wifi.sort_by(|a, b| a.iface.cmp(&b.iface));
    wired.into_iter().chain(wifi).collect()
}

fn temp_c() -> f32 {
    if let Ok(s) = fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
        if let Ok(v) = s.trim().parse::<f32>() {
            return v / 1000.0;
        }
    }
    0.0
}

fn host_and_kernel() -> (String, String) {
    let host = fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_string();
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string();
    (host, kernel)
}

fn port_open(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// 启动时同步探一次：开机阶段面板可能比 dsh 先起来，得立刻知道该站着还是躺着。
/// 返回 None 表示压根没探成（systemctl 都跑不起来），这时候别乱下结论。
pub fn probe_unit_active(unit: &str) -> Option<bool> {
    let out = Command::new("systemctl").arg("--user").arg("is-active").arg(unit).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().next().map(|l| l.trim() == "active").unwrap_or(false))
}

/// 一次 systemctl 拿全部 unit 状态，别为每个服务 fork 一次
fn service_states() -> Vec<ServiceState> {
    // dsh 的 unit 允许用 FF_DSH_UNIT 换掉（测试拿别的 unit 模拟挂掉/恢复）
    let dsh_unit = std::env::var("FF_DSH_UNIT").unwrap_or_else(|_| "dsh-qqbot.service".to_string());
    let unit_of = |label: &str, unit: &str| -> String {
        if label == "dsh" {
            dsh_unit.clone()
        } else {
            unit.to_string()
        }
    };
    let units: Vec<String> = SERVICES
        .iter()
        .map(|(l, u)| unit_of(l, u))
        .filter(|u| !u.is_empty())
        .collect();
    let mut states: Vec<String> = Vec::new();
    if !units.is_empty() {
        if let Ok(out) = Command::new("systemctl").arg("--user").arg("is-active").args(&units).output() {
            states = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .collect();
        }
    }
    let mut ui = 0usize;
    SERVICES
        .iter()
        .map(|(label, unit)| {
            let unit = &unit_of(label, unit);
            let ok = if unit.is_empty() {
                port_open(GLANCES_PORT)
            } else {
                let v = states.get(ui).map(|s| s == "active").unwrap_or(false);
                ui += 1;
                v
            };
            ServiceState { name: label.to_string(), ok }
        })
        .collect()
}

fn run(tx: Sender<SystemState>) {
    let mut prev = cpu_times();
    let mut last_err = Instant::now();
    loop {
        sleep_to_next_second();
        let cur = cpu_times();
        let dt = cur.1.saturating_sub(prev.1) as f32;
        let di = cur.0.saturating_sub(prev.0) as f32;
        prev = cur;
        let cpu_pct = if dt > 0.0 { (1.0 - di / dt) * 100.0 } else { 0.0 };

        let (mem_used, mem_total) = meminfo();
        let load1 = fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|s| s.split_whitespace().next().map(|v| v.parse::<f32>().ok()))
            .flatten()
            .unwrap_or(0.0);
        let (host, kernel) = host_and_kernel();
        let st = SystemState {
            stamp: Instant::now(),
            host,
            kernel: short_kernel(&kernel),
            cpu_pct,
            mem_used_gb: mem_used,
            mem_total_gb: mem_total,
            cpu_temp_c: temp_c(),
            load1,
            cores: thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
            services: service_states(),
            net: net_states(),
        };
        if tx.send(st).is_err() {
            break;
        }
        if last_err.elapsed() > Duration::from_secs(600) {
            last_err = Instant::now();
        }
    }
}

fn short_kernel(k: &str) -> String {
    k.split('-').take(2).collect::<Vec<_>>().join("-")
}
