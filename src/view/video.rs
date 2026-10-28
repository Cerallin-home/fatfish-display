//! 离线转好的 RGBA 帧序列（assets/states/dsh_down.clip）。
//!
//! DSH 挂了的时候，整屏放这一段：她慢慢躺倒、最后不动。
//! 帧不需要解码，直接 mmap 贴着走，内存里不多占一份。
use std::fs::File;

use memmap2::Mmap;

use super::canvas::{Canvas, Rgb565};

const MAGIC: &[u8; 8] = b"FISHCLP2";
const HEADER: usize = 8 + 4 * 4 + 4 * 2;

/// 一帧的尺寸和落点都在文件头里：落点是「成片左上角贴在画布的哪」，
/// 由转码脚本按立绘在面板上的位置反算出来，保证人物原地倒下。
pub struct Clip {
    map: Mmap,
    pub w: u32,
    pub h: u32,
    pub fps: f32,
    pub count: u32,
    pub x: i32,
    pub y: i32,
}

impl Clip {
    pub fn load(path: &str) -> Option<Clip> {
        let file = File::open(path).ok()?;
        // SAFETY: 文件由转码脚本一次写完，运行期只读
        let map = unsafe { Mmap::map(&file) }.ok()?;
        if map.len() < HEADER || &map[0..8] != MAGIC {
            return None;
        }
        let u32_at = |o: usize| u32::from_le_bytes([map[o], map[o + 1], map[o + 2], map[o + 3]]);
        let i32_at = |o: usize| i32::from_le_bytes([map[o], map[o + 1], map[o + 2], map[o + 3]]);
        let w = u32_at(8);
        let h = u32_at(12);
        let fps_num = u32_at(16);
        let count = u32_at(20);
        let x = i32_at(24);
        let y = i32_at(28);
        let need = HEADER + (w as usize) * (h as usize) * 4 * count as usize;
        if count == 0 || map.len() < need {
            return None;
        }
        Some(Clip { map, w, h, fps: fps_num as f32, count, x, y })
    }

    pub fn duration(&self) -> f32 {
        self.count as f32 / self.fps.max(1.0)
    }

    /// 播到哪一帧：到头了就定在最后一帧不动，不循环——她倒下了就该躺着
    pub fn frame_at(&self, t: f32) -> u32 {
        ((t * self.fps) as u32).min(self.count - 1)
    }

    pub fn blit(&self, cv: &mut Canvas, idx: u32) {
        let idx = idx.min(self.count - 1) as usize;
        let stride = (self.w as usize) * 4;
        let base = HEADER + idx * stride * (self.h as usize);
        for py in 0..self.h as i32 {
            let y = self.y + py;
            if y < 0 || y >= cv.h as i32 {
                continue;
            }
            let row = base + (py as usize) * stride;
            for px in 0..self.w as i32 {
                let x = self.x + px;
                if x < 0 || x >= cv.w as i32 {
                    continue;
                }
                let o = row + (px as usize) * 4;
                let a = self.map[o + 3];
                if a == 0 {
                    continue;
                }
                let c = Rgb565(
                    (((self.map[o] as u16) & 0xF8) << 8)
                        | (((self.map[o + 1] as u16) & 0xFC) << 3)
                        | ((self.map[o + 2] as u16) >> 3),
                );
                cv.blend(x, y, c, a);
            }
        }
    }
}
