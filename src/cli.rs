//! Console mode: visual-diff --cli <A> <B> [out.png]

use crate::diff::{self, Stats};
use crate::i18n::{tr, trf};
use image::RgbImage;
use std::{
    io::Read,
    process::{Child, Command, Stdio},
};

// Size that video frames are scaled to before comparison.
const VW: u32 = 640;
const VH: u32 = 360;
const PIXEL_THRESHOLD: u8 = 10;
const DIFF_GAIN: u32 = 4;

pub fn run(args: &[String]) {
    if args.len() < 2 {
        eprintln!("{}", tr("Usage: visual-diff --cli <A> <B> [out.png]"));
        std::process::exit(1);
    }
    let (a, b) = (&args[0], &args[1]);
    let out = args.get(2).map(String::as_str).unwrap_or("diff.png");

    let result = if diff::is_image(a) && diff::is_image(b) {
        compare_images(a, b, out)
    } else {
        compare_videos(a, b, out)
    };

    if let Err(e) = result {
        eprintln!("{}", trf("Error: {0}", &[&e]));
        std::process::exit(1);
    }
}

fn compare_images(a: &str, b: &str, out: &str) -> Result<(), String> {
    let img_a = image::open(a).map_err(|e| format!("{a}: {e}"))?.to_rgb8();
    let img_b = image::open(b).map_err(|e| format!("{b}: {e}"))?.to_rgb8();
    let img_b = diff::match_size(&img_a, img_b);

    let (stats, d) = diff::compare(&img_a, &img_b, DIFF_GAIN, PIXEL_THRESHOLD);
    print_stats(&stats);
    println!("SSIM: {:.4}", diff::ssim(&img_a, &img_b));

    diff::side_by_side(&img_a, &img_b, &d)
        .save(out)
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        trf("Saved: {0}  (left: A, center: B, right: difference)", &[&out])
    );
    Ok(())
}

fn spawn_decoder(path: &str) -> Result<Child, String> {
    let scale = format!("scale={VW}:{VH}");
    Command::new("ffmpeg")
        .args([
            "-v", "error", "-i", path,
            "-vf", scale.as_str(),
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| trf("failed to run ffmpeg (is it installed and in PATH?): {0}", &[&e]))
}

fn compare_videos(a: &str, b: &str, out: &str) -> Result<(), String> {
    let mut child_a = spawn_decoder(a)?;
    let mut child_b = spawn_decoder(b)?;
    let mut sa = child_a.stdout.take().unwrap();
    let mut sb = child_b.stdout.take().unwrap();

    let frame_len = (VW * VH * 3) as usize;
    let (mut buf_a, mut buf_b) = (vec![0u8; frame_len], vec![0u8; frame_len]);

    let mut n = 0usize;
    let (mut sum_psnr, mut psnr_cnt, mut sum_pct, mut sum_ssim) = (0.0f64, 0usize, 0.0f64, 0.0f64);
    let mut worst: Option<(usize, f64, RgbImage)> = None;

    while sa.read_exact(&mut buf_a).is_ok() && sb.read_exact(&mut buf_b).is_ok() {
        let fa = RgbImage::from_raw(VW, VH, buf_a.clone()).unwrap();
        let fb = RgbImage::from_raw(VW, VH, buf_b.clone()).unwrap();
        let (st, d) = diff::compare(&fa, &fb, DIFF_GAIN, PIXEL_THRESHOLD);
        let ssim = diff::ssim(&fa, &fb);

        if n % 30 == 0 {
            println!(
                "{}",
                trf(
                    "frame {0}: mean difference {1}, PSNR {2} dB, SSIM {3}, {4}% of pixels differ",
                    &[
                        &format!("{n:>6}"),
                        &format!("{:.2}", st.mean_abs),
                        &format!("{:.2}", st.psnr),
                        &format!("{ssim:.4}"),
                        &format!("{:.2}", st.diff_pct),
                    ],
                )
            );
        }
        if st.psnr.is_finite() {
            sum_psnr += st.psnr;
            psnr_cnt += 1;
        }
        sum_pct += st.diff_pct;
        sum_ssim += ssim;

        if worst.as_ref().map_or(true, |(_, p, _)| st.diff_pct > *p) {
            worst = Some((n, st.diff_pct, diff::side_by_side(&fa, &fb, &d)));
        }
        n += 1;
    }

    let _ = child_a.kill();
    let _ = child_b.kill();

    if n == 0 {
        return Err(tr("could not read any frames").to_string());
    }

    println!();
    println!("{}", trf("Frames compared: {0}", &[&n]));
    if psnr_cnt > 0 {
        println!(
            "{}",
            trf("Average PSNR: {0} dB", &[&format!("{:.2}", sum_psnr / psnr_cnt as f64)])
        );
    } else {
        println!("{}", tr("All frames are identical"));
    }
    println!("{}", trf("Average SSIM: {0}", &[&format!("{:.4}", sum_ssim / n as f64)]));
    println!(
        "{}",
        trf("Average difference: {0}% of pixels", &[&format!("{:.2}", sum_pct / n as f64)])
    );

    if let Some((idx, pct, img)) = worst {
        img.save(out).map_err(|e| e.to_string())?;
        println!(
            "{}",
            trf(
                "Most different frame: {0} ({1}%) -> {2}",
                &[&idx, &format!("{pct:.2}"), &out],
            )
        );
    }
    Ok(())
}

fn print_stats(s: &Stats) {
    println!(
        "{}",
        trf("Mean absolute difference: {0} (out of 255)", &[&format!("{:.3}", s.mean_abs)])
    );
    if s.psnr.is_infinite() {
        println!("{}", tr("PSNR: ∞ (images are identical)"));
    } else {
        println!("{}", trf("PSNR: {0} dB", &[&format!("{:.2}", s.psnr)]));
    }
    println!("{}", trf("Pixels that differ: {0}%", &[&format!("{:.2}", s.diff_pct)]));
}
