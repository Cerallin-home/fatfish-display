//! 蓝色大肥鱼桌宠 —— MVVM 版（Rust）
//!
//! model     只采样事实（/proc、/sys、statvfs、systemctl）
//! viewmodel 把事实翻译成画面需要的量（比例、颜色、气泡文案）
//! view      只认 Snapshot，画像素、写屏幕
mod model;
mod view;
mod viewmodel;

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use view::canvas::Canvas;
use view::panel::Panel;
use view::text::Fonts;
use view::{Backend, FbBackend, NullBackend, PngBackend};
use viewmodel::{ViewModel, H, W};

/// fbtft 的 deferred IO 节拍：delay = HZ/fps，本机 HZ=1000、overlay 里 fps=30，所以是 33ms 一次。
/// 应用帧率对齐这个周期（1000/33 ≈ 30.3fps），写帧的相位才不会相对驱动慢慢漂移
/// ——漂移的时候撕裂一会儿有一会儿没有，看着像是「有时候同步、有时候不同步」。
const DRIVER_TICK_MS: f32 = 33.0;
/// 上限留一点余量给对齐用；再往上 SPI 也吃不下，纯属白烧 CPU
const MAX_FPS: f32 = 1000.0 / DRIVER_TICK_MS;
const DEFAULT_FPS: f32 = 1000.0 / DRIVER_TICK_MS;


/// 气泡接口：在 $XDG_RUNTIME_DIR/fatfish-display.sock 上听一句话，
/// 收到就丢给主循环，让立绘上方弹出对话气泡。
fn spawn_say_listener(path: String) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let _ = std::fs::remove_file(&path);
        let listener = match std::os::unix::net::UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("气泡接口没起来 {}: {e}", path);
                return;
            }
        };
        for conn in listener.incoming() {
            let mut stream = match conn {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut buf = String::new();
            use std::io::Read;
            if stream.read_to_string(&mut buf).is_err() {
                continue;
            }
            let text = buf.trim().to_string();
            if !text.is_empty() {
                let _ = tx.send(text);
            }
        }
    });
    rx
}

fn say_socket_path() -> String {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());
    std::env::var("FF_SAY_SOCK").unwrap_or_else(|_| format!("{base}/fatfish-display.sock"))
}

/// 按驱动名找面板：插了 HDMI 之后 SPI 屏会从 fb0 退成 fb1，编号不能写死。
fn detect_fb() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/graphics").ok()?;
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("fb") || name == "fbcon" {
            continue;
        }
        let drv = std::fs::read_to_string(entry.path().join("name")).unwrap_or_default();
        if drv.trim() == "fb_ili9486" {
            return Some(format!("/dev/{name}"));
        }
    }
    None
}

/// 气泡状态文件：写下「当前这条气泡什么时候放完」，播报方据此算「上一条放完后再等几秒」。
/// 用文件而不是回 socket，是因为 socket 那头写完就关，没有应答通道。
fn bubble_state_path() -> String {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());
    format!("{base}/fatfish-display.bubble")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 取本地时:分。别每帧 fork 一个 date，那要 2~3ms，直接 localtime_r。
fn now_clock() -> String {
    unsafe {
        let mut t: libc::time_t = 0;
        libc::time(&mut t);
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return "--:--".into();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let backend_kind = args.get(1).map(|s| s.as_str()).unwrap_or("png");
    let frames: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let asked: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_FPS);
    let fps = if asked <= 0.0 { DEFAULT_FPS } else { asked.min(MAX_FPS) };
    if asked > MAX_FPS {
        println!("帧率按 {:.0} fps 封顶（请求的是 {:.0}）", MAX_FPS, asked);
    }

    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let asset_dir = here.join("assets");
    let fonts = match Fonts::load() {
        Some(f) => f,
        None => {
            eprintln!("字模加载失败：{}", asset_dir.display());
            std::process::exit(1);
        }
    };

    // model -> viewmodel
    let sampler = model::sampler::Sampler::spawn();
    let mut vm = ViewModel::new(sampler.rx);
    // 演示用：FF_SAY 塞一句气泡（气泡在=说话中），FF_HAPPY 把口型峰值放到 100%
    if let Ok(text) = std::env::var("FF_SAY") {
        vm.say(text, false);
    }
    if std::env::var("FF_HAPPY").is_ok() {
        vm.set_happy(true);
    }

    // viewmodel -> view
    let mut cv = Canvas::new(W, H);
    let mut backend: Box<dyn Backend> = if backend_kind == "null" {
        Box::new(NullBackend { frames: 0, dirty_rows_total: 0 })
    } else if backend_kind == "fb" {
        let owned = args
            .get(4)
            .cloned()
            .or_else(detect_fb)
            .unwrap_or_else(|| "/dev/fb0".to_string());
        let path = owned.as_str();
        match FbBackend::open(path) {
            Ok(b) => Box::new(b),
            Err(e) => {
                eprintln!("打不开 {path}: {e}");
                std::process::exit(1);
            }
        }
    } else {
        Box::new(PngBackend::new(here.join("frames")))
    };
    let sprites = view::sprites::Sprites::load(asset_dir.to_str().unwrap());
    if sprites.is_none() {
        eprintln!("警告：立绘图集缺失，先只画面板");
    }
    let bg = view::panel::load_bg(asset_dir.to_str().unwrap_or(""));
    let mut panel = Panel::new(&fonts, sprites.as_ref(), bg);
    let say_rx = spawn_say_listener(say_socket_path());

    let frame_dt = Duration::from_secs_f32(1.0 / fps);
    let t0 = Instant::now();
    let mut n = 0u64;
    let prof = std::env::var("FF_PROF").is_ok();
    let (mut d_snap, mut d_draw, mut d_pres, mut d_over) = (0f64, 0f64, 0f64, 0f64);
    let mut first_render: Option<Instant> = None;
    // 绝对时刻排程：每帧按 t0 + n*frame_dt 睡，不按「本帧耗时剩多少」睡，
    // 否则渲染抖动会累积成帧周期漂移，和驱动 33ms 节拍越走越偏。
    let mut next_at = Instant::now();
    // 上一条气泡的文案，用来认出「换了一条」这件事
    let mut last_bubble: Option<String> = None;
    let bubble_path = bubble_state_path();
    loop {
        let started = Instant::now();
        let clock = now_clock();
        let t = t0.elapsed().as_secs_f64();
        let dbg = std::env::var("FF_DEBUG").is_ok();
        let m0 = Instant::now();
        while let Ok(text) = say_rx.try_recv() {
            vm.say(text, false);
        }
        let Some(snap) = vm.snapshot(clock, t) else {
            next_at += frame_dt;
            let now = Instant::now();
            if next_at > now {
                std::thread::sleep(next_at - now);
            } else {
                next_at = now;
            }
            continue;
        };
        let m1 = Instant::now();
        {
            if dbg {
                eprintln!("t={:.2} phase={} blink={} mouth={}", t, snap.anim.phase, snap.anim.blink, snap.anim.mouth);
            }
            // 气泡开头/结尾各写一次状态文件，播报方不用猜动画多长
            let cur_bubble = snap.bubble.as_ref().map(|b| b.text.clone());
            if cur_bubble != last_bubble {
                if cur_bubble.is_some() {
                    let end = now_ms() + (viewmodel::BUB_LIFETIME * 1000.0) as u128;
                    let _ = std::fs::write(&bubble_path, end.to_string());
                } else {
                    let _ = std::fs::remove_file(&bubble_path);
                }
                last_bubble = cur_bubble;
            }
            panel.draw(&mut cv, &snap);
            let m2 = Instant::now();
            d_snap += (m1 - m0).as_secs_f64();
            d_draw += (m2 - m1).as_secs_f64();
            if let Err(e) = backend.present(&mut cv) {
                eprintln!("present 失败: {e}");
                std::process::exit(1);
            }
            d_pres += m2.elapsed().as_secs_f64();
            if first_render.is_none() {
                first_render = Some(Instant::now());
            }
            n += 1;
            if backend.name() != "fb" && n >= frames {
                break;
            }
        }
        let spent = started.elapsed();
        d_over += spent.as_secs_f64() - (d_snap + d_draw + d_pres).min(0.0) * 0.0;
        next_at += frame_dt;
        let now = Instant::now();
        if next_at > now {
            std::thread::sleep(next_at - now);
        } else {
            next_at = now;
        }
    }
    // 首帧前的等待（等采样线程第一次出数据）不算进帧率
    let secs = first_render.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0).max(1e-6);
    let frames_n = (n.saturating_sub(1)).max(1) as f32;
    println!("跑了 {} 帧，用时 {:.2}s（{:.1} ms/帧，{:.1} fps 实际 / {:.0} fps 上限）", n, secs,
             secs * 1000.0 / frames_n, frames_n / secs, fps);
    if prof {
        let f = n as f64;
        println!(
            "拆分：viewmodel {:.3} ms · 绘制 {:.3} ms · 送屏 {:.3} ms · 合成合计 {:.3} ms · 其它 {:.3} ms",
            d_snap * 1000.0 / f,
            d_draw * 1000.0 / f,
            d_pres * 1000.0 / f,
            (d_snap + d_draw) * 1000.0 / f,
            ((d_over * 1000.0 / f) - (d_snap + d_draw + d_pres) * 1000.0 / f).max(0.0)
        );
    }
    if let Some(nb) = backend.as_any().downcast_ref::<NullBackend>() {
        let rows = nb.dirty_rows_total as f32 / n as f32;
        println!("平均脏行 {:.0} 行/帧（={:.0} KB/帧 SPI 传输）", rows, rows * 480.0 * 2.0 / 1024.0);
    }
}
