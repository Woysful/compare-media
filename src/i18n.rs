//! Локализация интерфейса.
//!
//! Основной язык — английский: английский текст служит ключом. Переводы лежат в таблице TABLE
//! в виде (английский, русский, украинский). Если перевода нет, показывается английский текст.
//!
//! Строки с подстановками используют {0}, {1}, … и функцию `trf`.

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang {
    En,
    Ru,
    Uk,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::En, Lang::Ru, Lang::Uk];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
            Lang::Uk => "uk",
        }
    }

    pub fn from_code(s: &str) -> Option<Lang> {
        match s.trim().to_lowercase().as_str() {
            "en" => Some(Lang::En),
            "ru" => Some(Lang::Ru),
            "uk" | "ua" => Some(Lang::Uk),
            _ => None,
        }
    }

    /// Название языка на нём самом (не переводится).
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
            Lang::Uk => "Українська",
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Lang::En => 0,
            Lang::Ru => 1,
            Lang::Uk => 2,
        }
    }

    fn from_u8(v: u8) -> Lang {
        match v {
            1 => Lang::Ru,
            2 => Lang::Uk,
            _ => Lang::En,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) {
    CURRENT.store(l.to_u8(), Ordering::Relaxed);
}

pub fn lang() -> Lang {
    Lang::from_u8(CURRENT.load(Ordering::Relaxed))
}

/// Перевод строки без подстановок.
pub fn tr(en: &'static str) -> &'static str {
    let l = lang();
    if l == Lang::En {
        return en;
    }
    TABLE
        .iter()
        .find(|e| e.0 == en)
        .map(|e| if l == Lang::Ru { e.1 } else { e.2 })
        .unwrap_or(en)
}

/// Перевод строки с подстановкой {0}, {1}, …
pub fn trf(en: &'static str, args: &[&dyn Display]) -> String {
    let mut s = tr(en).to_string();
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), &a.to_string());
    }
    s
}

// (English, Русский, Українська)
static TABLE: &[(&str, &str, &str)] = &[
    // ----- config -----
    ("Auto (default)", "Авто (по умолчанию)", "Авто (за замовчуванням)"),
    ("X11 (via XWayland)", "X11 (через XWayland)", "X11 (через XWayland)"),
    ("could not determine the settings folder", "не удалось определить папку настроек", "не вдалося визначити теку налаштувань"),
    (
        "X11 selected but DISPLAY is not set (XWayland unavailable). Falling back to auto-detection.",
        "Выбран X11, но переменная DISPLAY не задана (XWayland недоступен). Используется автоопределение.",
        "Обрано X11, але змінну DISPLAY не задано (XWayland недоступний). Використовується автовизначення.",
    ),
    (
        "running via: {0}\nWayland session: {1}\nDISPLAY (X11/XWayland): {2}",
        "запущено через: {0}\nWayland-сессия: {1}\nDISPLAY (X11/XWayland): {2}",
        "запущено через: {0}\nWayland-сесія: {1}\nDISPLAY (X11/XWayland): {2}",
    ),
    ("unknown", "неизвестно", "невідомо"),
    ("yes", "да", "так"),
    ("no", "нет", "ні"),
    ("available", "доступен", "доступний"),
    ("not set", "не задан", "не задано"),
    ("could not determine the program path", "не удалось определить путь к программе", "не вдалося визначити шлях до програми"),
    ("failed to restart: {0}", "не удалось перезапустить: {0}", "не вдалося перезапустити: {0}"),
    // ----- diff / ffmpeg -----
    ("failed to run ffprobe (is it installed?): {0}", "не удалось запустить ffprobe (он установлен?): {0}", "не вдалося запустити ffprobe (він встановлений?): {0}"),
    ("could not read video: {0}", "не удалось прочитать видео: {0}", "не вдалося прочитати відео: {0}"),
    ("failed to run ffmpeg (is it installed?): {0}", "не удалось запустить ffmpeg (он установлен?): {0}", "не вдалося запустити ffmpeg (він встановлений?): {0}"),
    ("ffmpeg returned no frame for {0} at {1} s", "ffmpeg не вернул кадр для {0} на {1} с", "ffmpeg не повернув кадр для {0} на {1} с"),
    ("failed to run ffmpeg: {0}", "не удалось запустить ffmpeg: {0}", "не вдалося запустити ffmpeg: {0}"),
    ("could not parse the VMAF value", "не удалось разобрать значение VMAF", "не вдалося розібрати значення VMAF"),
    ("this ffmpeg build has no libvmaf", "эта сборка ffmpeg без libvmaf", "ця збірка ffmpeg без libvmaf"),
    ("VMAF failed: {0}", "VMAF не посчитался: {0}", "VMAF не порахувався: {0}"),
    // ----- worker -----
    ("Analysis is only available for video", "Анализ доступен только для видео", "Аналіз доступний лише для відео"),
    ("could not get ffmpeg output", "не удалось получить вывод ffmpeg", "не вдалося отримати вивід ffmpeg"),
    ("Analysis cancelled", "Анализ отменён", "Аналіз скасовано"),
    ("could not read any frames (is ffmpeg installed?)", "не удалось прочитать ни одного кадра (ffmpeg установлен?)", "не вдалося прочитати жодного кадру (ffmpeg встановлено?)"),
    ("Computing VMAF…", "Считаю VMAF…", "Рахую VMAF…"),
    // ----- CLI -----
    ("Usage: compare-media --cli <A> <B> [out.png]", "Использование: compare-media --cli <A> <B> [out.png]", "Використання: compare-media --cli <A> <B> [out.png]"),
    ("Error: {0}", "Ошибка: {0}", "Помилка: {0}"),
    ("Saved: {0}  (left: A, center: B, right: difference)", "Сохранено: {0}  (слева A, по центру B, справа разница)", "Збережено: {0}  (ліворуч A, по центру B, праворуч різниця)"),
    ("failed to run ffmpeg (is it installed and in PATH?): {0}", "не удалось запустить ffmpeg (он установлен и есть в PATH?): {0}", "не вдалося запустити ffmpeg (він встановлений і є в PATH?): {0}"),
    ("could not read any frames", "не удалось прочитать ни одного кадра", "не вдалося прочитати жодного кадру"),
    (
        "frame {0}: mean difference {1}, PSNR {2} dB, SSIM {3}, {4}% of pixels differ",
        "кадр {0}: средняя разница {1}, PSNR {2} дБ, SSIM {3}, отличается {4}% пикселей",
        "кадр {0}: середня різниця {1}, PSNR {2} дБ, SSIM {3}, відрізняється {4}% пікселів",
    ),
    ("Frames compared: {0}", "Сравнено кадров: {0}", "Порівняно кадрів: {0}"),
    ("Average PSNR: {0} dB", "Средний PSNR: {0} дБ", "Середній PSNR: {0} дБ"),
    ("All frames are identical", "Все кадры идентичны", "Усі кадри ідентичні"),
    ("Average SSIM: {0}", "Средний SSIM: {0}", "Середній SSIM: {0}"),
    ("Average difference: {0}% of pixels", "Среднее отличие: {0}% пикселей", "Середня відмінність: {0}% пікселів"),
    ("Most different frame: {0} ({1}%) -> {2}", "Самый отличающийся кадр: {0} ({1}%) -> {2}", "Найбільш відмінний кадр: {0} ({1}%) -> {2}"),
    ("Mean absolute difference: {0} (out of 255)", "Средняя абсолютная разница: {0} (из 255)", "Середня абсолютна різниця: {0} (з 255)"),
    ("PSNR: ∞ (images are identical)", "PSNR: ∞ (изображения идентичны)", "PSNR: ∞ (зображення ідентичні)"),
    ("PSNR: {0} dB", "PSNR: {0} дБ", "PSNR: {0} дБ"),
    ("Pixels that differ: {0}%", "Отличающихся пикселей: {0}%", "Пікселів, що відрізняються: {0}%"),
    ("GUI error: {0}", "Ошибка GUI: {0}", "Помилка GUI: {0}"),
    // ----- GUI: top bar -----
    ("Original", "Оригинал", "Оригінал"),
    ("Open {0}…", "Открыть {0}…", "Відкрити {0}…"),
    ("+ Original", "+ Оригинал", "+ Оригінал"),
    ("Optional. Reference for an objective quality comparison of A and B", "Необязательно. Эталон для объективной оценки качества A и B", "Необов'язково. Еталон для об'єктивної оцінки якості A і B"),
    ("Remove original", "Убрать оригинал", "Прибрати оригінал"),
    ("Swap A and B", "Поменять A и B", "Поміняти A і B"),
    ("⚙ Settings", "⚙ Настройки", "⚙ Налаштування"),
    ("Side by side (1)", "Рядом (1)", "Поруч (1)"),
    ("Wipe (2)", "Шторка (2)", "Шторка (2)"),
    ("Blink (3)", "Мигание (3)", "Блимання (3)"),
    ("Difference (4)", "Разница (4)", "Різниця (4)"),
    ("add difference pane", "добавить панель разницы", "додати панель різниці"),
    ("A vs Original", "A vs Оригинал", "A vs Оригінал"),
    ("B vs Original", "B vs Оригинал", "B vs Оригінал"),
    ("B on the left, A on the right — drag the handle", "слева B, справа A — тяните за ручку", "ліворуч B, праворуч A — тягніть за ручку"),
    ("automatically alternates between A and B", "автоматическое переключение A / B", "автоматичне перемикання A / B"),
    ("Reset view (0)", "Сбросить вид (0)", "Скинути вигляд (0)"),
    ("Zoom: {0}%", "Масштаб: {0}%", "Масштаб: {0}%"),
    (
        "Wheel — zoom · drag — pan · double-click — reset view · ←/→ — frame (Shift — one second) · 1–4 — view mode",
        "Колесо — масштаб · перетаскивание — сдвиг · двойной клик — сброс вида · ←/→ — кадр (Shift — секунда) · 1–4 — режим",
        "Колесо — масштаб · перетягування — зсув · подвійний клік — скидання вигляду · ←/→ — кадр (Shift — секунда) · 1–4 — режим",
    ),
    // ----- GUI: timeline -----
    ("-1 frame", "-1 кадр", "-1 кадр"),
    ("+1 frame", "+1 кадр", "+1 кадр"),
    ("frame {0} · {1} fps", "кадр {0} · {1} fps", "кадр {0} · {1} fps"),
    ("Graph:", "График:", "Графік:"),
    // ----- GUI: quality panel -----
    ("Quality", "Качество", "Якість"),
    ("Open two files.", "Откройте два файла.", "Відкрийте два файли."),
    ("Metrics are computed against the original: higher means closer to it.", "Метрики считаются относительно оригинала: больше — ближе к нему.", "Метрики рахуються відносно оригіналу: більше — ближче до нього."),
    ("No original set: A is used as the reference; showing how similar B is to A.", "Оригинал не задан: A принят за эталон, показано сходство B с A.", "Оригінал не задано: A прийнято за еталон, показано схожість B з A."),
    ("PSNR, dB (frame)", "PSNR, дБ (кадр)", "PSNR, дБ (кадр)"),
    ("SSIM (frame)", "SSIM (кадр)", "SSIM (кадр)"),
    ("PSNR, dB (average)", "PSNR, дБ (среднее)", "PSNR, дБ (середнє)"),
    ("SSIM (average)", "SSIM (среднее)", "SSIM (середнє)"),
    ("SSIM (worst frame)", "SSIM (худший кадр)", "SSIM (найгірший кадр)"),
    ("Closer to the original: A ({0} of {1} metrics)", "Ближе к оригиналу: A ({0} из {1} метрик)", "Ближче до оригіналу: A ({0} з {1} метрик)"),
    ("Closer to the original: B ({0} of {1} metrics)", "Ближе к оригиналу: B ({0} из {1} метрик)", "Ближче до оригіналу: B ({0} з {1} метрик)"),
    ("The metrics show no clear winner", "Метрики не выявили явного победителя", "Метрики не виявили явного переможця"),
    ("based on averages over the whole video", "по средним значениям за всё видео", "за середніми значеннями по всьому відео"),
    ("based on the current frame", "по текущему кадру", "за поточним кадром"),
    (
        "To find out which file is objectively better, add an original (the “+ Original” button at the top). Without a reference you can only measure how much the files differ from each other.",
        "Чтобы узнать, какой файл объективно лучше, добавьте оригинал (кнопка «+ Оригинал» сверху): без эталона можно измерить только, насколько файлы отличаются друг от друга.",
        "Щоб дізнатися, який файл об'єктивно кращий, додайте оригінал (кнопка «+ Оригінал» угорі): без еталона можна виміряти лише, наскільки файли відрізняються один від одного.",
    ),
    ("Size: A {0} · B {1}", "Размер: A {0} · B {1}", "Розмір: A {0} · B {1}"),
    ("B is {0}% smaller than A", "B меньше A на {0}%", "B менший за A на {0}%"),
    ("B is {0}% larger than A", "B больше A на {0}%", "B більший за A на {0}%"),
    ("Bitrate: A {0} · B {1} kbps", "Битрейт: A {0} · B {1} кбит/с", "Бітрейт: A {0} · B {1} кбіт/с"),
    ("How to read the metrics", "Как читать метрики", "Як читати метрики"),
    (
        "PSNR is the mean squared error between pixels. Around 30 dB artifacts are visible, 35–40 is good, 45+ is nearly lossless. It ignores human perception.",
        "PSNR — среднеквадратичная ошибка пикселей. Около 30 дБ артефакты заметны, 35–40 хорошо, 45+ почти без отличий. Не учитывает восприятие.",
        "PSNR — середньоквадратична помилка пікселів. Близько 30 дБ артефакти помітні, 35–40 добре, 45+ майже без відмінностей. Не враховує сприйняття.",
    ),
    (
        "SSIM measures structural similarity (luminance, contrast, texture), from 0 to 1. Above 0.95 is good, above 0.98 is nearly indistinguishable.",
        "SSIM — сходство структуры (яркость, контраст, текстуры), от 0 до 1. Выше 0.95 — хорошо, выше 0.98 — почти неотличимо.",
        "SSIM — схожість структури (яскравість, контраст, текстури), від 0 до 1. Вище 0.95 — добре, вище 0.98 — майже непомітно.",
    ),
    (
        "VMAF is Netflix's metric and correlates best with viewers' opinions, 0–100. Around 93+ is indistinguishable from the original for most people.",
        "VMAF — метрика Netflix, лучше всех совпадает с мнением зрителей, 0–100. Около 93+ для большинства неотличимо от оригинала.",
        "VMAF — метрика Netflix, найкраще збігається з думкою глядачів, 0–100. Близько 93+ для більшості невідрізнимо від оригіналу.",
    ),
    (
        "Compare compression settings only against the same original. Metrics only approximate perception, so for borderline cases look with your own eyes in Wipe and Side by side modes with zoom.",
        "Сравнивать настройки сжатия корректно только при одном и том же оригинале. Метрики — приближение к восприятию, поэтому спорные случаи смотрите глазами в режимах «Шторка» и «Рядом» с зумом.",
        "Порівнювати налаштування стиснення коректно лише за одного й того самого оригіналу. Метрики лише наближають сприйняття, тому спірні випадки дивіться очима в режимах «Шторка» та «Поруч» із зумом.",
    ),
    // ----- GUI: analysis / difference -----
    ("Video analysis", "Анализ видео", "Аналіз відео"),
    ("PSNR and SSIM for every frame. The graph appears on the timeline.", "PSNR и SSIM каждого кадра. График появится на таймлайне.", "PSNR і SSIM кожного кадру. Графік з'явиться на таймлайні."),
    ("Also compute VMAF (needs ffmpeg with libvmaf, slow)", "Считать ещё и VMAF (нужен ffmpeg с libvmaf, долго)", "Рахувати також VMAF (потрібен ffmpeg з libvmaf, довго)"),
    ("{0} / {1} frames", "{0} / {1} кадров", "{0} / {1} кадрів"),
    ("Cancel", "Отмена", "Скасувати"),
    ("Analyze the whole video", "Проанализировать всё видео", "Проаналізувати все відео"),
    ("Two videos (A and B) are required.", "Нужны два видео (A и B).", "Потрібні два відео (A і B)."),
    ("Difference", "Разница", "Різниця"),
    ("gain", "усиление", "підсилення"),
    ("threshold", "порог", "поріг"),
    ("{0}% of pixels differ, mean difference {1}/255", "Отличается {0}% пикселей, средняя разница {1}/255", "Відрізняється {0}% пікселів, середня різниця {1}/255"),
    ("Showing the difference between A and the original", "Показана разница A и оригинала", "Показано різницю A та оригіналу"),
    ("Showing the difference between B and the original", "Показана разница B и оригинала", "Показано різницю B та оригіналу"),
    ("Showing the difference between A and B", "Показана разница A и B", "Показано різницю A та B"),
    // ----- GUI: canvas -----
    ("Drop two files here", "Перетащите сюда два файла", "Перетягніть сюди два файли"),
    ("images or videos. A third file is the original (optional).", "изображения или видео. Третий файл — оригинал (необязательно).", "зображення або відео. Третій файл — оригінал (необов'язково)."),
    ("Difference: {0}", "Разница: {0}", "Різниця: {0}"),
    ("Loading…", "Загрузка…", "Завантаження…"),
    ("Orig.", "Ориг.", "Ориг."),
    // ----- GUI: settings -----
    ("Settings", "Настройки", "Налаштування"),
    ("Language", "Язык", "Мова"),
    ("Graphics server", "Графический сервер", "Графічний сервер"),
    (
        "If drag-and-drop does not work under Wayland, choose “X11 (via XWayland)”. The change takes effect after a restart.",
        "Если под Wayland не работает перетаскивание файлов, выберите «X11 (через XWayland)». Изменение применяется после перезапуска.",
        "Якщо під Wayland не працює перетягування файлів, виберіть «X11 (через XWayland)». Зміна застосовується після перезапуску.",
    ),
    ("Theme", "Тема", "Тема"),
    ("Dark", "Тёмная", "Темна"),
    ("Light", "Светлая", "Світла"),
    ("Save", "Сохранить", "Зберегти"),
    ("Save and restart", "Сохранить и перезапустить", "Зберегти й перезапустити"),
    // ----- GUI: misc -----
    ("Drop files: the first is A, the second is B, the third is the original", "Отпустите файлы: первый — A, второй — B, третий — оригинал", "Відпустіть файли: перший — A, другий — B, третій — оригінал"),
    ("Images and video", "Изображения и видео", "Зображення та відео"),
    ("{0} GB", "{0} ГБ", "{0} ГБ"),
    ("{0} MB", "{0} МБ", "{0} МБ"),
    ("{0} KB", "{0} КБ", "{0} КБ"),
    ("{0} B", "{0} Б", "{0} Б"),
];
