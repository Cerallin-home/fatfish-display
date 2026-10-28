//! View 层：只吃 Snapshot，把它画成像素。
use crate::viewmodel::{BubbleVis, Snapshot, H, PANEL_W, W};

use super::canvas::{rgb565, Canvas, Rgb565};
use super::sprites::Sprites;
use super::text::{Fonts, DIM, FG};
use super::video::Clip;

const BG: Rgb565 = rgb565(14, 18, 32);
const PANEL: Rgb565 = rgb565(26, 32, 52);
const TRACK: Rgb565 = rgb565(44, 52, 76);
const BUBBLE: Rgb565 = rgb565(240, 244, 252);
const TEXT_DARK: Rgb565 = rgb565(24, 28, 44);

/// 气泡烘焙成一张小图层：颜色 + 覆盖率，绘制时整体乘透明度
struct BubbleLayer {
    w: i32,
    h: i32,
    px: Vec<u16>,
    cov: Vec<u8>,
}

/// 读一张海洋背景：等比缩放裁切到满屏，转成 RGB565
pub fn load_bg(dir: &str) -> Option<Canvas> {
    let path = format!("{dir}/bg.png");
    let img = image::open(&path).ok()?.to_rgb8();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let scale = (W as f32 / w).max(H as f32 / h);
    let (nw, nh) = ((w * scale).ceil() as u32, (h * scale).ceil() as u32);
    let resized = image::imageops::resize(&img, nw.max(W), nh.max(H), image::imageops::FilterType::Triangle);
    let ox = (resized.width().saturating_sub(W)) / 2;
    let oy = (resized.height().saturating_sub(H)) / 2;
    let mut cv = Canvas::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let px = resized.get_pixel(x + ox, y + oy);
            cv.put(x as i32, y as i32, rgb565(px[0], px[1], px[2]));
        }
    }
    Some(cv)
}

pub struct Panel<'a> {
    pub fonts: &'a Fonts,
    pub sprites: Option<&'a Sprites>,
    /// 海洋背景（480x320），没有就用纯色
    pub bg: Option<Canvas>,
    /// 静态部分（面板）的缓存：只有内容变了才重画
    cache: Option<(String, Canvas)>,
    /// 气泡图层缓存
    layer: Option<(String, BubbleLayer)>,
}

impl<'a> Panel<'a> {
    pub fn new(fonts: &'a Fonts, sprites: Option<&'a Sprites>, bg: Option<Canvas>) -> Self {
        Panel { fonts, sprites, bg, cache: None, layer: None }
    }

    fn static_key(s: &Snapshot) -> String {
        let bars: String = s
            .bars
            .iter()
            .map(|b| format!("{}={}{};", b.label, b.value, b.unit))
            .collect();
        let svc: String = s.services.iter().map(|(n, ok)| format!("{n}:{ok};")).collect();
        let net: String = if s.net.is_empty() {
            "无网口".to_string()
        } else {
            s.net
                .iter()
                .map(|(k, ip)| format!("{k} {}", ip.clone().unwrap_or_else(|| "-".into())))
                .collect::<Vec<_>>()
                .join(";")
        };
        format!("{}|{}|{}|{}|{}", s.clock, s.sub_line, bars, svc, net)
    }
}

impl<'a> Panel<'a> {
    pub fn draw(&mut self, cv: &mut Canvas, s: &Snapshot, down: Option<(&Clip, f32)>) {
        let key = Self::static_key(s);
        let fresh = std::env::var_os("FF_NOCACHE").is_some() || match &self.cache {
            Some((k, _)) => k != &key,
            None => true,
        };
        if fresh {
            let mut buf = Canvas::new(W, H);
            self.draw_static(&mut buf, s);
            self.cache = Some((key, buf));
        }
        let (_, buf) = self.cache.as_ref().unwrap();
        cv.copy_from(buf);
        draw_bubbles(cv, &s.bubbles);
        if let Some((clip, t)) = down {
            // DSH 挂了：立绘让位给躺倒的帧序列，帧自带 alpha，面板和背景照旧露着
            clip.blit(cv, clip.frame_at(t));
            return;
        }
        if let Some(sp) = self.sprites {
            let x0 = PANEL_W as i32 + (W as i32 - PANEL_W as i32 - sp.tile_w as i32) / 2;
            let y0 = H as i32 - sp.tile_h as i32 + 14;
            sp.blit(cv, s.anim.phase, s.anim.blink, s.anim.mouth, x0, y0);
        }
        if let Some(b) = &s.bubble {
            self.draw_bubble(cv, b);
        }
    }

    fn draw_static(&self, cv: &mut Canvas, s: &Snapshot) {
        match &self.bg {
            Some(bg) => cv.copy_from(bg),
            None => cv.clear(BG),
        }
        // 面板带一点点透明度，让海水的光从底下透出来
        let panel_alpha = if self.bg.is_some() { 204 } else { 255 };
        cv.fill_round_rect_aa_alpha(6, 6, PANEL_W as i32 - 12, H as i32 - 12, 10.0, PANEL, panel_alpha);

        // 标题 / 副标题 / 时钟：全部按基线对齐
        self.fonts.draw_base_big(cv, 16, 29, &s.host_line, FG);
        self.fonts.mono10.draw_base(cv, 16, 44, &s.sub_line, DIM, 255);
        let clock_w = self.fonts.width_mixed(&s.clock) as i32;
        self.fonts
            .draw_base(cv, PANEL_W as i32 - 16 - clock_w, 28, &s.clock, DIM);

        // 五个指标：标签和数值共用一条基线，进度条压在基线下面固定距离
        for (i, b) in s.bars.iter().enumerate() {
            let base = 66 + i as i32 * 33;
            let mut cx = 16;
            for c in b.label.chars() {
                let f = self.fonts.pick_small(c);
                f.draw_base(cv, cx, base, &c.to_string(), DIM, 255);
                cx += f.advance(c).max(0);
            }
            let val = format!("{}{}", b.value, b.unit);
            let vw = self.fonts.width_mixed(&val) as i32;
            self.fonts
                .draw_base(cv, PANEL_W as i32 - 18 - vw, base, &val, FG);

            // 药丸形进度条：两端半圆 + 抗锯齿
            let (tx, ty, tw, th) = (16, base + 9, PANEL_W as i32 - 38, 6);
            cv.fill_round_rect_aa(tx, ty, tw, th, 3.0, TRACK);
            let fill = ((tw as f32) * b.ratio.clamp(0.0, 1.0)) as i32;
            if fill > 0 {
                cv.fill_round_rect_aa(tx, ty, fill.max(6), th, 3.0, rgb565(b.color[0], b.color[1], b.color[2]));
            }
        }

        // 服务：两条并排，占一行
        let svc_base = 204;
        self.fonts.draw_base(cv, 16, svc_base, "服务", DIM);
        for (i, (name, ok)) in s.services.iter().enumerate().take(2) {
            let x = 16 + (i as i32 % 2) * 104;
            let base = svc_base + 22 + (i as i32 / 2) * 20;
            let c = if *ok { rgb565(108, 220, 150) } else { rgb565(255, 110, 96) };
            // 绿点按 x-height 居中，不跟基线硬碰
            cv.fill_rect(x, base - 7, 6, 6, c);
            self.fonts.draw_base(cv, x + 12, base, name, FG);
        }

        // 网络：一个网口一行（有线在上、WiFi 在下）。有地址就把最后一段（231/234）挑亮，
        // 没连上就标红的“未连接”，行一直占着，别让网口消失得莫名其妙。
        let net_base = 252;
        self.fonts.draw_base(cv, 16, net_base, "网络", DIM);
        if s.net.is_empty() {
            self.fonts
                .draw_base(cv, 16, net_base + 22, "无网口", rgb565(255, 110, 96));
        }
        let right = PANEL_W as i32 - 18;
        for (i, (kind, ip)) in s.net.iter().enumerate().take(2) {
            let base = net_base + 22 + (i as i32) * 20;
            self.fonts.draw_base(cv, 16, base, kind, FG);
            match ip {
                Some(ip) => {
                    let (head, tail) = match ip.rsplit_once('.') {
                        Some((h, t)) => (format!("{h}."), t),
                        None => (String::new(), ip.as_str()),
                    };
                    let tail_w = self.fonts.width_mixed(tail) as i32;
                    let head_w = self.fonts.width_mixed(&head) as i32;
                    let x_tail = right - tail_w;
                    if !head.is_empty() {
                        self.fonts.draw_base(cv, x_tail - head_w, base, &head, FG);
                    }
                    self.fonts
                        .draw_base(cv, x_tail, base, tail, rgb565(120, 214, 255));
                }
                None => {
                    let w = self.fonts.width_mixed("未连接") as i32;
                    self.fonts
                        .draw_base(cv, right - w, base, "未连接", rgb565(255, 110, 96));
                }
            }
        }
    }

    /// 按宽度折行，最多 max_lines 行，放不下就用省略号收尾
    fn wrap(&self, text: &str, max_w: u32, max_lines: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_w = 0u32;
        let ell = self.fonts.advance('…').max(0) as u32;
        for c in text.chars() {
            if c == '\n' {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
                if lines.len() >= max_lines {
                    break;
                }
                continue;
            }
            let cw = self.fonts.advance(c).max(0) as u32;
            if cur_w + cw > max_w && !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
                if lines.len() >= max_lines {
                    if let Some(last) = lines.last_mut() {
                        while self.fonts.width_mixed(last) + ell > max_w && last.pop().is_some() {}
                        last.push('…');
                    }
                    return lines;
                }
            }
            cur.push(c);
            cur_w += cw;
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    /// 把气泡烘焙成图层（文案或配色变了才重烤）
    fn bake(&mut self, text: &str, warn: bool) {
        let key = format!("{warn}|{text}");
        if let Some((k, _)) = &self.layer {
            if k == &key {
                return;
            }
        }
        let pad: i32 = 10;
        let max_bw = W as i32 - 12 - (PANEL_W as i32 + 6);
        let lines = self.wrap(text, (max_bw - pad * 2) as u32, 2);
        let tw = lines.iter().map(|l| self.fonts.width_mixed(l)).max().unwrap_or(0) as i32;
        let bw = (tw + pad * 2).min(max_bw);
        let line_h = 17;
        let ascent = self.fonts.pick('中').ascent();
        let descent = self.fonts.pick('中').descent();
        let bh = 8 + line_h * (lines.len() as i32 - 1) + ascent + descent + 8;

        // 圆角矩形的覆盖率（角上做一点抗锯齿）
        let r = 9.0f32;
        let mut cov = vec![0u8; (bw * bh) as usize];
        for y in 0..bh {
            for x in 0..bw {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let cx = fx.clamp(r, bw as f32 - r);
                let cy = fy.clamp(r, bh as f32 - r);
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                let c = (r - d + 0.5).clamp(0.0, 1.0);
                cov[(y * bw + x) as usize] = (c * 255.0).round() as u8;
            }
        }

        // 文字覆盖率 → 和底色混合
        let mut tm = vec![0u8; (bw * bh) as usize];
        for (i, line) in lines.iter().enumerate() {
            let baseline = 8 + ascent + (i as i32) * line_h;
            self.fonts.mask_base(&mut tm, bw, bh, pad, baseline, line);
        }
        let (br, bg, bb) = super::canvas::unpack(BUBBLE.0);
        let (tr, tg, tb) = super::canvas::unpack(TEXT_DARK.0);
        let mut px = vec![0u16; (bw * bh) as usize];
        for i in 0..px.len() {
            let t = tm[i] as u32;
            let mix = |a: u8, b: u8| -> u8 { ((a as u32 * (255 - t) + b as u32 * t) / 255) as u8 };
            px[i] = rgb565(mix(br, tr), mix(bg, tg), mix(bb, tb)).0;
        }
        self.layer = Some((key, BubbleLayer { w: bw, h: bh, px, cov }));
    }

    /// 气泡位置：以人物中轴居中，加一点随机横偏移
    fn bubble_origin(&self, bw: i32, b: &BubbleVis) -> (i32, i32) {
        let axis = PANEL_W as i32 + (W as i32 - PANEL_W as i32) / 2;
        let lo = PANEL_W as i32 + 6;
        let hi = (W as i32 - 6 - bw).max(lo);
        let bx = (axis - bw / 2 + b.dx as i32).clamp(lo, hi);
        // 起始摆在人物头顶上方，上浮的全程都在画面里
        let by = 54 + b.dy as i32;
        (bx, by)
    }

    fn draw_bubble(&mut self, cv: &mut Canvas, b: &BubbleVis) {
        self.bake(&b.text, b.warn);
        let Some((_, layer)) = &self.layer else { return };
        let (bx, by) = self.bubble_origin(layer.w, b);
        if b.alpha == 0 {
            return;
        }
        for y in 0..layer.h {
            for x in 0..layer.w {
                let c = layer.cov[(y * layer.w + x) as usize];
                if c == 0 {
                    continue;
                }
                let a = (c as u32 * b.alpha as u32 / 255) as u8;
                if a == 0 {
                    continue;
                }
                cv.blend(bx + x, by + y, Rgb565(layer.px[(y * layer.w + x) as usize]), a);
            }
        }
    }
}

/// 上浮的小气泡：边缘亮、中间透，画在立绘后面才有“在水里”的感觉
fn draw_bubbles(cv: &mut Canvas, bubbles: &[crate::viewmodel::BubbleUp]) {
    let shell = rgb565(178, 232, 255);
    let core = rgb565(96, 176, 232);
    for b in bubbles {
        if b.alpha == 0 {
            continue;
        }
        let r = b.r as f32;
        for dy in -b.r..=b.r {
            for dx in -b.r..=b.r {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                let cov = (r - d + 0.5).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                // 靠近边沿的地方更亮，整体半透
                let t = (d / r.max(0.5)).clamp(0.0, 1.0);
                let c = if t > 0.62 { shell } else { core };
                let a = (cov * b.alpha as f32 * (0.30 + 0.70 * t * t)) as u8;
                if a > 0 {
                    cv.blend(b.x + dx, b.y + dy, c, a);
                }
            }
        }
    }
}
