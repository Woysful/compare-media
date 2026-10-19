//! Графический интерфейс (egui/eframe).

use crate::config::{self, Backend, Config, Theme};
use crate::diff::{self, Stats};
use crate::i18n::{self, tr, trf, Lang};
use crate::worker::{
    self, Analysis, AnalysisHandle, AnalysisJob, AnalysisMsg, FrameSet, LoadRequest, Loader, Media,
    Series,
};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, ColorImage, CursorIcon, FontId, Key, Layout, Painter,
    Pos2, Rect, RichText, Rounding, Sense, Shape, Stroke, TextureFilter, TextureHandle, TextureId,
    TextureOptions, Vec2,
};
use image::RgbImage;
use std::{path::PathBuf, sync::atomic::Ordering, time::Duration};

/// Максимальная сторона кадра для отображения (защита от огромных текстур).
const MAX_SIDE: u32 = 4096;

const MEDIA_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "bmp", "webp", "gif", "tif", "tiff", "mp4", "mkv", "avi", "mov", "webm",
    "m4v", "flv", "wmv", "ts", "mpg", "mpeg",
];

const COLOR_A: Color32 = Color32::from_rgb(79, 156, 249);
const COLOR_B: Color32 = Color32::from_rgb(249, 166, 79);
const COLOR_R: Color32 = Color32::from_rgb(98, 200, 140);
const COLOR_D: Color32 = Color32::from_rgb(130, 134, 150);
const COLOR_GOOD: Color32 = Color32::from_rgb(90, 200, 120);

pub fn run(files: Vec<String>, cfg: Config) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([920.0, 560.0])
            .with_title("Compare Media")
            .with_app_id("compare-media"),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Compare Media",
        options,
        Box::new(move |cc| {
            apply_style(&cc.egui_ctx, cfg.theme);
            Ok(Box::new(App::new(cc.egui_ctx.clone(), files, cfg)))
        }),
    );
    if let Err(e) = result {
        eprintln!("{}", trf("GUI error: {0}", &[&e]));
        std::process::exit(1);
    }
}

// ---------- типы ----------

#[derive(PartialEq, Clone, Copy)]
enum ViewMode {
    Side,
    Wipe,
    Blink,
    Diff,
}

#[derive(PartialEq, Clone, Copy)]
enum DiffPair {
    AB,
    AR,
    BR,
}

#[derive(PartialEq, Clone, Copy)]
enum GraphMetric {
    Ssim,
    Psnr,
}

#[derive(PartialEq, Clone, Copy)]
enum Slot {
    A,
    B,
    R,
}

impl Slot {
    fn tag(self) -> &'static str {
        match self {
            Slot::A => "A",
            Slot::B => "B",
            Slot::R => tr("Original"),
        }
    }
    fn color(self) -> Color32 {
        match self {
            Slot::A => COLOR_A,
            Slot::B => COLOR_B,
            Slot::R => COLOR_R,
        }
    }
}

struct Loaded {
    set: FrameSet,
    tex_a: TextureHandle,
    tex_b: TextureHandle,
    tex_r: Option<TextureHandle>,
    tex_diff: TextureHandle,
    diff_stats: Stats,
}

struct App {
    cfg: Config,
    cfg_edit: Config,
    show_settings: bool,

    a: Option<Media>,
    b: Option<Media>,
    r: Option<Media>,
    time: f64,

    mode: ViewMode,
    diff_pair: DiffPair,
    show_diff_pane: bool,
    gain: u32,
    threshold: u8,

    wipe: f32,
    wipe_drag: bool,
    zoom: f32,
    center: Vec2,
    view_reset: bool,
    hud_scale: f32,

    loader: Loader,
    req_id: u64,
    pending: bool,
    loaded: Option<Loaded>,
    need_request: bool,
    need_diff: bool,
    error: Option<String>,

    analysis: Option<Analysis>,
    running: Option<AnalysisHandle>,
    /// A и B поменяли местами, пока шёл анализ: результат нужно развернуть по завершении.
    analysis_swapped: bool,
    progress: usize,
    status: String,
    want_vmaf: bool,
    graph_metric: GraphMetric,

    probe: Option<String>,
}

// ---------- метрики для таблицы ----------

#[derive(Clone, Copy)]
enum Kind {
    Psnr,
    Ssim,
    Vmaf,
}

impl Kind {
    fn fmt(self, v: Option<f64>) -> String {
        match v {
            None => "—".to_string(),
            Some(v) => match self {
                Kind::Psnr => {
                    if v.is_infinite() {
                        "∞".to_string()
                    } else {
                        format!("{v:.2}")
                    }
                }
                Kind::Ssim => format!("{v:.4}"),
                Kind::Vmaf => format!("{v:.2}"),
            },
        }
    }
    /// Разница меньше этого значения считается ничьей.
    fn eps(self) -> f64 {
        match self {
            Kind::Psnr => 0.01,
            Kind::Ssim => 0.0001,
            Kind::Vmaf => 0.05,
        }
    }
}

struct Row {
    name: &'static str,
    a: Option<f64>,
    b: Option<f64>,
    kind: Kind,
    /// Строка относится к анализу всего видео (а не к одному кадру).
    video: bool,
    /// Участвует в подсчёте победителя.
    tally: bool,
}

impl Row {
    /// Some(true) — лучше A, Some(false) — лучше B, None — ничья или нет данных.
    /// У всех метрик здесь "больше — лучше".
    fn winner(&self) -> Option<bool> {
        match (self.a, self.b) {
            (Some(a), Some(b)) => {
                if a > b + self.kind.eps() {
                    Some(true)
                } else if b > a + self.kind.eps() {
                    Some(false)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

// ---------- реализация App ----------

impl App {
    fn new(ctx: egui::Context, files: Vec<String>, cfg: Config) -> Self {
        let mut app = Self {
            cfg: cfg.clone(),
            cfg_edit: cfg,
            show_settings: false,
            a: None,
            b: None,
            r: None,
            time: 0.0,
            mode: ViewMode::Side,
            diff_pair: DiffPair::AB,
            show_diff_pane: false,
            gain: 4,
            threshold: 10,
            wipe: 0.5,
            wipe_drag: false,
            zoom: 1.0,
            center: Vec2::ZERO,
            view_reset: true,
            hud_scale: 1.0,
            loader: Loader::new(ctx),
            req_id: 0,
            pending: false,
            loaded: None,
            need_request: false,
            need_diff: false,
            error: None,
            analysis: None,
            running: None,
            analysis_swapped: false,
            progress: 0,
            status: String::new(),
            want_vmaf: false,
            graph_metric: GraphMetric::Ssim,
            probe: None,
        };
        for (slot, p) in [Slot::A, Slot::B, Slot::R].into_iter().zip(files) {
            app.set_media(slot, PathBuf::from(p));
        }
        app
    }

    fn slot_mut(&mut self, s: Slot) -> &mut Option<Media> {
        match s {
            Slot::A => &mut self.a,
            Slot::B => &mut self.b,
            Slot::R => &mut self.r,
        }
    }

    fn cancel_analysis(&mut self) {
        if let Some(h) = &self.running {
            h.cancel.store(true, Ordering::Relaxed);
        }
        self.running = None;
        self.analysis_swapped = false;
        self.status.clear();
    }

    /// Меняет A и B местами, сохраняя результаты анализа (график на таймлайне).
    fn swap_ab(&mut self) {
        std::mem::swap(&mut self.a, &mut self.b);
        let has_ref = self.r.is_some();
        if let Some(an) = self.analysis.as_mut() {
            an.swap_ab(has_ref);
        }
        if self.running.is_some() {
            self.analysis_swapped = !self.analysis_swapped;
        }
        self.need_request = self.a.is_some() && self.b.is_some();
    }

    fn set_media(&mut self, slot: Slot, path: PathBuf) {
        match Media::open(path) {
            Ok(m) => {
                *self.slot_mut(slot) = Some(m);
                self.error = None;
                self.cancel_analysis();
                self.analysis = None;
                self.time = self.time.min(self.max_time());
                self.need_request = self.a.is_some() && self.b.is_some();
                self.view_reset = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Длительность (минимум по всем видео); 0 — если видео нет.
    fn max_time(&self) -> f64 {
        let m = [&self.a, &self.b, &self.r]
            .iter()
            .filter_map(|m| m.as_ref())
            .filter(|m| m.is_video)
            .map(|m| m.duration)
            .fold(f64::INFINITY, f64::min);
        if m.is_finite() {
            m
        } else {
            0.0
        }
    }

    fn fps(&self) -> f64 {
        [&self.r, &self.a, &self.b]
            .iter()
            .filter_map(|m| m.as_ref())
            .find(|m| m.is_video)
            .map(|m| m.fps)
            .unwrap_or(0.0)
    }

    fn step(&mut self, dir: f64, big: bool) {
        let max_t = self.max_time();
        if max_t <= 0.0 {
            return;
        }
        let fps = self.fps();
        let t = if big || fps <= 0.0 {
            self.time + dir
        } else {
            ((self.time * fps).round() + dir) / fps
        };
        self.time = t.clamp(0.0, max_t);
        self.need_request = true;
    }

    fn request_frames(&mut self) {
        let (Some(a), Some(b)) = (&self.a, &self.b) else {
            return;
        };
        self.req_id += 1;
        self.loader.request(LoadRequest {
            id: self.req_id,
            time: self.time,
            a: a.clone(),
            b: b.clone(),
            r: self.r.clone(),
            limit: MAX_SIDE,
        });
        self.pending = true;
    }

    fn compute_diff(&self, set: &FrameSet) -> (Stats, RgbImage) {
        let (x, y) = match (self.diff_pair, &set.r) {
            (DiffPair::AR, Some(r)) => (&set.a, r),
            (DiffPair::BR, Some(r)) => (&set.b, r),
            _ => (&set.a, &set.b),
        };
        diff::compare(x, y, self.gain, self.threshold)
    }

    fn install(&mut self, ctx: &egui::Context, set: FrameSet) {
        let dims = set.a.dimensions();
        let changed = self.loaded.as_ref().map_or(true, |l| l.set.a.dimensions() != dims);

        let tex_a = make_tex(ctx, "a", &set.a);
        let tex_b = make_tex(ctx, "b", &set.b);
        let tex_r = set.r.as_ref().map(|r| make_tex(ctx, "r", r));
        let (diff_stats, d) = self.compute_diff(&set);
        let tex_diff = make_tex(ctx, "diff", &d);

        self.loaded = Some(Loaded { set, tex_a, tex_b, tex_r, tex_diff, diff_stats });
        if changed {
            self.view_reset = true;
        }
        self.error = None;
    }

    fn refresh_diff(&mut self, ctx: &egui::Context) {
        let out = self.loaded.as_ref().map(|l| self.compute_diff(&l.set));
        if let Some((stats, img)) = out {
            let tex = make_tex(ctx, "diff", &img);
            if let Some(l) = self.loaded.as_mut() {
                l.tex_diff = tex;
                l.diff_stats = stats;
            }
        }
    }

    fn can_analyze(&self) -> bool {
        match (&self.a, &self.b) {
            (Some(a), Some(_)) => self.r.as_ref().unwrap_or(a).is_video,
            _ => false,
        }
    }

    fn start_analysis(&mut self, ctx: &egui::Context) {
        let (Some(a), Some(b)) = (&self.a, &self.b) else {
            return;
        };
        let job = AnalysisJob { a: a.clone(), b: b.clone(), r: self.r.clone(), vmaf: self.want_vmaf };
        match worker::start_analysis(job, ctx.clone()) {
            Ok(h) => {
                self.progress = 0;
                self.status.clear();
                self.running = Some(h);
                self.analysis_swapped = false;
                self.analysis = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    // ----- опрос фоновых потоков -----

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.loader.rx.try_recv() {
            if res.id != self.req_id {
                continue; // устаревший ответ
            }
            self.pending = false;
            match res.result {
                Ok(set) => self.install(ctx, set),
                Err(e) => self.error = Some(e),
            }
        }

        let mut finished: Option<Result<Analysis, String>> = None;
        if let Some(h) = &self.running {
            while let Ok(m) = h.rx.try_recv() {
                match m {
                    AnalysisMsg::Progress(n) => self.progress = n,
                    AnalysisMsg::Status(s) => self.status = s,
                    AnalysisMsg::Done(a) => finished = Some(Ok(a)),
                    AnalysisMsg::Failed(e) => finished = Some(Err(e)),
                }
            }
        }
        if let Some(r) = finished {
            self.running = None;
            self.status.clear();
            match r {
                Ok(mut a) => {
                    if self.analysis_swapped {
                        a.swap_ab(self.r.is_some());
                    }
                    self.analysis = Some(a);
                }
                Err(e) => self.error = Some(e),
            }
            self.analysis_swapped = false;
        }
    }

    // ----- ввод: файлы и клавиши -----

    fn handle_input(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        if dropped.len() >= 2 {
            for (p, s) in dropped.into_iter().zip([Slot::A, Slot::B, Slot::R]) {
                self.set_media(s, p);
            }
        } else if let Some(p) = dropped.into_iter().next() {
            let slot = if self.a.is_none() {
                Slot::A
            } else if self.b.is_none() {
                Slot::B
            } else if self.r.is_none() {
                Slot::R
            } else {
                Slot::B
            };
            self.set_media(slot, p);
        }

        let (left, right, shift, k0, k1, k2, k3, k4) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.modifiers.shift,
                i.key_pressed(Key::Num0),
                i.key_pressed(Key::Num1),
                i.key_pressed(Key::Num2),
                i.key_pressed(Key::Num3),
                i.key_pressed(Key::Num4),
            )
        });
        if left {
            self.step(-1.0, shift);
        }
        if right {
            self.step(1.0, shift);
        }
        if k0 {
            self.view_reset = true;
        }
        if k1 {
            self.mode = ViewMode::Side;
        }
        if k2 {
            self.mode = ViewMode::Wipe;
        }
        if k3 {
            self.mode = ViewMode::Blink;
        }
        if k4 {
            self.mode = ViewMode::Diff;
        }
    }

    // ----- верхняя панель -----

    fn slot_ui(&mut self, ui: &mut egui::Ui, slot: Slot) {
        let info = match slot {
            Slot::A => &self.a,
            Slot::B => &self.b,
            Slot::R => &self.r,
        }
        .as_ref()
        .map(|m| (m.name(), m.path.display().to_string()));

        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.5, slot.color());

        let text = match &info {
            Some((n, _)) => format!("{}: {}", slot.tag(), short(n, 24)),
            None if slot == Slot::R => tr("+ Original").to_string(),
            None => trf("Open {0}…", &[&slot.tag()]),
        };
        let mut resp = ui.button(text);
        resp = match &info {
            Some((_, p)) => resp.on_hover_text(p.clone()),
            None if slot == Slot::R => resp
                .on_hover_text(tr("Optional. Reference for an objective quality comparison of A and B")),
            None => resp,
        };
        if resp.clicked() {
            if let Some(p) = pick_file() {
                self.set_media(slot, p);
            }
        }

        if slot == Slot::R && info.is_some() {
            if ui.small_button("×").on_hover_text(tr("Remove original")).clicked() {
                self.r = None;
                self.cancel_analysis();
                self.analysis = None;
                self.need_request = self.a.is_some() && self.b.is_some();
            }
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Compare Media").strong().size(16.0));
                ui.separator();
                for slot in [Slot::A, Slot::B, Slot::R] {
                    self.slot_ui(ui, slot);
                    ui.add_space(4.0);
                }
                if ui.button(tr("Swap A and B")).clicked() {
                    self.swap_ab();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(tr("⚙ Settings")).clicked() {
                        self.cfg_edit = self.cfg.clone();
                        self.show_settings = true;
                    }
                });
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for (m, label) in [
                    (ViewMode::Side, tr("Side by side (1)")),
                    (ViewMode::Wipe, tr("Wipe (2)")),
                    (ViewMode::Blink, tr("Blink (3)")),
                    (ViewMode::Diff, tr("Difference (4)")),
                ] {
                    if ui.selectable_label(self.mode == m, label).clicked() {
                        self.mode = m;
                    }
                }
                ui.separator();

                match self.mode {
                    ViewMode::Side => {
                        ui.checkbox(&mut self.show_diff_pane, tr("add difference pane"));
                    }
                    ViewMode::Diff => {
                        if self.r.is_some() {
                            let old = self.diff_pair;
                            egui::ComboBox::new("diffpair", "")
                                .selected_text(diff_pair_label(self.diff_pair))
                                .show_ui(ui, |ui| {
                                    for p in [DiffPair::AB, DiffPair::AR, DiffPair::BR] {
                                        ui.selectable_value(
                                            &mut self.diff_pair,
                                            p,
                                            diff_pair_label(p),
                                        );
                                    }
                                });
                            if old != self.diff_pair {
                                self.need_diff = true;
                            }
                        } else {
                            ui.weak("A vs B");
                        }
                    }
                    ViewMode::Wipe => {
                        ui.weak(tr("B on the left, A on the right — drag the handle"));
                    }
                    ViewMode::Blink => {
                        ui.weak(tr("automatically alternates between A and B"));
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(tr("Reset view (0)")).clicked() {
                        self.view_reset = true;
                    }
                    ui.label(trf("Zoom: {0}%", &[&format!("{:.0}", self.hud_scale * 100.0)]));
                });
            });
            ui.add_space(6.0);
        });
    }

    // ----- нижние панели -----

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(e) = &self.error {
                    ui.colored_label(Color32::from_rgb(240, 110, 110), e);
                } else {
                    ui.weak(tr("Wheel — zoom · drag — pan · double-click — reset view · ←/→ — frame (Shift — one second) · 1–4 — view mode"));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(p) = &self.probe {
                        ui.monospace(p);
                    }
                });
            });
        });
    }

    fn timeline_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("timeline").show(ctx, |ui| {
            ui.add_space(6.0);
            let max_t = self.max_time();
            let fps = self.fps();

            ui.horizontal(|ui| {
                if ui.button(tr("-1 frame")).clicked() {
                    self.step(-1.0, false);
                }
                if ui.button(tr("+1 frame")).clicked() {
                    self.step(1.0, false);
                }
                ui.monospace(format!("{} / {}", fmt_time(self.time), fmt_time(max_t)));
                if fps > 0.0 {
                    ui.weak(trf("frame {0} · {1} fps", &[&((self.time * fps).round() as i64), &format!("{:.2}", fps)]));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(an) = &self.analysis {
                        ui.selectable_value(&mut self.graph_metric, GraphMetric::Psnr, "PSNR");
                        ui.selectable_value(&mut self.graph_metric, GraphMetric::Ssim, "SSIM");
                        ui.label(tr("Graph:"));
                        ui.colored_label(COLOR_B, "— B");
                        if an.a.is_some() {
                            ui.colored_label(COLOR_A, "— A");
                        }
                    }
                });
            });

            self.scrubber(ui, max_t);
            ui.add_space(6.0);
        });
    }

    fn scrubber(&mut self, ui: &mut egui::Ui, max_t: f64) {
        let height = if self.analysis.is_some() { 88.0 } else { 40.0 };
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
        let weak = ui.visuals().weak_text_color();
        let strong = ui.visuals().strong_text_color();

        let hover_t = resp.hover_pos().map(|p| {
            (((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64) * max_t
        });

        if let Some(an) = &self.analysis {
            let metric = self.graph_metric;
            let (lo, hi) = graph_range(an, metric);
            if let Some(sa) = &an.a {
                draw_series(&painter, rect, series_vals(sa, metric), an.fps, max_t, lo, hi, COLOR_A);
            }
            draw_series(&painter, rect, series_vals(&an.b, metric), an.fps, max_t, lo, hi, COLOR_B);

            let font = FontId::proportional(10.0);
            painter.text(rect.left_top() + vec2(6.0, 3.0), Align2::LEFT_TOP, format!("{hi:.3}"), font.clone(), weak);
            painter.text(rect.left_bottom() + vec2(6.0, -3.0), Align2::LEFT_BOTTOM, format!("{lo:.3}"), font.clone(), weak);

            // значения под курсором
            if let Some(t) = hover_t {
                let idx = (t * an.fps).round() as usize;
                let get = |s: &Series| series_vals(s, metric).get(idx).copied();
                let name = if metric == GraphMetric::Ssim { "SSIM" } else { "PSNR" };
                let mut text = format!("{}  {name}", fmt_time(t));
                if let Some(sa) = &an.a {
                    if let Some(v) = get(sa) {
                        text.push_str(&format!("  A {v:.3}"));
                    }
                }
                if let Some(v) = get(&an.b) {
                    text.push_str(&format!("  B {v:.3}"));
                }
                painter.text(rect.right_top() + vec2(-8.0, 3.0), Align2::RIGHT_TOP, text, font, weak);
            }
        } else if let Some(t) = hover_t {
            painter.text(
                rect.right_top() + vec2(-8.0, 3.0),
                Align2::RIGHT_TOP,
                fmt_time(t),
                FontId::proportional(11.0),
                weak,
            );
        }

        // ползунок воспроизведения
        let frac = if max_t > 0.0 { (self.time / max_t).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let x = rect.left() + frac * rect.width();
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, strong),
        );
        painter.circle_filled(pos2(x, rect.top() + 5.0), 5.0, strong);

        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        // перемотка
        if max_t > 0.0 && (resp.is_pointer_button_down_on() || resp.clicked() || resp.dragged()) {
            if let Some(p) = resp.interact_pointer_pos().or(resp.hover_pos()) {
                let f = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
                let mut t = f * max_t;
                let fps = self.fps();
                if fps > 0.0 {
                    t = (t * fps).round() / fps;
                }
                t = t.clamp(0.0, max_t);
                if (t - self.time).abs() > 1e-6 {
                    self.time = t;
                    self.need_request = true;
                }
            }
        }
    }

    // ----- правая панель -----

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("side")
            .resizable(true)
            .default_width(320.0)
            .min_width(260.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    self.quality_section(ui);
                    ui.add_space(10.0);
                    ui.separator();
                    self.analysis_section(ui);
                    ui.add_space(10.0);
                    ui.separator();
                    self.diff_section(ui);
                });
            });
    }

    fn metric_rows(&self) -> Vec<Row> {
        let Some(l) = &self.loaded else {
            return Vec::new();
        };
        let set = &l.set;
        let mut rows = Vec::new();

        match set.qa {
            Some(qa) => {
                rows.push(Row { name: tr("PSNR, dB (frame)"), a: Some(qa.psnr), b: Some(set.qb.psnr), kind: Kind::Psnr, video: false, tally: true });
                rows.push(Row { name: tr("SSIM (frame)"), a: Some(qa.ssim), b: Some(set.qb.ssim), kind: Kind::Ssim, video: false, tally: true });
            }
            None => {
                rows.push(Row { name: tr("PSNR, dB (frame)"), a: None, b: Some(set.qb.psnr), kind: Kind::Psnr, video: false, tally: false });
                rows.push(Row { name: tr("SSIM (frame)"), a: None, b: Some(set.qb.ssim), kind: Kind::Ssim, video: false, tally: false });
            }
        }

        if let Some(an) = &self.analysis {
            let nz = |v: f64| if v.is_finite() { Some(v) } else { None };
            let sa = an.a.as_ref();
            rows.push(Row { name: tr("PSNR, dB (average)"), a: sa.and_then(|s| nz(s.mean_psnr())), b: nz(an.b.mean_psnr()), kind: Kind::Psnr, video: true, tally: sa.is_some() });
            rows.push(Row { name: tr("SSIM (average)"), a: sa.and_then(|s| nz(s.mean_ssim())), b: nz(an.b.mean_ssim()), kind: Kind::Ssim, video: true, tally: sa.is_some() });
            rows.push(Row { name: tr("SSIM (worst frame)"), a: sa.and_then(|s| nz(s.min_ssim())), b: nz(an.b.min_ssim()), kind: Kind::Ssim, video: true, tally: false });

            let va = an.vmaf_a.as_ref().and_then(|r| r.as_ref().ok().copied());
            let vb = an.vmaf_b.as_ref().and_then(|r| r.as_ref().ok().copied());
            if va.is_some() || vb.is_some() {
                rows.push(Row { name: "VMAF", a: va, b: vb, kind: Kind::Vmaf, video: true, tally: sa.is_some() });
            }
        }
        rows
    }

    fn quality_section(&self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new(tr("Quality")).strong().size(15.0));
        ui.add_space(4.0);

        let Some(l) = &self.loaded else {
            ui.weak(tr("Open two files."));
            return;
        };
        let has_ref = l.set.r.is_some();
        let rows = self.metric_rows();

        if has_ref {
            ui.weak(tr("Metrics are computed against the original: higher means closer to it."));
        } else {
            ui.weak(tr("No original set: A is used as the reference; showing how similar B is to A."));
        }
        ui.add_space(4.0);

        egui::Grid::new("q_grid")
            .num_columns(if has_ref { 3 } else { 2 })
            .spacing([16.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label("");
                if has_ref {
                    ui.colored_label(COLOR_A, RichText::new("A").strong());
                    ui.colored_label(COLOR_B, RichText::new("B").strong());
                } else {
                    ui.colored_label(COLOR_B, RichText::new("B vs A").strong());
                }
                ui.end_row();

                for row in &rows {
                    ui.label(row.name);
                    let w = row.winner();
                    if has_ref {
                        value_cell(ui, row.kind.fmt(row.a), w == Some(true));
                    }
                    value_cell(ui, row.kind.fmt(row.b), has_ref && w == Some(false));
                    ui.end_row();
                }
            });

        if let Some(an) = &self.analysis {
            for (name, r) in [("A", &an.vmaf_a), ("B", &an.vmaf_b)] {
                if let Some(Err(e)) = r {
                    ui.colored_label(Color32::from_rgb(240, 110, 110), format!("VMAF {name}: {e}"));
                }
            }
        }

        // вердикт
        ui.add_space(8.0);
        if has_ref {
            let use_video = rows.iter().any(|r| r.video && r.tally);
            let (mut wa, mut wb, mut n) = (0, 0, 0);
            for r in rows.iter().filter(|r| r.tally && r.video == use_video) {
                n += 1;
                match r.winner() {
                    Some(true) => wa += 1,
                    Some(false) => wb += 1,
                    None => {}
                }
            }
            let (text, color) = if wa > wb {
                (trf("Closer to the original: A ({0} of {1} metrics)", &[&wa, &n]), COLOR_A)
            } else if wb > wa {
                (trf("Closer to the original: B ({0} of {1} metrics)", &[&wb, &n]), COLOR_B)
            } else {
                (tr("The metrics show no clear winner").to_string(), COLOR_D)
            };
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.colored_label(color, RichText::new(text).strong());
                ui.weak(if use_video { tr("based on averages over the whole video") } else { tr("based on the current frame") });
            });
        } else {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.weak(tr("To find out which file is objectively better, add an original (the “+ Original” button at the top). Without a reference you can only measure how much the files differ from each other."));
            });
        }

        // размер и битрейт
        ui.add_space(8.0);
        if let (Some(a), Some(b)) = (&self.a, &self.b) {
            ui.label(trf("Size: A {0} · B {1}", &[&fmt_size(a.size_bytes), &fmt_size(b.size_bytes)]));
            if a.size_bytes > 0 && b.size_bytes > 0 && a.size_bytes != b.size_bytes {
                let d = (b.size_bytes as f64 - a.size_bytes as f64) / a.size_bytes as f64 * 100.0;
                ui.weak(trf(if d < 0.0 { "B is {0}% smaller than A" } else { "B is {0}% larger than A" }, &[&format!("{:.1}", d.abs())]));
            }
            if let (Some(ba), Some(bb)) = (a.bitrate_kbps(), b.bitrate_kbps()) {
                ui.label(trf("Bitrate: A {0} · B {1} kbps", &[&format!("{ba:.0}"), &format!("{bb:.0}")]));
            }
        }

        ui.add_space(6.0);
        ui.collapsing(tr("How to read the metrics"), |ui| {
            ui.label(tr("PSNR is the mean squared error between pixels. Around 30 dB artifacts are visible, 35–40 is good, 45+ is nearly lossless. It ignores human perception."));
            ui.label(tr("SSIM measures structural similarity (luminance, contrast, texture), from 0 to 1. Above 0.95 is good, above 0.98 is nearly indistinguishable."));
            ui.label(tr("VMAF is Netflix's metric and correlates best with viewers' opinions, 0–100. Around 93+ is indistinguishable from the original for most people."));
            ui.label(tr("Compare compression settings only against the same original. Metrics only approximate perception, so for borderline cases look with your own eyes in Wipe and Side by side modes with zoom."));
        });
    }

    fn analysis_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new(tr("Video analysis")).strong().size(15.0));
        ui.weak(tr("PSNR and SSIM for every frame. The graph appears on the timeline."));
        ui.add_space(4.0);
        ui.checkbox(&mut self.want_vmaf, tr("Also compute VMAF (needs ffmpeg with libvmaf, slow)"));
        ui.add_space(4.0);

        if self.running.is_some() {
            let (total, cancel) = {
                let h = self.running.as_ref().unwrap();
                (h.total.max(1), h.cancel.clone())
            };
            let frac = (self.progress as f32 / total as f32).min(1.0);
            let text = if self.status.is_empty() {
                trf("{0} / {1} frames", &[&self.progress, &total])
            } else {
                self.status.clone()
            };
            ui.add(egui::ProgressBar::new(frac).text(text));
            if ui.button(tr("Cancel")).clicked() {
                cancel.store(true, Ordering::Relaxed);
            }
        } else {
            let can = self.can_analyze();
            if ui.add_enabled(can, egui::Button::new(tr("Analyze the whole video"))).clicked() {
                let ctx = ui.ctx().clone();
                self.start_analysis(&ctx);
            }
            if !can {
                ui.weak(tr("Two videos (A and B) are required."));
            }
        }
    }

    fn diff_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new(tr("Difference")).strong().size(15.0));
        ui.add_space(4.0);
        if ui.add(egui::Slider::new(&mut self.gain, 1..=16).text(tr("gain"))).changed() {
            self.need_diff = true;
        }
        if ui.add(egui::Slider::new(&mut self.threshold, 0..=100).text(tr("threshold"))).changed() {
            self.need_diff = true;
        }
        if let Some(l) = &self.loaded {
            ui.add_space(4.0);
            ui.label(trf("{0}% of pixels differ, mean difference {1}/255", &[&format!("{:.2}", l.diff_stats.diff_pct), &format!("{:.2}", l.diff_stats.mean_abs)]));
            ui.weak(match (self.diff_pair, l.set.r.is_some()) {
                (DiffPair::AR, true) => tr("Showing the difference between A and the original"),
                (DiffPair::BR, true) => tr("Showing the difference between B and the original"),
                _ => tr("Showing the difference between A and B"),
            });
        }
    }

    // ----- центральная область -----

    fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();

        let Some(l) = &self.loaded else {
            let pending = self.pending;
            ui.vertical_centered(|ui| {
                ui.add_space(avail.height() / 3.0);
                if pending {
                    ui.spinner();
                }
                ui.label(RichText::new(tr("Drop two files here")).size(22.0));
                ui.weak(tr("images or videos. A third file is the original (optional)."));
            });
            return;
        };

        let (iw, ih) = (l.set.a.width() as f32, l.set.a.height() as f32);
        let id_a = l.tex_a.id();
        let id_b = l.tex_b.id();
        let id_r = l.tex_r.as_ref().map(|t| t.id());
        let id_d = l.tex_diff.id();

        if self.view_reset {
            self.zoom = 1.0;
            self.center = vec2(iw / 2.0, ih / 2.0);
            self.view_reset = false;
        }

        // какие панели показывать
        let blink_b = (ui.input(|i| i.time) * 2.0) as i64 % 2 == 1;
        let mut panes: Vec<(String, TextureId, Color32)> = Vec::new();
        match self.mode {
            ViewMode::Side => {
                if let Some(r) = id_r {
                    panes.push((tr("Original").to_string(), r, COLOR_R));
                }
                panes.push(("A".to_string(), id_a, COLOR_A));
                panes.push(("B".to_string(), id_b, COLOR_B));
                if self.show_diff_pane {
                    panes.push((tr("Difference").to_string(), id_d, COLOR_D));
                }
            }
            ViewMode::Diff => {
                let which = if id_r.is_some() { diff_pair_label(self.diff_pair) } else { "A vs B" };
                panes.push((trf("Difference: {0}", &[&which]), id_d, COLOR_D));
            }
            ViewMode::Wipe => panes.push((String::new(), id_a, COLOR_A)),
            ViewMode::Blink => {
                if blink_b {
                    panes.push(("B".to_string(), id_b, COLOR_B));
                } else {
                    panes.push(("A".to_string(), id_a, COLOR_A));
                }
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
        }

        let n = panes.len();
        let gap = 8.0;
        let pw = (avail.width() - gap * (n as f32 - 1.0)) / n as f32;
        let rects: Vec<Rect> = (0..n)
            .map(|i| {
                Rect::from_min_size(
                    pos2(avail.left() + i as f32 * (pw + gap), avail.top()),
                    vec2(pw, avail.height()),
                )
            })
            .collect();
        let fit = (pw / iw).min(avail.height() / ih).max(1e-6);

        let resp = ui.allocate_rect(avail, Sense::click_and_drag());
        let hover = resp.hover_pos();
        let mut s = fit * self.zoom;

        // --- зум колесом мыши (вокруг курсора, во всех панелях синхронно) ---
        if let Some(hp) = hover {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let k = rects.iter().position(|r| r.contains(hp)).unwrap_or(0);
                let c = rects[k].center();
                let q = self.center + (hp - c) / s;
                let nz = (self.zoom * (scroll * 0.003).exp()).clamp(1.0, 256.0);
                let ns = fit * nz;
                self.center = q - (hp - c) / ns;
                self.zoom = nz;
                s = ns;
            }
        }

        // --- перетаскивание: сдвиг картинки или ручка шторки ---
        let wipe_x_before = avail.left() + avail.width() * self.wipe;
        if resp.drag_started() {
            self.wipe_drag = false;
            if self.mode == ViewMode::Wipe {
                if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                    if (p.x - wipe_x_before).abs() < 18.0 {
                        self.wipe_drag = true;
                    }
                }
            }
        }
        if resp.dragged() {
            if self.wipe_drag {
                if let Some(p) = resp.interact_pointer_pos() {
                    self.wipe = ((p.x - avail.left()) / avail.width()).clamp(0.0, 1.0);
                }
            } else {
                self.center -= resp.drag_delta() / s;
            }
        }
        if resp.drag_stopped() {
            self.wipe_drag = false;
        }
        if resp.double_clicked() {
            self.zoom = 1.0;
            self.center = vec2(iw / 2.0, ih / 2.0);
        }
        if self.zoom <= 1.0001 {
            self.center = vec2(iw / 2.0, ih / 2.0);
        }
        self.center.x = self.center.x.clamp(0.0, iw);
        self.center.y = self.center.y.clamp(0.0, ih);
        s = fit * self.zoom;
        self.hud_scale = s;

        let wipe_x = avail.left() + avail.width() * self.wipe;

        // курсор
        if self.mode == ViewMode::Wipe
            && (self.wipe_drag || hover.map_or(false, |p| (p.x - wipe_x).abs() < 18.0))
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        } else if resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }

        // --- отрисовка ---
        let bg = ui.visuals().extreme_bg_color;
        let image_rect = |pane: Rect| {
            Rect::from_min_size(pane.center() - self.center * s, vec2(iw * s, ih * s))
        };

        if self.mode == ViewMode::Wipe {
            let rect = rects[0];
            let ir = image_rect(rect);
            let pa = ui.painter_at(rect);
            pa.rect_filled(rect, 6.0, bg);
            pa.image(id_a, ir, uv_full(), Color32::WHITE);

            let left_clip = Rect::from_min_max(rect.min, pos2(wipe_x, rect.max.y));
            ui.painter_at(left_clip).image(id_b, ir, uv_full(), Color32::WHITE);

            chip(&pa, rect.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, "B", COLOR_B);
            chip(&pa, rect.right_top() + vec2(-8.0, 8.0), Align2::RIGHT_TOP, "A", COLOR_A);

            pa.line_segment(
                [pos2(wipe_x, rect.min.y), pos2(wipe_x, rect.max.y)],
                Stroke::new(2.0, Color32::WHITE),
            );
            let c = pos2(wipe_x, rect.center().y);
            pa.circle_filled(c, 16.0, Color32::from_rgb(30, 33, 39));
            pa.circle_stroke(c, 16.0, Stroke::new(2.0, Color32::WHITE));
            pa.add(Shape::convex_polygon(
                vec![pos2(c.x - 4.0, c.y - 5.5), pos2(c.x - 4.0, c.y + 5.5), pos2(c.x - 10.5, c.y)],
                Color32::WHITE,
                Stroke::NONE,
            ));
            pa.add(Shape::convex_polygon(
                vec![pos2(c.x + 4.0, c.y - 5.5), pos2(c.x + 4.0, c.y + 5.5), pos2(c.x + 10.5, c.y)],
                Color32::WHITE,
                Stroke::NONE,
            ));
        } else {
            for (i, (label, id, color)) in panes.iter().enumerate() {
                let rect = rects[i];
                let p = ui.painter_at(rect);
                p.rect_filled(rect, 6.0, bg);
                p.image(*id, image_rect(rect), uv_full(), Color32::WHITE);
                chip(&p, rect.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, label, *color);
            }
        }

        if self.pending {
            chip(
                &ui.painter_at(avail),
                avail.right_bottom() + vec2(-10.0, -10.0),
                Align2::RIGHT_BOTTOM,
                tr("Loading…"),
                COLOR_D,
            );
        }

        // --- пиксель под курсором ---
        let mut probe_xy: Option<(u32, u32)> = None;
        if let Some(hp) = hover {
            if let Some(k) = rects.iter().position(|r| r.contains(hp)) {
                let ip = self.center + (hp - rects[k].center()) / s;
                if ip.x >= 0.0 && ip.y >= 0.0 && ip.x < iw && ip.y < ih {
                    probe_xy = Some((ip.x as u32, ip.y as u32));
                }
            }
        }
        self.probe = probe_xy
            .and_then(|(x, y)| self.loaded.as_ref().map(|l| format_probe(&l.set, x, y)));
    }

    // ----- настройки и оверлеи -----

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut save = false;
        let mut restart = false;
        let mut edit = self.cfg_edit.clone();

        egui::Window::new(tr("Settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(tr("Language")).strong());
                egui::ComboBox::new("lang_combo", "")
                    .selected_text(edit.lang.native_name())
                    .show_ui(ui, |ui| {
                        for l in Lang::ALL {
                            ui.selectable_value(&mut edit.lang, l, l.native_name());
                        }
                    });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new(tr("Graphics server")).strong());
                egui::ComboBox::new("backend_combo", "")
                    .selected_text(edit.backend.label())
                    .show_ui(ui, |ui| {
                        for b in [Backend::Auto, Backend::X11, Backend::Wayland] {
                            ui.selectable_value(&mut edit.backend, b, b.label());
                        }
                    });
                ui.weak(tr("If drag-and-drop does not work under Wayland, choose “X11 (via XWayland)”. The change takes effect after a restart."));
                ui.add_space(4.0);
                ui.weak(config::session_description());

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new(tr("Theme")).strong());
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut edit.theme, Theme::Dark, tr("Dark"));
                    ui.selectable_value(&mut edit.theme, Theme::Light, tr("Light"));
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Save")).clicked() {
                        save = true;
                    }
                    if ui.button(tr("Save and restart")).clicked() {
                        restart = true;
                    }
                });
            });

        self.cfg_edit = edit;

        if save || restart {
            self.cfg = self.cfg_edit.clone();
            i18n::set_lang(self.cfg.lang);
            if let Err(e) = self.cfg.save() {
                self.error = Some(e);
            }
            apply_style(ctx, self.cfg.theme);
            self.show_settings = false;

            if restart {
                let files: Vec<String> = [&self.a, &self.b, &self.r]
                    .iter()
                    .filter_map(|m| m.as_ref())
                    .map(|m| m.path.display().to_string())
                    .collect();
                match config::restart(&files) {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => self.error = Some(e),
                }
            }
        } else if !open {
            self.show_settings = false;
            i18n::set_lang(self.cfg.lang);
        } else {
            i18n::set_lang(self.cfg_edit.lang);
        }
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop_overlay"),
        ));
        let r = ctx.screen_rect();
        painter.rect_filled(r, 0.0, Color32::from_black_alpha(170));
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            tr("Drop files: the first is A, the second is B, the third is the original"),
            FontId::proportional(22.0),
            Color32::WHITE,
        );
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        self.handle_input(ctx);

        self.top_bar(ctx);
        self.status_bar(ctx);
        if self.max_time() > 0.0 {
            self.timeline_panel(ctx);
        }
        self.side_panel(ctx);
        egui::CentralPanel::default().show(ctx, |ui| self.canvas_ui(ui));

        self.settings_window(ctx);
        self.drop_overlay(ctx);

        if self.need_request {
            self.need_request = false;
            self.request_frames();
        }
        if self.need_diff {
            self.need_diff = false;
            self.refresh_diff(ctx);
            ctx.request_repaint();
        }
        if self.running.is_some() || self.pending {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

// ---------- вспомогательные функции ----------

fn apply_style(ctx: &egui::Context, theme: Theme) {
    let dark = theme == Theme::Dark;
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let accent = COLOR_A;

    v.selection.bg_fill = accent.gamma_multiply(if dark { 0.55 } else { 0.35 });
    v.hyperlink_color = accent;

    let r = Rounding::same(7.0);
    v.widgets.noninteractive.rounding = r;
    v.widgets.inactive.rounding = r;
    v.widgets.hovered.rounding = r;
    v.widgets.active.rounding = r;
    v.widgets.open.rounding = r;
    v.window_rounding = Rounding::same(12.0);
    v.menu_rounding = Rounding::same(8.0);

    if dark {
        v.panel_fill = Color32::from_rgb(24, 26, 31);
        v.window_fill = Color32::from_rgb(30, 33, 39);
        v.extreme_bg_color = Color32::from_rgb(15, 16, 19);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(40, 44, 52);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 57, 67);
    }
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.slider_width = 150.0;
    });
}

fn uv_full() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

fn pick_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(tr("Images and video"), MEDIA_EXT)
        .pick_file()
}

fn make_tex(ctx: &egui::Context, name: &str, img: &RgbImage) -> TextureHandle {
    let ci = ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw());
    // При увеличении показываем чёткие пиксели, при уменьшении сглаживаем.
    let opts = TextureOptions {
        magnification: TextureFilter::Nearest,
        minification: TextureFilter::Linear,
        ..TextureOptions::LINEAR
    };
    ctx.load_texture(name, ci, opts)
}

/// Небольшая подпись-плашка поверх картинки.
fn chip(painter: &Painter, pos: Pos2, anchor: Align2, text: &str, bg: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(13.0), Color32::WHITE);
    let size = galley.size() + vec2(14.0, 8.0);
    let r = anchor.anchor_size(pos, size);
    painter.rect_filled(r, 5.0, bg.gamma_multiply(0.9));
    painter.galley(r.min + vec2(7.0, 4.0), galley, Color32::WHITE);
}

fn value_cell(ui: &mut egui::Ui, text: String, best: bool) {
    if best {
        ui.colored_label(COLOR_GOOD, RichText::new(text).strong());
    } else {
        ui.label(text);
    }
}

fn diff_pair_label(p: DiffPair) -> &'static str {
    match p {
        DiffPair::AB => "A vs B",
        DiffPair::AR => tr("A vs Original"),
        DiffPair::BR => tr("B vs Original"),
    }
}

fn series_vals(s: &Series, m: GraphMetric) -> &[f32] {
    match m {
        GraphMetric::Ssim => &s.ssim,
        GraphMetric::Psnr => &s.psnr,
    }
}

/// Диапазон оси Y для графика по обоим рядам.
fn graph_range(an: &Analysis, m: GraphMetric) -> (f32, f32) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut scan = |s: &Series| {
        for v in series_vals(s, m) {
            if v.is_finite() {
                min = min.min(*v);
                max = max.max(*v);
            }
        }
    };
    if let Some(sa) = &an.a {
        scan(sa);
    }
    scan(&an.b);

    if !min.is_finite() {
        return (0.0, 1.0);
    }
    match m {
        GraphMetric::Ssim => {
            let lo = ((min - 0.005).max(0.0) * 100.0).floor() / 100.0;
            let hi = 1.0f32;
            (lo.min(hi - 0.01), hi)
        }
        GraphMetric::Psnr => {
            let lo = (min - 1.0).floor().max(0.0);
            let hi = (max + 1.0).ceil();
            (lo, hi.max(lo + 1.0))
        }
    }
}

fn draw_series(
    painter: &Painter,
    rect: Rect,
    vals: &[f32],
    fps: f64,
    max_t: f64,
    lo: f32,
    hi: f32,
    color: Color32,
) {
    if vals.is_empty() || max_t <= 0.0 || fps <= 0.0 {
        return;
    }
    let top = rect.top() + 16.0;
    let bottom = rect.bottom() - 6.0;
    let height = bottom - top;
    let to_y = |v: f32| bottom - ((v - lo) / (hi - lo)).clamp(0.0, 1.0) * height;
    let width = rect.width();
    let cols = (width as usize).max(1);

    let mut pts: Vec<Pos2> = Vec::new();
    if vals.len() <= cols {
        for (i, v) in vals.iter().enumerate() {
            if v.is_finite() {
                let x = rect.left() + (((i as f64 / fps) / max_t).min(1.0) as f32) * width;
                pts.push(pos2(x, to_y(*v)));
            }
        }
    } else {
        // Кадров больше, чем пикселей: на каждый пиксель берём худшее значение,
        // чтобы провалы качества не пропадали.
        let per = vals.len() as f32 / cols as f32;
        for c in 0..cols {
            let s = (c as f32 * per) as usize;
            let e = (((c + 1) as f32 * per) as usize).min(vals.len()).max(s + 1);
            let m = vals[s..e]
                .iter()
                .cloned()
                .filter(|v| v.is_finite())
                .fold(f32::INFINITY, f32::min);
            if m.is_finite() {
                pts.push(pos2(rect.left() + c as f32, to_y(m)));
            }
        }
    }
    if pts.len() >= 2 {
        painter.add(Shape::line(pts, Stroke::new(1.5, color)));
    }
}

fn format_probe(set: &FrameSet, x: u32, y: u32) -> String {
    let px = |img: &RgbImage| {
        let p = img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1));
        format!("{:>3},{:>3},{:>3}", p[0], p[1], p[2])
    };
    let mut s = format!("x {x}  y {y}   A {}   B {}", px(&set.a), px(&set.b));
    if let Some(r) = &set.r {
        s.push_str(&format!("   {} {}", tr("Orig."), px(r)));
    }
    s
}

fn short(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let head: String = chars[..max.saturating_sub(1)].iter().collect();
        format!("{head}…")
    }
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0);
    let m = (t / 60.0).floor() as u64;
    let s = t - (m as f64) * 60.0;
    format!("{m}:{s:06.3}")
}

fn fmt_size(b: u64) -> String {
    let f = b as f64;
    if f >= 1e9 {
        trf("{0} GB", &[&format!("{:.2}", f / 1e9)])
    } else if f >= 1e6 {
        trf("{0} MB", &[&format!("{:.2}", f / 1e6)])
    } else if f >= 1e3 {
        trf("{0} KB", &[&format!("{:.1}", f / 1e3)])
    } else {
        trf("{0} B", &[&b])
    }
}
