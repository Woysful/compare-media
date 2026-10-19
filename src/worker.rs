//! Фоновые потоки: загрузка кадров (чтобы интерфейс не зависал при перемотке)
//! и анализ всего видео покадрово.

use crate::diff::{self, Quality};
use crate::i18n::{tr, trf};
use eframe::egui;
use image::RgbImage;
use std::{
    io::Read,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
};

// ---------- медиафайл ----------

#[derive(Clone)]
pub struct Media {
    pub path: PathBuf,
    pub is_video: bool,
    pub duration: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
}

impl Media {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let p = path.to_string_lossy().to_string();
        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if diff::is_image(&p) {
            let (width, height) =
                image::image_dimensions(&path).map_err(|e| format!("{p}: {e}"))?;
            Ok(Self { path, is_video: false, duration: 0.0, fps: 0.0, width, height, size_bytes })
        } else {
            let i = diff::video_info(&p)?;
            Ok(Self {
                path,
                is_video: true,
                duration: i.duration,
                fps: i.fps,
                width: i.width,
                height: i.height,
                size_bytes,
            })
        }
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Средний битрейт файла, кбит/с (только для видео).
    pub fn bitrate_kbps(&self) -> Option<f64> {
        if self.is_video && self.duration > 0.0 && self.size_bytes > 0 {
            Some(self.size_bytes as f64 * 8.0 / self.duration / 1000.0)
        } else {
            None
        }
    }

    pub fn frame(&self, t: f64) -> Result<RgbImage, String> {
        if self.is_video {
            // Полкадра в запас, чтобы при покадровой навигации не попадать на предыдущий кадр.
            let half = if self.fps > 0.0 { 0.5 / self.fps } else { 0.0 };
            let t = (t + half).min((self.duration - 0.05).max(0.0));
            diff::video_frame(&self.path.to_string_lossy(), t)
        } else {
            image::open(&self.path)
                .map(|i| i.to_rgb8())
                .map_err(|e| format!("{}: {e}", self.path.display()))
        }
    }
}

// ---------- загрузка кадров ----------

pub struct LoadRequest {
    pub id: u64,
    pub time: f64,
    pub a: Media,
    pub b: Media,
    pub r: Option<Media>,
    pub limit: u32,
}

pub struct FrameSet {
    pub a: RgbImage,
    pub b: RgbImage,
    pub r: Option<RgbImage>,
    /// Качество A относительно оригинала (если оригинал задан).
    pub qa: Option<Quality>,
    /// Качество B относительно оригинала, а без оригинала — относительно A.
    pub qb: Quality,
}

pub struct LoadResult {
    pub id: u64,
    pub result: Result<FrameSet, String>,
}

pub struct Loader {
    tx: Sender<LoadRequest>,
    pub rx: Receiver<LoadResult>,
}

impl Loader {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, req_rx) = mpsc::channel::<LoadRequest>();
        let (res_tx, rx) = mpsc::channel::<LoadResult>();

        std::thread::spawn(move || {
            while let Ok(mut req) = req_rx.recv() {
                // Если пользователь быстро крутит ползунок — берём только последний запрос.
                while let Ok(next) = req_rx.try_recv() {
                    req = next;
                }
                let id = req.id;
                let result = process(req);
                if res_tx.send(LoadResult { id, result }).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });

        Self { tx, rx }
    }

    pub fn request(&self, req: LoadRequest) {
        let _ = self.tx.send(req);
    }
}

fn process(req: LoadRequest) -> Result<FrameSet, String> {
    let fa = req.a.frame(req.time)?;
    let fb = req.b.frame(req.time)?;
    let fr = match &req.r {
        Some(r) => Some(r.frame(req.time)?),
        None => None,
    };

    // Базовый размер — размер оригинала, а без него — размер A.
    let (w, h) = match &fr {
        Some(r) => r.dimensions(),
        None => fa.dimensions(),
    };
    let fa = diff::limit_one(diff::resize_to(fa, w, h), req.limit);
    let fb = diff::limit_one(diff::resize_to(fb, w, h), req.limit);
    let fr = fr.map(|r| diff::limit_one(r, req.limit));

    let (qa, qb) = match &fr {
        Some(r) => (Some(diff::quality(&fa, r)), diff::quality(&fb, r)),
        None => (None, diff::quality(&fb, &fa)),
    };

    Ok(FrameSet { a: fa, b: fb, r: fr, qa, qb })
}

// ---------- анализ всего видео ----------

pub struct AnalysisJob {
    pub a: Media,
    pub b: Media,
    pub r: Option<Media>,
    pub vmaf: bool,
}

#[derive(Default, Clone)]
pub struct Series {
    pub psnr: Vec<f32>,
    pub ssim: Vec<f32>,
}

fn mean(v: &[f32]) -> f64 {
    let f: Vec<f64> = v.iter().filter(|x| x.is_finite()).map(|x| *x as f64).collect();
    if f.is_empty() {
        f64::NAN
    } else {
        f.iter().sum::<f64>() / f.len() as f64
    }
}

impl Series {
    pub fn mean_psnr(&self) -> f64 {
        mean(&self.psnr)
    }
    pub fn mean_ssim(&self) -> f64 {
        mean(&self.ssim)
    }
    pub fn min_ssim(&self) -> f64 {
        self.ssim
            .iter()
            .filter(|x| x.is_finite())
            .fold(f64::INFINITY, |m, x| m.min(*x as f64))
    }
    fn push(&mut self, q: Quality) {
        // PSNR идентичных кадров бесконечен — ограничиваем, чтобы график и среднее были конечными.
        self.psnr.push(q.psnr.min(100.0) as f32);
        self.ssim.push(q.ssim as f32);
    }
}

pub struct Analysis {
    pub fps: f64,
    /// Ряды для A (только если задан оригинал).
    pub a: Option<Series>,
    /// Ряды для B (относительно оригинала или, без него, относительно A).
    pub b: Series,
    pub vmaf_a: Option<Result<f64, String>>,
    pub vmaf_b: Option<Result<f64, String>>,
}

impl Analysis {
    /// Меняет местами результаты для A и B (когда пользователь поменял файлы местами).
    pub fn swap_ab(&mut self, has_ref: bool) {
        if has_ref {
            if let Some(sa) = self.a.take() {
                let old_b = std::mem::replace(&mut self.b, sa);
                self.a = Some(old_b);
            }
            std::mem::swap(&mut self.vmaf_a, &mut self.vmaf_b);
        } else {
            // Без оригинала ряд описывает пару "B относительно A". PSNR и SSIM симметричны,
            // поэтому после обмена он остаётся верным. VMAF несимметричен (эталон/проверяемое),
            // и после обмена его значение уже не относится к новой паре.
            self.vmaf_b = None;
        }
    }
}

pub enum AnalysisMsg {
    Progress(usize),
    Status(String),
    Done(Analysis),
    Failed(String),
}

pub struct AnalysisHandle {
    pub rx: Receiver<AnalysisMsg>,
    pub cancel: Arc<AtomicBool>,
    pub total: usize,
}

pub fn start_analysis(job: AnalysisJob, ctx: egui::Context) -> Result<AnalysisHandle, String> {
    let (fps, duration, is_video) = {
        let base = job.r.as_ref().unwrap_or(&job.a);
        (base.fps, base.duration, base.is_video)
    };
    if !is_video {
        return Err(tr("Analysis is only available for video").to_string());
    }
    let total = (duration * fps) as usize;

    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_thread = cancel.clone();

    std::thread::spawn(move || {
        match run_analysis(&job, &tx, &ctx, &cancel_thread) {
            Ok(a) => {
                let _ = tx.send(AnalysisMsg::Done(a));
            }
            Err(e) => {
                let _ = tx.send(AnalysisMsg::Failed(e));
            }
        }
        ctx.request_repaint();
    });

    Ok(AnalysisHandle { rx, cancel, total })
}

fn spawn_raw(path: &str, fps: f64, w: u32, h: u32) -> Result<Child, String> {
    let vf = format!("fps={fps:.3},scale={w}:{h}:flags=bicubic");
    Command::new("ffmpeg")
        .args([
            "-v", "error",
            "-an", "-sn",
            "-i", path,
            "-vf", vf.as_str(),
            "-f", "rawvideo",
            "-pix_fmt", "rgb24",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| trf("failed to run ffmpeg: {0}", &[&e]))
}

fn kill_all(children: &mut [Child]) {
    for c in children.iter_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn run_analysis(
    job: &AnalysisJob,
    tx: &Sender<AnalysisMsg>,
    ctx: &egui::Context,
    cancel: &AtomicBool,
) -> Result<Analysis, String> {
    let has_ref = job.r.is_some();
    let base = job.r.as_ref().unwrap_or(&job.a);
    let fps = base.fps;

    // Анализируем в уменьшенном размере (не больше 960 px по ширине) — это намного быстрее.
    let aw = (base.width.min(960) / 2 * 2).max(2);
    let ah = ((((base.height as f64) * aw as f64 / base.width as f64).round() as u32) / 2 * 2).max(2);

    // Порядок потоков: [оригинал,] A, B
    let mut paths: Vec<&PathBuf> = Vec::new();
    if let Some(r) = &job.r {
        paths.push(&r.path);
    }
    paths.push(&job.a.path);
    paths.push(&job.b.path);

    let mut children: Vec<Child> = Vec::new();
    for p in &paths {
        match spawn_raw(&p.to_string_lossy(), fps, aw, ah) {
            Ok(c) => children.push(c),
            Err(e) => {
                kill_all(&mut children);
                return Err(e);
            }
        }
    }

    let mut pipes = Vec::new();
    for c in children.iter_mut() {
        match c.stdout.take() {
            Some(s) => pipes.push(s),
            None => {
                kill_all(&mut children);
                return Err(tr("could not get ffmpeg output").to_string());
            }
        }
    }

    let flen = (aw * ah * 3) as usize;
    let mut bufs: Vec<Vec<u8>> = vec![vec![0u8; flen]; pipes.len()];
    let (mut series_a, mut series_b) = (Series::default(), Series::default());
    let mut n = 0usize;

    'frames: loop {
        if cancel.load(Ordering::Relaxed) {
            kill_all(&mut children);
            return Err(tr("Analysis cancelled").to_string());
        }
        for (pipe, buf) in pipes.iter_mut().zip(bufs.iter_mut()) {
            if pipe.read_exact(buf).is_err() {
                break 'frames;
            }
        }

        let imgs: Vec<RgbImage> = bufs
            .iter()
            .map(|b| RgbImage::from_raw(aw, ah, b.clone()).unwrap())
            .collect();

        if has_ref {
            series_a.push(diff::quality(&imgs[1], &imgs[0]));
            series_b.push(diff::quality(&imgs[2], &imgs[0]));
        } else {
            series_b.push(diff::quality(&imgs[1], &imgs[0]));
        }

        n += 1;
        if n % 5 == 0 {
            let _ = tx.send(AnalysisMsg::Progress(n));
            ctx.request_repaint();
        }
    }

    kill_all(&mut children);
    if n == 0 {
        return Err(tr("could not read any frames (is ffmpeg installed?)").to_string());
    }
    let _ = tx.send(AnalysisMsg::Progress(n));

    // VMAF (необязательно, медленно)
    let (mut vmaf_a, mut vmaf_b) = (None, None);
    if job.vmaf {
        let _ = tx.send(AnalysisMsg::Status(tr("Computing VMAF…").to_string()));
        ctx.request_repaint();
        let reference = base.path.to_string_lossy().to_string();
        if has_ref {
            vmaf_a = Some(diff::vmaf(
                &job.a.path.to_string_lossy(),
                &reference,
                base.width,
                base.height,
                fps,
            ));
        }
        vmaf_b = Some(diff::vmaf(
            &job.b.path.to_string_lossy(),
            &reference,
            base.width,
            base.height,
            fps,
        ));
    }

    Ok(Analysis {
        fps,
        a: if has_ref { Some(series_a) } else { None },
        b: series_b,
        vmaf_a,
        vmaf_b,
    })
}
