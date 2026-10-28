//! View 层：画布 + 后端（PNG 预览 / /dev/fb1）+ 界面绘制。
pub mod canvas;
pub mod panel;
pub mod sprites;
pub mod text;

use std::path::Path;

use canvas::Canvas;

pub trait Backend {
    fn present(&mut self, cv: &mut Canvas) -> std::io::Result<()>;
    fn name(&self) -> &str;
    fn as_any(&self) -> &dyn std::any::Any {
        &()
    }
}

/// 离线预览：把帧存成 PNG，没屏幕也能验。
pub struct PngBackend {
    dir: std::path::PathBuf,
    frame: u64,
}

impl PngBackend {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref().to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        PngBackend { dir, frame: 0 }
    }
}

impl Backend for PngBackend {
    fn present(&mut self, cv: &mut Canvas) -> std::io::Result<()> {
        cv.dirty_rows();
        let buf = cv.back_buffer();
        let mut img = image::RgbImage::new(cv.w, cv.h);
        for y in 0..cv.h {
            for x in 0..cv.w {
                let (r, g, b) = canvas::unpack(buf[(y * cv.w + x) as usize]);
                img.put_pixel(x, y, image::Rgb([r, g, b]));
            }
        }
        let p = self.dir.join(format!("frame_{:04}.png", self.frame));
        self.frame += 1;
        img.save(p).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    fn name(&self) -> &str {
        "png"
    }
}

/// 空后端：只跑合成，用来量性能
pub struct NullBackend {
    pub frames: u64,
    pub dirty_rows_total: u64,
}

impl Backend for NullBackend {
    fn present(&mut self, cv: &mut Canvas) -> std::io::Result<()> {
        let _ = cv.commit();
        self.frames += 1;
        self.dirty_rows_total += cv.dirty_rows().iter().map(|(a, b)| (b - a) as u64).sum::<u64>();
        Ok(())
    }

    fn name(&self) -> &str {
        "null"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// fbdev 后端：mmap 整个帧缓冲，按脏行区间写 RGB565。
pub struct FbBackend {
    map: memmap2::MmapMut,
    stride_px: u32,
}

impl FbBackend {
    pub fn open(path: &str) -> std::io::Result<Self> {
        let f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        // framebuffer 是字符设备，metadata.len() 是 0，mmap 必须显式给长度，
        // 否则只映射到一页，写下去的内容连两行都盖不住。
        let node = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "fb0".into());
        let stride = std::fs::read_to_string(format!("/sys/class/graphics/{node}/stride"))
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(960);
        let len = (stride as usize) * 320;
        let map = unsafe { memmap2::MmapOptions::new().len(len).map_mut(&f)? };
        Ok(FbBackend { map, stride_px: stride / 2 })
    }
}

impl Backend for FbBackend {
    fn present(&mut self, cv: &mut Canvas) -> std::io::Result<()> {
        // commit() 才会算出脏行并更新影子帧；只读 dirty_rows() 永远是空的。
        cv.commit();
        if cv.dirty_rows().is_empty() {
            return Ok(());
        }
        let rows: Vec<(u32, u32)> = cv.dirty_rows().to_vec();
        let buf = cv.back_buffer();
        let dst = &mut self.map[..];
        for (a, b) in rows {
            let s = (a * cv.w) as usize;
            let e = (b * cv.w) as usize;
            let bytes = unsafe {
                std::slice::from_raw_parts(buf[s..e].as_ptr() as *const u8, (e - s) * 2)
            };
            let off = s * 2;
            let len = bytes.len().min(dst.len().saturating_sub(off));
            if len > 0 {
                dst[off..off + len].copy_from_slice(&bytes[..len]);
            }
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "fb"
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas::new(crate::viewmodel::W, crate::viewmodel::H)
    }
}
