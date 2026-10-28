//! ViewModel 层：把 Model 的事实翻译成“画面需要的量”。
//! View 只认 Snapshot，永远碰不到 Model，也就不会到处乱读。
use std::sync::mpsc::Receiver;

use crate::model::SystemState;

pub const PANEL_W: u32 = 232;
pub const W: u32 = 480;
pub const H: u32 = 320;

#[derive(Clone, Debug)]
pub struct Bar {
    pub label: &'static str,
    pub value: String,
    pub unit: &'static str,
    pub ratio: f32,
    pub color: [u8; 3],
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub clock: String,
    pub host_line: String,
    pub sub_line: String,
    pub bars: Vec<Bar>,
    pub services: Vec<(String, bool)>,
    /// 网络：("有线"/"WiFi", Option<IPv4>)，物理网口各一条，没地址的也会在
    pub net: Vec<(String, Option<String>)>,
    pub bubble: Option<BubbleVis>,
    pub tick: u64,
    pub stale_ms: u128,
    pub anim: AnimFrame,
}

/// 动画状态：只暴露“第几帧”，不暴露表情细节
#[derive(Clone, Copy, Debug)]
pub struct AnimFrame {
    pub phase: u32,
    pub blink: usize,
    pub mouth: usize,
}

const PHASES: u32 = 45;
const PHASE_SPEED: f64 = 30.0 / 45.0;   // 30fps 下每帧正好推进一个相位，一轮 1.5s
const BLINK_PERIOD: f64 = 4.2;

fn blink_amount(t: f64) -> f64 {
    let p = (t + 2.1) % BLINK_PERIOD;
    if p < 0.16 {
        1.0
    } else {
        0.0
    }
}

/// 口型档位下标：0=闭 1=15% 2=50% 3=90% 4=100%
const MOUTH_NORMAL: usize = 3;   // 平常封顶 90%
const MOUTH_HAPPY: usize = 4;    // 开心才冲到 100%

/// 说话节奏：(持续秒, 张开强度 0~1)
const TALK_PATTERN: [(f64, f32); 10] = [
    (0.18, 0.0),
    (0.22, 0.62),
    (0.10, 0.15),
    (0.28, 1.0),
    (0.12, 0.0),
    (0.20, 0.45),
    (0.14, 0.0),
    (0.24, 0.85),
    (0.10, 0.25),
    (0.18, 0.0),
];
const TALK_CYCLE: f64 = 1.86;

/// 强度 → 档位下标（0/15/50/90/100），并按峰值封顶
fn level_index(strength: f32, peak: usize) -> usize {
    let idx = if strength < 0.08 {
        0
    } else if strength < 0.32 {
        1
    } else if strength < 0.70 {
        2
    } else {
        3
    };
    idx.min(peak)
}

pub fn mouth_level(t: f64, talking: bool, happy: bool) -> usize {
    if !talking {
        return 0;
    }
    let peak = if happy { MOUTH_HAPPY } else { MOUTH_NORMAL };
    let mut acc = 0.0;
    let p = t % TALK_CYCLE;
    for (dur, strength) in TALK_PATTERN {
        if p < acc + dur {
            return level_index(strength, peak);
        }
        acc += dur;
    }
    0
}

/// 气泡当前这一帧长什么样
#[derive(Clone, Debug)]
pub struct BubbleVis {
    pub text: String,
    pub warn: bool,
    /// 相对人物中轴的横向随机偏移
    pub dx: f32,
    /// 上下位移（负数是往上）
    pub dy: f32,
    pub alpha: u8,
}

struct BubbleState {
    text: String,
    warn: bool,
    start: f64,
    dx: f32,
}

/// 出现后实心停留
const BUB_HOLD: f64 = 0.30;
/// 抖一下的动作时长（上去、回来、落定）
const BUB_SHAKE: f64 = 0.50;
/// 顶上也停这么久，落回来之后再停这么久
const BUB_PAUSE: f64 = 1.20;
const BUB_PAUSE_TOP: f64 = 1.20;
const BUB_BOUNCES: usize = 3;
/// 最后上浮 + 淡出
const BUB_FLOAT: f64 = 1.30;
const BUB_RISE: f32 = 40.0;
const BUB_AMP: f32 = 2.0;

fn ease_out(p: f32) -> f32 {
    1.0 - (1.0 - p) * (1.0 - p)
}

/// 一次抖动内部的位移：上去 → 回来带一点点过冲 → 落定 → 停
/// 抬上去 / 落回来各只占一帧，中间停顿靠 BUB_PAUSE_TOP / BUB_PAUSE
const BUB_STEP: f64 = 1.0 / 30.0;

fn bounce_dy(u: f32) -> f32 {
    // 一帧抬到顶，停住；再一帧落回原处，停住
    if (u as f64) < BUB_STEP + BUB_PAUSE_TOP {
        -BUB_AMP
    } else {
        0.0
    }
}

fn bubble_vis(b: &BubbleState, t: f64) -> Option<BubbleVis> {
    let age = t - b.start;
    if age < 0.0 {
        return None;
    }
    let cycle = BUB_STEP + BUB_PAUSE_TOP + BUB_STEP + BUB_PAUSE;
    let jitter_end = BUB_HOLD + cycle * BUB_BOUNCES as f64;
    let (dy, alpha) = if age < BUB_HOLD {
        (0.0, 255u8)
    } else if age < jitter_end {
        // 每轮：前面 SHAKE 秒抖动，后面 PAUSE 秒停住不动
        let u = ((age - BUB_HOLD) % cycle) as f32;
        (bounce_dy(u), 255)
    } else {
        let p = ((age - jitter_end) / BUB_FLOAT).min(1.0) as f32;
        if p >= 1.0 {
            return None;
        }
        // 慢慢飘上去：起步和收尾都软一点
        let dy = -BUB_RISE * (p * p * (3.0 - 2.0 * p));
        // 前 1/4 保持实心，后面才逐渐透明
        let fade = ((p - 0.25) / 0.75).clamp(0.0, 1.0);
        let a = (255.0 * (1.0 - fade).powf(1.15)).round() as u8;
        (dy, a.max(0))
    };
    Some(BubbleVis { text: b.text.clone(), warn: b.warn, dx: b.dx, dy, alpha })
}

pub struct ViewModel {
    rx: Receiver<SystemState>,
    latest: Option<SystemState>,
    tick: u64,
    pending: Option<(String, bool)>,
    bubble_state: Option<BubbleState>,
    last_warn: f64,
    rng: u64,
    happy: bool,
}

const ACCENT: [u8; 3] = [74, 158, 255];
const WARN: [u8; 3] = [255, 138, 76];

impl ViewModel {
    pub fn new(rx: Receiver<SystemState>) -> Self {
        ViewModel {
            rx,
            latest: None,
            tick: 0,
            pending: None,
            bubble_state: None,
            last_warn: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
            happy: false,
        }
    }

    pub fn set_happy(&mut self, happy: bool) {
        self.happy = happy;
    }

    /// 时间驱动的动画帧
    pub fn anim(&self, t: f64, talking: bool) -> AnimFrame {
        AnimFrame {
            phase: ((t * PHASE_SPEED * PHASES as f64) as u32) % PHASES,
            blink: if blink_amount(t) > 0.5 { 1 } else { 0 },
            mouth: mouth_level(t, talking, self.happy),
        }
    }

    /// 冒一句话：下次取快照时开始播放动画（重复调用会重新来一遍）
    pub fn say(&mut self, text: impl Into<String>, warn: bool) {
        self.pending = Some((text.into(), warn));
    }

    fn next_offset(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        ((self.rng >> 33) as i32 % 25 - 12) as f32
    }

    fn drain(&mut self) {
        while let Ok(st) = self.rx.try_recv() {
            self.latest = Some(st);
        }
    }

    /// 每帧调一次；返回 None 表示数据还没到。
    pub fn snapshot(&mut self, clock: String, t: f64) -> Option<Snapshot> {
        self.drain();
        self.tick += 1;
        let st = self.latest.clone()?;
        let temp_warn = st.cpu_temp_c >= 65.0;
        let any_bad = st.services.iter().any(|s| !s.ok);
        let bars = vec![
            Bar { label: "CPU", value: format!("{:.0}", st.cpu_pct), unit: "%", ratio: st.cpu_pct / 100.0, color: ACCENT },
            Bar {
                label: "内存",
                value: format!("{:.1}/{:.1}G", st.mem_used_gb, st.mem_total_gb),
                unit: "",
                ratio: if st.mem_total_gb > 0.0 { st.mem_used_gb / st.mem_total_gb } else { 0.0 },
                color: ACCENT,
            },
            Bar {
                label: "温度",
                value: format!("{:.0}", st.cpu_temp_c),
                unit: "℃",
                ratio: st.cpu_temp_c / 85.0,
                color: if temp_warn { WARN } else { ACCENT },
            },
            Bar {
                label: "负载",
                value: format!("{:.2}", st.load1),
                unit: "",
                ratio: (st.load1 / st.cores.max(1) as f32).min(1.0),
                color: ACCENT,
            },
        ];
        // 警报气泡：没人说话时，每 30 秒最多冒一次
        if self.pending.is_none() && self.bubble_state.is_none() && t - self.last_warn >= 30.0 {
            if temp_warn {
                self.pending = Some((format!("有点烫，{:.0}℃ 了", st.cpu_temp_c), true));
                self.last_warn = t;
            } else if any_bad {
                let names: Vec<&str> = st.services.iter().filter(|s| !s.ok).map(|s| s.name.as_str()).collect();
                self.pending = Some((format!("{} 没在跑", names.join("、")), true));
                self.last_warn = t;
            }
        }
        if let Some((text, warn)) = self.pending.take() {
            let dx = self.next_offset();
            self.bubble_state = Some(BubbleState { text, warn, start: t, dx });
        }
        let bubble = match self.bubble_state.as_ref() {
            Some(b) => {
                let vis = bubble_vis(b, t);
                if vis.is_none() {
                    self.bubble_state = None;
                }
                vis
            }
            None => None,
        };
        Some(Snapshot {
            clock,
            host_line: "肥鱼窝".to_string(),
            sub_line: format!("rpi5 · ubuntu 26.04"),
            bars,
            services: st.services.iter().map(|s| (s.name.clone(), s.ok)).collect(),
            net: st.net.iter().map(|n| (n.kind.to_string(), n.ipv4.clone())).collect(),
            anim: self.anim(t, bubble.is_some()),
            bubble,
            tick: self.tick,
            stale_ms: st.age_ms(),
        })
    }
}
