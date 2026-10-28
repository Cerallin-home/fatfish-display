//! 画布：RGB565 双缓冲 + 脏行差分。
//! 每帧先画进 back，再和 front 比行，只有变化的行才推给屏幕。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb565(pub u16);

pub const fn rgb565(r: u8, g: u8, b: u8) -> Rgb565 {
    let v = ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3);
    Rgb565(v)
}

pub struct Canvas {
    pub w: u32,
    pub h: u32,
    /// 影子帧：只用来和当前帧比差异，不做页翻转
    shadow: Vec<u16>,
    back: Vec<u16>,
    dirty: Vec<(u32, u32)>, // 连续脏行区间

}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        let n = (w * h) as usize;
        Canvas {
            w,
            h,
            shadow: vec![0; n],
            back: vec![0; n],
            dirty: Vec::new(),
        }
    }

    /// 整块拷贝（从缓存好的静态底图恢复），比重画快一个数量级
    pub fn copy_from(&mut self, other: &Canvas) {
        self.back.copy_from_slice(&other.back);
    }

    pub fn clear(&mut self, c: Rgb565) {
        self.back.fill(c.0);
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: Rgb565) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        self.back[(y as u32 * self.w + x as u32) as usize] = c.0;
    }

    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: Rgb565, a: u8) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || a == 0 {
            return;
        }
        let idx = (y as u32 * self.w + x as u32) as usize;
        if a == 255 {
            self.back[idx] = c.0;
            return;
        }
        let dst = self.back[idx];
        let (dr, dg, db) = unpack(dst);
        let (sr, sg, sb) = unpack(c.0);
        let mix = |d: u8, s: u8| -> u16 {
            ((d as u32 * (255 - a as u32) + s as u32 * a as u32) / 255) as u16
        };
        self.back[idx] = (((mix(dr, sr) & 0xF8) << 8) | ((mix(dg, sg) & 0xFC) << 3) | (mix(db, sb) >> 3)) as u16;
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb565) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }

    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, c: Rgb565) {
        if r <= 0 {
            self.fill_rect(x, y, w, h, c);
            return;
        }
        for yy in 0..h {
            for xx in 0..w {
                let (cx, cy) = (xx as i32, yy as i32);
                let (dx, dy) = if cx < r && cy < r {
                    (r - cx, r - cy)
                } else if cx >= w - r && cy < r {
                    (cx - (w - r - 1), r - cy)
                } else if cx < r && cy >= h - r {
                    (r - cx, cy - (h - r - 1))
                } else if cx >= w - r && cy >= h - r {
                    (cx - (w - r - 1), cy - (h - r - 1))
                } else {
                    self.put(x + cx, y + cy, c);
                    continue;
                };
                if dx * dx + dy * dy <= r * r {
                    self.put(x + cx, y + cy, c);
                }
            }
        }
    }

    /// 抗锯齿圆角矩形：r 可以给到 h/2，两端就是完整的半圆（药丸形）
    pub fn fill_round_rect_aa(&mut self, x: i32, y: i32, w: i32, h: i32, r: f32, c: Rgb565) {
        if w <= 0 || h <= 0 {
            return;
        }
        let hw = w as f32 * 0.5;
        let hh = h as f32 * 0.5;
        let r = r.max(0.0).min(hw).min(hh);
        let cx = x as f32 + hw;
        let cy = y as f32 + hh;
        let (qx, qy) = ((hw - r).max(0.0), (hh - r).max(0.0));
        for yy in (y - 1)..(y + h + 1) {
            for xx in (x - 1)..(x + w + 1) {
                let px = xx as f32 + 0.5 - cx;
                let py = yy as f32 + 0.5 - cy;
                let dx = px.abs() - qx;
                let dy = py.abs() - qy;
                let d = if dx > 0.0 || dy > 0.0 {
                    (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - r
                } else {
                    dx.max(dy) - r
                };
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend(xx, yy, c, (cov * 255.0).round() as u8);
                }
            }
        }
    }

    /// 把 back 里和 front 不同的连续行挑出来（脏行差分）。
    pub fn commit(&mut self) -> &[u16] {
        self.dirty.clear();
        let w = self.w as usize;
        let mut start: Option<u32> = None;
        for y in 0..self.h {
            let row = y as usize * w;
            let changed = self.back[row..row + w] != self.shadow[row..row + w];
            match (changed, start) {
                (true, None) => start = Some(y),
                (false, Some(s)) => {
                    self.dirty.push((s, y));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            self.dirty.push((s, self.h));
        }
        // 把这一帧记进影子帧，下次才能比出变化行
        for &(a, b) in &self.dirty {
            let s = (a * self.w) as usize;
            let e = (b * self.w) as usize;
            self.shadow[s..e].copy_from_slice(&self.back[s..e]);
        }
        &self.back
    }

    pub fn dirty_rows(&self) -> &[(u32, u32)] {
        &self.dirty
    }

    pub fn back_buffer(&self) -> &[u16] {
        &self.back
    }
}

#[inline]
pub fn unpack(v: u16) -> (u8, u8, u8) {
    let r = ((v >> 11) & 0x1F) as u8;
    let g = ((v >> 5) & 0x3F) as u8;
    let b = (v & 0x1F) as u8;
    ((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))
}
