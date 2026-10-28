//! 字体：直接读系统里的 TTF/TTC，用 ab_glyph 现算轮廓，不再预烤字模图。
//!
//! 以前是 Python 把源码里出现过的汉字烤成 font.png + font.json，运行时只贴图。
//! 好处是零依赖、贴图快；坏处是字模里没有的字画不出来（运行时冒出来的气泡文案
//! 会缺字），换个字体还得重烤。现在改成运行时栅格化：每个字形第一次用到时算一遍，
//! 结果缓存在 HashMap 里，之后就是贴小位图，静态层又是缓存好的，帧率不受影响。
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ab_glyph::{Font as _, FontArc, FontVec, PxScale, ScaleFont};

use super::canvas::{rgb565, Canvas, Rgb565};

const CJK_FONT: &str = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc";
/// Noto CJK 的 ttc 里依次是 JP / KR / SC / TC / HK，简体在第 3 个
const CJK_INDEX: u32 = 2;
const MONO_BOLD: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf";
const MONO: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf";

struct CachedGlyph {
    left: i32,
    top: i32,
    w: u32,
    h: u32,
    cov: Vec<u8>,
}

pub struct Font {
    font: FontArc,
    scale: PxScale,
    ascent: i32,
    descent: i32,
    line_h: u32,
    cache: RefCell<HashMap<char, Option<Rc<CachedGlyph>>>>,
}

impl Font {
    /// 从磁盘读一份字体（TTC 要指定第几个），同一份数据可以拿去做多个字号
    pub fn open(path: &str, index: u32) -> Option<FontArc> {
        let data = std::fs::read(path).ok()?;
        Some(FontArc::new(FontVec::try_from_vec_and_index(data, index).ok()?))
    }

    /// 用已有的字体数据开一个字号；FontArc 是 Arc，多字号共享同一份数据，不重复占内存
    ///
    /// 注意 ab_glyph 的 PxScale 是按「ascent - descent」当高度的，不是按 em 方块，
    /// 直接传 em 尺寸会整体缩水（Noto CJK 只剩 0.69 倍）。这里按 em 换算一下，
    /// 传进来的 px 就是字面意思的 em 像素。
    pub fn at_size(font: FontArc, px: f32) -> Font {
        let upem = font.units_per_em().unwrap_or(1.0) as f32;
        let span = font.height_unscaled();
        let scale = PxScale::from(px * span / upem);
        let sf = font.as_scaled(scale);
        Font {
            scale,
            ascent: sf.ascent().round() as i32,
            descent: sf.descent().abs().round() as i32,
            line_h: (sf.ascent() - sf.descent()).ceil() as u32,
            cache: RefCell::new(HashMap::new()),
            font,
        }
    }

    /// 这个字体里有这个字形吗（ab_glyph 找不到时给 GlyphId(0)）
    pub fn has_glyph(&self, c: char) -> bool {
        !c.is_whitespace() && self.font.glyph_id(c).0 != 0
    }

    pub fn line_h(&self) -> u32 {
        self.line_h
    }

    pub fn ascent(&self) -> i32 {
        self.ascent
    }

    pub fn descent(&self) -> i32 {
        self.descent
    }

    pub fn advance(&self, c: char) -> i32 {
        let sf = self.font.as_scaled(self.scale);
        sf.h_advance(self.font.glyph_id(c)).round() as i32
    }

    pub fn width(&self, text: &str) -> u32 {
        text.chars().map(|c| self.advance(c).max(0) as u32).sum()
    }

    fn glyph(&self, c: char) -> Option<Rc<CachedGlyph>> {
        if let Some(v) = self.cache.borrow().get(&c) {
            return v.clone();
        }
        let rc = self.raster(c).map(Rc::new);
        self.cache.borrow_mut().insert(c, rc.clone());
        rc
    }

    fn raster(&self, c: char) -> Option<CachedGlyph> {
        let g = self.font.glyph_id(c).with_scale(self.scale);
        let outlined = self.font.outline_glyph(g)?;
        let bb = outlined.px_bounds();
        let w = bb.width().ceil() as u32;
        let h = bb.height().ceil() as u32;
        if w == 0 || h == 0 {
            return None;
        }
        let mut cov = vec![0u8; (w * h) as usize];
        outlined.draw(|x, y, v| {
            let (x, y) = (x as u32, y as u32);
            if x < w && y < h {
                cov[(y * w + x) as usize] = (v * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        });
        Some(CachedGlyph { left: bb.min.x.floor() as i32, top: bb.min.y.floor() as i32, w, h, cov })
    }

    /// 按基线画：baseline 是文字底边（不含下伸部），不同字号共用同一条基线才对得齐
    pub fn draw_base(&self, cv: &mut Canvas, x: i32, baseline: i32, text: &str, color: Rgb565, alpha: u8) {
        self.ink(cv, x, baseline, text, color, alpha)
    }

    /// 左上角对齐画（y 是 em 方块顶边）
    pub fn draw(&self, cv: &mut Canvas, x: i32, y: i32, text: &str, color: Rgb565, alpha: u8) {
        self.ink(cv, x, y + self.ascent, text, color, alpha)
    }

    /// 把字形覆盖率写进灰度 mask（x, baseline 与 draw_base 一致），用来做半透明小图层
    pub fn mask(&self, buf: &mut [u8], w: i32, h: i32, mut x: i32, baseline: i32, text: &str) {
        for c in text.chars() {
            let adv = self.advance(c).max(0);
            if let Some(g) = self.glyph(c) {
                for gy in 0..g.h {
                    for gx in 0..g.w {
                        let a = g.cov[(gy * g.w + gx) as usize];
                        if a == 0 {
                            continue;
                        }
                        let px = x + g.left + gx as i32;
                        let py = baseline + g.top + gy as i32;
                        if px < 0 || py < 0 || px >= w || py >= h {
                            continue;
                        }
                        let i = (py * w + px) as usize;
                        if a > buf[i] {
                            buf[i] = a;
                        }
                    }
                }
            }
            x += adv;
        }
    }

    /// 真正落笔：baseline 是基线，字形的 bb 本来就是相对基线量的
    fn ink(&self, cv: &mut Canvas, mut x: i32, baseline: i32, text: &str, color: Rgb565, alpha: u8) {
        for c in text.chars() {
            let adv = self.advance(c).max(0);
            let Some(g) = self.glyph(c) else {
                x += adv;
                continue;
            };
            for gy in 0..g.h {
                for gx in 0..g.w {
                    let a = g.cov[(gy * g.w + gx) as usize];
                    if a == 0 {
                        continue;
                    }
                    let eff = (a as u32 * alpha as u32 / 255) as u8;
                    cv.blend(x + g.left + gx as i32, baseline + g.top + gy as i32, color, eff);
                }
            }
            x += adv;
        }
    }
}

pub struct Fonts {
    pub mono13: Font,
    pub mono10: Font,
    pub cjk13: Option<Font>,
    pub cjk10: Option<Font>,
}

impl Fonts {
    pub fn load() -> Option<Fonts> {
        let mono_path = if std::path::Path::new(MONO_BOLD).exists() { MONO_BOLD } else { MONO };
        let mono = Font::open(mono_path, 0)?;
        let cjk = Font::open(CJK_FONT, CJK_INDEX);
        let fonts = Fonts {
            mono13: Font::at_size(mono.clone(), 13.0),
            mono10: Font::at_size(mono, 10.0),
            cjk13: cjk.as_ref().map(|f| Font::at_size(f.clone(), 13.0)),
            cjk10: cjk.map(|f| Font::at_size(f, 10.0)),
        };
        Some(fonts)
    }

    pub fn pick_small(&self, c: char) -> &Font {
        let cjk = self.cjk10.as_ref().or(self.cjk13.as_ref());
        match cjk {
            Some(f) if (c as u32) >= 0x2E80 && f.has_glyph(c) => f,
            Some(f) if !self.mono10.has_glyph(c) && f.has_glyph(c) => f,
            _ => &self.mono10,
        }
    }

    pub fn pick(&self, c: char) -> &Font {
        let cjk = self.cjk13.as_ref();
        match cjk {
            Some(f) if (c as u32) >= 0x2E80 && f.has_glyph(c) => f,
            Some(f) if !self.mono13.has_glyph(c) && f.has_glyph(c) => f,
            _ => &self.mono13,
        }
    }

    pub fn advance(&self, c: char) -> i32 {
        self.pick(c).advance(c).max(0)
    }

    pub fn advance_small(&self, c: char) -> i32 {
        self.pick_small(c).advance(c).max(0)
    }

    /// 按基线中英混排
    pub fn draw_base(&self, cv: &mut Canvas, x: i32, baseline: i32, text: &str, color: Rgb565) -> i32 {
        let mut cx = x;
        for c in text.chars() {
            let f = self.pick(c);
            f.draw_base(cv, cx, baseline, &c.to_string(), color, 255);
            cx += f.advance(c).max(0);
        }
        cx
    }

    /// 中英混排的灰度 mask
    pub fn mask_base(&self, buf: &mut [u8], w: i32, h: i32, x: i32, baseline: i32, text: &str) {
        let mut cx = x;
        for c in text.chars() {
            let f = self.pick(c);
            f.mask(buf, w, h, cx, baseline, &c.to_string());
            cx += f.advance(c).max(0);
        }
    }

    pub fn width_mixed(&self, text: &str) -> u32 {
        text.chars().map(|c| self.advance(c).max(0) as u32).sum()
    }

    /// 小号（10px）中英混排宽度
    pub fn width_small(&self, text: &str) -> u32 {
        text.chars().map(|c| self.advance_small(c).max(0) as u32).sum()
    }

    /// 中英混排：CJK 用 cjk，其余用 mono
    pub fn draw_mixed(&self, cv: &mut Canvas, x: i32, y: i32, text: &str, color: Rgb565) -> i32 {
        let mut cx = x;
        for c in text.chars() {
            let f = self.pick(c);
            f.draw(cv, cx, y, &c.to_string(), color, 255);
            cx += f.advance(c).max(0);
        }
        cx
    }
}

pub const FG: Rgb565 = rgb565(234, 240, 255);
pub const DIM: Rgb565 = rgb565(126, 145, 178);
