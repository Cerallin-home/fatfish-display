//! DSH 可用性判定：只做去抖，观测值来自采样线程每秒的 systemctl 结果。
//!
//! 「可用」= dsh-qqbot.service 处于 active。不自己 fork、不猜端口：
//! 采样线程本来每秒就要 systemctl 一次，顺手把答案带上来就行。
//! 连续 need 次观测一致才算数，免得一次抖动就摔一跤。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Up,
    Down,
}

/// 去抖之后产生的两种结果：第一次拿到定论 / 之后每一次翻转
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    First(Health),
    Flip(Health),
}

pub struct Watcher {
    need: u32,
    last: Option<bool>,
    streak: u32,
    cur: Option<Health>,
}

impl Watcher {
    pub fn new(need: u32) -> Self {
        Watcher { need: need.max(1), last: None, streak: 0, cur: None }
    }

    /// 喂一次观测。返回 Some 表示「这次可以动画面了」。
    pub fn feed(&mut self, alive: bool) -> Option<Event> {
        match self.last {
            Some(prev) if prev == alive => self.streak += 1,
            _ => {
                self.last = Some(alive);
                self.streak = 1;
            }
        }
        if self.streak < self.need {
            return None;
        }
        let now = if alive { Health::Up } else { Health::Down };
        match self.cur {
            Some(c) if c == now => None,
            Some(_) => {
                self.cur = Some(now);
                Some(Event::Flip(now))
            }
            None => {
                self.cur = Some(now);
                Some(Event::First(now))
            }
        }
    }

    /// 启动时已经直接探过一发：把结论钉成当前状态，不用再去抖一次
    pub fn seed(&mut self, h: Health) {
        self.cur = Some(h);
        self.last = Some(h == Health::Up);
        self.streak = self.need;
    }

    /// 已经确认过的状态（没确认过就是 None）
    pub fn current(&self) -> Option<Health> {
        self.cur
    }
}
