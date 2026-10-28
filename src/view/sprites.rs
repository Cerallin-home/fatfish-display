//! 立绘图集：底图 45 相位 + 眼睛/嘴的预烤补丁。
//!
//! 早先是把「相位 × 眨眼 × 口型」全烤成一张大图（10 行），内存 ~100MB。
//! 现在只烤底图（闭嘴睁眼）和小补丁：眨眼只换眼睛那一小块、说话只换嘴那一小块，
//! 运行时先贴底图再盖补丁，像素结果一样，内存降到 ~11MB。
use serde_json::Value;

use super::canvas::{Canvas, Rgb565};

/// 一块预烤好的小补丁序列（列 = 相位，行 = 档位）
pub struct Patch {
    px: Vec<u8>,
    sheet_w: u32,
    w: u32,
    h: u32,
    /// 补丁在 tile 内的偏移
    ox: u32,
    oy: u32,
    cols: u32,
}

impl Patch {
    fn load(dir: &str, file: &str, key: &str, meta: &Value) -> Option<Patch> {
        let m = &meta[key];
        if m.is_null() {
            return None;
        }
        let img = image::open(format!("{dir}/{file}")).ok()?.to_rgba8();
        let (sw, _sh) = img.dimensions();
        Some(Patch {
            px: img.into_raw(),
            sheet_w: sw,
            w: m["w"].as_u64()? as u32,
            h: m["h"].as_u64()? as u32,
            ox: m["x"].as_u64()? as u32,
            oy: m["y"].as_u64()? as u32,
            cols: m["cols"].as_u64().unwrap_or(1) as u32,
        })
    }

    fn blit(&self, cv: &mut Canvas, col: u32, row: u32, x0: i32, y0: i32) {
        let sx = (col % self.cols) * self.w;
        let sy = row * self.h;
        for ty in 0..self.h {
            for tx in 0..self.w {
                let i = (((sy + ty) * self.sheet_w) + sx + tx) as usize * 4;
                let a = self.px[i + 3];
                if a == 0 {
                    continue;
                }
                let c = Rgb565(
                    (((self.px[i] as u16) & 0xF8) << 8)
                        | (((self.px[i + 1] as u16) & 0xFC) << 3)
                        | ((self.px[i + 2] as u16) >> 3),
                );
                cv.blend(x0 + tx as i32, y0 + ty as i32, c, a);
            }
        }
    }
}

pub struct Sprites {
    px: Vec<u8>, // RGBA 底图
    sheet_w: u32,
    pub tile_w: u32,
    pub tile_h: u32,
    pub cols: u32,
    pub phases: u32,
    pub blink_levels: usize,
    pub mouth_levels: usize,
    blink: Option<Patch>,
    mouth: Option<Patch>,
}

impl Sprites {
    pub fn load(dir: &str) -> Option<Sprites> {
        let text = std::fs::read_to_string(format!("{dir}/sprites.json")).ok()?;
        let meta: Value = serde_json::from_str(&text).ok()?;
        let img = image::open(format!("{dir}/sprites.png")).ok()?.to_rgba8();
        let (sw, _sh) = img.dimensions();
        Some(Sprites {
            px: img.into_raw(),
            sheet_w: sw,
            tile_w: meta["tile_w"].as_u64()? as u32,
            tile_h: meta["tile_h"].as_u64()? as u32,
            cols: meta["cols"].as_u64()? as u32,
            phases: meta["phases"].as_u64()? as u32,
            blink_levels: meta["blinks"].as_array().map(|a| a.len()).unwrap_or(2),
            mouth_levels: meta["mouths"].as_array().map(|a| a.len() + 1).unwrap_or(5),
            blink: Patch::load(dir, "blink.png", "blink_patch", &meta),
            mouth: Patch::load(dir, "mouth.png", "mouth_patch", &meta),
        })
    }

    /// 贴一张立绘（底图 + 眼睛/嘴补丁）；返回贴的位置，方便上层算脏区。
    pub fn blit(&self, cv: &mut Canvas, phase: u32, blink: usize, mouth: usize, x0: i32, y0: i32) -> (i32, i32, u32, u32) {
        let col = phase % self.phases;
        let sx = col * self.tile_w;
        for ty in 0..self.tile_h {
            for tx in 0..self.tile_w {
                let i = ((ty * self.sheet_w) + sx + tx) as usize * 4;
                let a = self.px[i + 3];
                if a == 0 {
                    continue;
                }
                let c = Rgb565(
                    (((self.px[i] as u16) & 0xF8) << 8)
                        | (((self.px[i + 1] as u16) & 0xFC) << 3)
                        | ((self.px[i + 2] as u16) >> 3),
                );
                cv.blend(x0 + tx as i32, y0 + ty as i32, c, a);
            }
        }
        if blink > 0 {
            if let Some(p) = &self.blink {
                p.blit(cv, col, blink.saturating_sub(1) as u32, x0 + p.ox as i32, y0 + p.oy as i32);
            }
        }
        if mouth > 0 {
            if let Some(p) = &self.mouth {
                let row = (mouth - 1).min(self.mouth_levels.saturating_sub(2)) as u32;
                p.blit(cv, col, row, x0 + p.ox as i32, y0 + p.oy as i32);
            }
        }
        (x0, y0, self.tile_w, self.tile_h)
    }
}
