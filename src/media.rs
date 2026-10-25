//! Decoding GIF / PNG files into frames, and turning frames into GPU textures.
//!
//! Decoding runs on a worker thread (`gio::spawn_blocking`); only the cheap
//! texture creation happens on the main thread. Textures are created once and
//! reused for every repaint, so animating a widget costs one texture swap per
//! GIF frame and no pixel work at all.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Upper bound for decoded RGBA data of one image (all frames together).
/// Bigger animations are scaled down at load time.
pub const BUDGET_BYTES: usize = 192 * 1024 * 1024;

pub struct RawFrame {
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<RawFrame>,
}

/// GIF files commonly store delays of 0 or 1 centisecond meaning "as fast as
/// possible". Browsers (and we) treat those as 100 ms.
fn gif_delay_ms(centis: u16) -> u32 {
    if centis <= 1 {
        100
    } else {
        centis as u32 * 10
    }
}

/// Cheap check by file signature (not by extension).
pub fn is_supported(path: &Path) -> bool {
    let mut magic = [0u8; 8];
    File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok()
        && (magic.starts_with(b"GIF8") || magic.starts_with(&[0x89, b'P', b'N', b'G']))
}

pub fn decode(path: &Path, budget: usize) -> Result<Decoded, String> {
    let mut magic = [0u8; 8];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|e| format!("cannot read file: {e}"))?;

    let mut decoded = if magic.starts_with(b"GIF8") {
        decode_gif(path)?
    } else if magic.starts_with(&[0x89, b'P', b'N', b'G']) {
        decode_png(path)?
    } else {
        return Err("unsupported file type (expected GIF or PNG)".into());
    };
    fit_budget(&mut decoded, budget);
    Ok(decoded)
}

fn decode_png(path: &Path) -> Result<Decoded, String> {
    let img = image::open(path).map_err(|e| format!("PNG decode failed: {e}"))?.into_rgba8();
    let (width, height) = img.dimensions();
    Ok(Decoded { width, height, frames: vec![RawFrame { rgba: img.into_raw(), delay_ms: 0 }] })
}

/// Full GIF decoding with correct disposal methods and transparency.
/// Every returned frame is the complete canvas, so a frame can be shown on its own.
fn decode_gif(path: &Path) -> Result<Decoded, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(BufReader::new(file)).map_err(|e| format!("GIF decode failed: {e}"))?;

    let (w, h) = (decoder.width() as usize, decoder.height() as usize);
    if w == 0 || h == 0 {
        return Err("GIF has no size".into());
    }

    let mut canvas = vec![0u8; w * h * 4];
    let mut saved: Option<Vec<u8>> = None; // canvas before the previous frame (for DisposalMethod::Previous)
    let mut prev_dispose = gif::DisposalMethod::Any;
    let mut prev_rect = (0usize, 0usize, 0usize, 0usize);
    let mut frames = Vec::new();

    while let Some(frame) = decoder.read_next_frame().map_err(|e| format!("GIF decode failed: {e}"))? {
        // 1. Undo the previous frame as its disposal method demands.
        match prev_dispose {
            gif::DisposalMethod::Background => {
                let (l, t, fw, fh) = prev_rect;
                for y in t..(t + fh).min(h) {
                    let start = (y * w + l) * 4;
                    let end = (y * w + (l + fw).min(w)) * 4;
                    canvas[start..end].fill(0);
                }
            }
            gif::DisposalMethod::Previous => {
                if let Some(prev) = saved.take() {
                    canvas = prev;
                }
            }
            _ => {}
        }
        // 2. Remember the canvas if *this* frame must be undone later.
        saved = (frame.dispose == gif::DisposalMethod::Previous).then(|| canvas.clone());

        // 3. Draw. GIF transparency is binary: a transparent pixel leaves the canvas untouched.
        let (l, t, fw, fh) = (frame.left as usize, frame.top as usize, frame.width as usize, frame.height as usize);
        for row in 0..fh {
            let y = t + row;
            if y >= h {
                break;
            }
            for col in 0..fw {
                let x = l + col;
                if x >= w {
                    break;
                }
                let s = (row * fw + col) * 4;
                if frame.buffer[s + 3] == 0 {
                    continue;
                }
                let d = (y * w + x) * 4;
                canvas[d..d + 4].copy_from_slice(&frame.buffer[s..s + 4]);
            }
        }

        frames.push(RawFrame { rgba: canvas.clone(), delay_ms: gif_delay_ms(frame.delay) });
        prev_dispose = frame.dispose;
        prev_rect = (l, t, fw, fh);
    }

    if frames.is_empty() {
        return Err("GIF contains no frames".into());
    }
    Ok(Decoded { width: w as u32, height: h as u32, frames })
}

/// Scale the whole animation down if it would need more than `budget` bytes.
fn fit_budget(d: &mut Decoded, budget: usize) {
    let total = d.width as usize * d.height as usize * 4 * d.frames.len();
    if total <= budget {
        return;
    }
    let factor = (budget as f64 / total as f64).sqrt();
    let nw = ((d.width as f64 * factor) as u32).max(1);
    let nh = ((d.height as f64 * factor) as u32).max(1);
    for f in &mut d.frames {
        if let Some(buf) = image::RgbaImage::from_raw(d.width, d.height, std::mem::take(&mut f.rgba)) {
            f.rgba = image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Triangle).into_raw();
        }
    }
    d.width = nw;
    d.height = nh;
}

/// Decoded animation, uploaded as textures.
pub struct Anim {
    pub width: i32,
    pub height: i32,
    /// (texture, delay in ms). Static images have exactly one entry.
    pub frames: Vec<(gdk::Texture, u32)>,
}

impl Anim {
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    pub fn from_decoded(d: Decoded) -> Anim {
        let stride = d.width as usize * 4;
        let frames = d
            .frames
            .into_iter()
            .map(|f| {
                let bytes = glib::Bytes::from_owned(f.rgba);
                // Straight (non-premultiplied) alpha; GTK converts once at upload.
                let tex: gdk::Texture =
                    gdk::MemoryTexture::new(d.width as i32, d.height as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, stride).upcast();
                (tex, f.delay_ms)
            })
            .collect();
        Anim { width: d.width as i32, height: d.height as i32, frames }
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, Weak<Anim>>> = RefCell::new(HashMap::new());
}

/// Load (or reuse) the animation for `path`. Two widgets showing the same file share one set of textures.
pub async fn load(path: PathBuf) -> Result<Rc<Anim>, String> {
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&path).and_then(Weak::upgrade)) {
        return Ok(hit);
    }
    let p = path.clone();
    let decoded = gio::spawn_blocking(move || decode(&p, BUDGET_BYTES))
        .await
        .map_err(|_| "decoder thread crashed".to_string())??;
    let anim = Rc::new(Anim::from_decoded(decoded));
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|_, w| w.strong_count() > 0);
        c.insert(path, Rc::downgrade(&anim));
    });
    Ok(anim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gifdeck-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// 4x2 canvas. Frame 1 paints the left half red (dispose: Background),
    /// frame 2 paints a transparent patch + one blue pixel on the right (dispose: Keep),
    /// frame 3 paints green over the left half (dispose: Previous), frame 4 does nothing visible.
    fn write_test_gif(path: &Path) {
        let mut file = File::create(path).unwrap();
        let mut enc = gif::Encoder::new(&mut file, 4, 2, &[255, 0, 0, 0, 0, 255, 0, 255, 0]).unwrap();
        // palette: 0 red, 1 blue, 2 green
        let mk = |left, width, color: u8, dispose, delay, transparent: Option<u8>| {
            let mut f = gif::Frame::default();
            f.left = left;
            f.top = 0;
            f.width = width;
            f.height = 2;
            f.buffer = std::borrow::Cow::Owned(vec![color; width as usize * 2]);
            f.dispose = dispose;
            f.delay = delay;
            f.transparent = transparent;
            f
        };
        enc.write_frame(&mk(0, 2, 0, gif::DisposalMethod::Background, 5, None)).unwrap();
        enc.write_frame(&mk(2, 2, 1, gif::DisposalMethod::Keep, 0, None)).unwrap();
        enc.write_frame(&mk(0, 2, 2, gif::DisposalMethod::Previous, 20, None)).unwrap();
        // fully transparent frame covering everything: must leave the canvas as-is
        enc.write_frame(&mk(0, 4, 0, gif::DisposalMethod::Keep, 7, Some(0))).unwrap();
        drop(enc);
        file.flush().unwrap();
    }

    fn px(d: &Decoded, frame: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * d.width as usize + x) * 4;
        d.frames[frame].rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn gif_disposal_transparency_and_timing() {
        let path = tmp("t.gif");
        write_test_gif(&path);
        let d = decode(&path, BUDGET_BYTES).unwrap();
        assert_eq!((d.width, d.height, d.frames.len()), (4, 2, 4));

        // frame 0: red left half, transparent (alpha 0) right half
        assert_eq!(px(&d, 0, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&d, 0, 3, 0)[3], 0);
        // frame 1: previous frame disposed to background -> left half cleared, blue on the right
        assert_eq!(px(&d, 1, 0, 0)[3], 0);
        assert_eq!(px(&d, 1, 3, 1), [0, 0, 255, 255]);
        // frame 2: green on the left over the kept blue
        assert_eq!(px(&d, 2, 1, 1), [0, 255, 0, 255]);
        assert_eq!(px(&d, 2, 2, 0), [0, 0, 255, 255]);
        // frame 3: frame 2 had "restore previous" -> left half is back to transparent;
        // the fully transparent frame 3 adds nothing
        assert_eq!(px(&d, 3, 0, 0)[3], 0);
        assert_eq!(px(&d, 3, 3, 0), [0, 0, 255, 255]);

        // delays: 5cs -> 50ms, 0cs -> browser-compatible 100ms, 20cs -> 200ms
        assert_eq!(d.frames.iter().map(|f| f.delay_ms).collect::<Vec<_>>(), vec![50, 100, 200, 70]);
    }

    #[test]
    fn png_keeps_alpha() {
        let path = tmp("t.png");
        let mut img = image::RgbaImage::new(3, 2);
        img.put_pixel(1, 1, image::Rgba([10, 20, 30, 128]));
        img.save(&path).unwrap();
        let d = decode(&path, BUDGET_BYTES).unwrap();
        assert_eq!((d.width, d.height, d.frames.len()), (3, 2, 1));
        assert_eq!(px(&d, 0, 1, 1), [10, 20, 30, 128]);
        assert_eq!(px(&d, 0, 0, 0)[3], 0);
    }

    #[test]
    fn rejects_other_files_and_respects_budget() {
        let path = tmp("t.txt");
        std::fs::write(&path, b"hello world, not an image").unwrap();
        assert!(decode(&path, BUDGET_BYTES).is_err());

        let gif = tmp("big.gif");
        write_test_gif(&gif);
        let d = decode(&gif, 4 * 2 * 4 * 4 / 2).unwrap(); // half the needed memory
        assert!(d.width < 4 && d.width >= 1);
        assert_eq!(d.frames[0].rgba.len(), (d.width * d.height * 4) as usize);
    }
}
