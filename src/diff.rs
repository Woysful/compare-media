//! Общая логика: попиксельное сравнение, метрики качества (PSNR, SSIM, VMAF), помощники для ffmpeg.

use crate::i18n::{tr, trf};
use image::{imageops::FilterType, Rgb, RgbImage};
use std::process::Command;

// ---------- попиксельная разница ----------

pub struct Stats {
    pub mean_abs: f64,
    pub psnr: f64,
    pub diff_pct: f64,
}

pub fn is_image(p: &str) -> bool {
    let ext = std::path::Path::new(p)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    matches!(
        ext.as_deref(),
        Some("png" | "jpg" | "jpeg" | "bmp" | "webp" | "gif" | "tif" | "tiff")
    )
}

/// Сравнивает два изображения одного размера.
/// `gain` — усиление картинки разницы, `threshold` — порог (0..255) для "отличающегося" пикселя.
pub fn compare(a: &RgbImage, b: &RgbImage, gain: u32, threshold: u8) -> (Stats, RgbImage) {
    let (w, h) = a.dimensions();
    let mut diff = RgbImage::new(w, h);
    let (mut sum_abs, mut sum_sq, mut differing) = (0u64, 0u64, 0u64);

    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        let mut max_d = 0u8;
        let mut out = [0u8; 3];
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            sum_abs += d as u64;
            sum_sq += (d as u64) * (d as u64);
            max_d = max_d.max(d);
            out[c] = (d as u32 * gain).min(255) as u8;
        }
        if max_d > threshold {
            differing += 1;
        }
        diff.put_pixel(x, y, Rgb(out));
    }

    let samples = (w as u64 * h as u64 * 3) as f64;
    let mse = sum_sq as f64 / samples;
    let psnr = if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    };

    (
        Stats {
            mean_abs: sum_abs as f64 / samples,
            psnr,
            diff_pct: differing as f64 * 100.0 / (w as f64 * h as f64),
        },
        diff,
    )
}

pub fn side_by_side(a: &RgbImage, b: &RgbImage, diff: &RgbImage) -> RgbImage {
    let (w, h) = a.dimensions();
    let mut out = RgbImage::new(w * 3, h);
    for (i, img) in [a, b, diff].iter().enumerate() {
        image::imageops::replace(&mut out, *img, (i as u32 * w) as i64, 0);
    }
    out
}

/// Приводит изображение к заданному размеру (если он отличается).
pub fn resize_to(img: RgbImage, w: u32, h: u32) -> RgbImage {
    if img.dimensions() == (w, h) {
        img
    } else {
        image::imageops::resize(&img, w, h, FilterType::Lanczos3)
    }
}

/// Приводит `b` к размеру `a`.
pub fn match_size(a: &RgbImage, b: RgbImage) -> RgbImage {
    resize_to(b, a.width(), a.height())
}

/// Уменьшает изображение, если его большая сторона больше `limit`
/// (защита от слишком больших текстур в GUI). Все кадры одного набора
/// имеют один размер, поэтому масштабируются одинаково.
pub fn limit_one(img: RgbImage, limit: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let m = w.max(h);
    if m <= limit {
        return img;
    }
    let k = limit as f32 / m as f32;
    let nw = ((w as f32 * k) as u32).max(1);
    let nh = ((h as f32 * k) as u32).max(1);
    image::imageops::resize(&img, nw, nh, FilterType::Triangle)
}

// ---------- метрики качества ----------

#[derive(Clone, Copy, Debug)]
pub struct Quality {
    pub psnr: f64,
    pub ssim: f64,
}

/// PSNR по всем каналам RGB (дБ). Для идентичных изображений — бесконечность.
pub fn psnr(a: &RgbImage, b: &RgbImage) -> f64 {
    let mut sq = 0u64;
    for (x, y) in a.as_raw().iter().zip(b.as_raw().iter()) {
        let d = x.abs_diff(*y) as u64;
        sq += d * d;
    }
    if sq == 0 {
        return f64::INFINITY;
    }
    let mse = sq as f64 / a.as_raw().len() as f64;
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

fn luma(img: &RgbImage) -> Vec<f32> {
    img.as_raw()
        .chunks_exact(3)
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}

/// SSIM по яркости: окна 8x8 с шагом 4 (как в ffmpeg/x264). Результат 0..1.
/// Для изображений меньше 8 пикселей по стороне возвращает NaN.
pub fn ssim(a: &RgbImage, b: &RgbImage) -> f64 {
    let (w, h) = (a.width() as usize, a.height() as usize);
    if w < 8 || h < 8 {
        return f64::NAN;
    }
    let la = luma(a);
    let lb = luma(b);

    const C1: f64 = 6.5025; // (0.01 * 255)^2
    const C2: f64 = 58.5225; // (0.03 * 255)^2
    const N: f64 = 64.0;

    let (mut total, mut count) = (0.0f64, 0u64);
    let mut y = 0;
    while y + 8 <= h {
        let mut x = 0;
        while x + 8 <= w {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f32, 0f32, 0f32, 0f32, 0f32);
            for j in 0..8 {
                let row = (y + j) * w + x;
                for i in 0..8 {
                    let p = la[row + i];
                    let q = lb[row + i];
                    sa += p;
                    sb += q;
                    saa += p * p;
                    sbb += q * q;
                    sab += p * q;
                }
            }
            let ma = sa as f64 / N;
            let mb = sb as f64 / N;
            let va = saa as f64 / N - ma * ma;
            let vb = sbb as f64 / N - mb * mb;
            let cov = sab as f64 / N - ma * mb;
            total += ((2.0 * ma * mb + C1) * (2.0 * cov + C2))
                / ((ma * ma + mb * mb + C1) * (va + vb + C2));
            count += 1;
            x += 4;
        }
        y += 4;
    }
    total / count as f64
}

pub fn quality(a: &RgbImage, b: &RgbImage) -> Quality {
    Quality { psnr: psnr(a, b), ssim: ssim(a, b) }
}

// ---------- ffmpeg / ffprobe ----------

pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

fn parse_fraction(s: &str) -> f64 {
    match s.split_once('/') {
        Some((n, d)) => {
            let n: f64 = n.trim().parse().unwrap_or(0.0);
            let d: f64 = d.trim().parse().unwrap_or(1.0);
            if d == 0.0 {
                0.0
            } else {
                n / d
            }
        }
        None => s.trim().parse().unwrap_or(0.0),
    }
}

/// Размер, частота кадров и длительность видео (через ffprobe).
pub fn video_info(path: &str) -> Result<VideoInfo, String> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height,r_frame_rate:format=duration",
            "-of", "default=noprint_wrappers=1",
            path,
        ])
        .output()
        .map_err(|e| trf("failed to run ffprobe (is it installed?): {0}", &[&e]))?;

    let text = String::from_utf8_lossy(&out.stdout);
    let (mut w, mut h, mut fps, mut dur) = (0u32, 0u32, 0f64, 0f64);
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            match k.trim() {
                "width" => w = v.parse().unwrap_or(0),
                "height" => h = v.parse().unwrap_or(0),
                "r_frame_rate" => fps = parse_fraction(v),
                "duration" => dur = v.parse().unwrap_or(0.0),
                _ => {}
            }
        }
    }
    if w == 0 || h == 0 || dur <= 0.0 {
        return Err(trf("could not read video: {0}", &[&path]));
    }
    if fps <= 0.0 {
        fps = 30.0;
    }
    Ok(VideoInfo { width: w, height: h, fps, duration: dur })
}

/// Достаёт один кадр видео на момент `t` секунд (через ffmpeg).
pub fn video_frame(path: &str, t: f64) -> Result<RgbImage, String> {
    let ts = format!("{t:.3}");
    let out = Command::new("ffmpeg")
        .args([
            "-v", "error",
            "-ss", ts.as_str(),
            "-i", path,
            "-frames:v", "1",
            "-f", "image2pipe",
            "-vcodec", "png",
            "-",
        ])
        .output()
        .map_err(|e| trf("failed to run ffmpeg (is it installed?): {0}", &[&e]))?;
    if out.stdout.is_empty() {
        return Err(trf("ffmpeg returned no frame for {0} at {1} s", &[&path, &ts]));
    }
    image::load_from_memory(&out.stdout)
        .map(|i| i.to_rgb8())
        .map_err(|e| e.to_string())
}

/// VMAF (0..100) через фильтр libvmaf в ffmpeg. `w`,`h`,`fps` — параметры эталона:
/// проверяемое видео приводится к ним перед сравнением.
pub fn vmaf(distorted: &str, reference: &str, w: u32, h: u32, fps: f64) -> Result<f64, String> {
    let graph = format!(
        "[0:v]fps={fps:.3},scale={w}:{h}:flags=bicubic,setpts=PTS-STARTPTS[d];\
         [1:v]fps={fps:.3},setpts=PTS-STARTPTS[r];\
         [d][r]libvmaf"
    );
    let out = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-i", distorted,
            "-i", reference,
            "-lavfi", graph.as_str(),
            "-f", "null",
            "-",
        ])
        .output()
        .map_err(|e| trf("failed to run ffmpeg: {0}", &[&e]))?;

    let err = String::from_utf8_lossy(&out.stderr);
    const MARK: &str = "VMAF score:";
    for line in err.lines() {
        if let Some(i) = line.find(MARK) {
            return line[i + MARK.len()..]
                .trim()
                .parse::<f64>()
                .map_err(|_| tr("could not parse the VMAF value").to_string());
        }
    }
    if err.contains("No such filter") {
        Err(tr("this ffmpeg build has no libvmaf").to_string())
    } else {
        let tail: Vec<&str> = err.lines().rev().take(2).collect();
        Err(trf("VMAF failed: {0}", &[&tail.join(" | ")]))
    }
}
