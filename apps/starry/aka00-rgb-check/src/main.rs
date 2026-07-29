//! IVE 硬件 CSC 正确性验证工具（大核用户态）。
//!
//! 判据不靠肉眼：mmap 拿到**同一帧**的 YUV 输入和 IVE 输出的 RGB，
//! 在大核上用软件按 BT.601 limited range 重算一遍 RGB 作为参考，逐像素比对。
//! IVE 若正确，差值应在定点舍入的量级（个位数）。
//!
//! 两个未知量一并扫描：
//! - `fmt_sel`：IVE 输入格式编码（无手册，逆向得到的 0 未必对应 YUV422）
//! - 输出平面顺序：系数表名叫 `coef_BT601_to_GBR`，输出可能是 G/B/R 而非 R/G/B
//!
//! 用法：`rgb-check [每个候选值等待的帧数]`

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;

const W: usize = 640;
const H: usize = 480;
const YUV_MAP_SIZE: usize = 0x96000; // 614400
const RGB_MAP_SIZE: usize = 921600; // 640*480*3
const PLANE: usize = W * H; // 307200

const MMAP_OFF_YUV: i64 = 0;
const MMAP_OFF_RGB: i64 = YUV_MAP_SIZE as i64;

const PROT_READ: i32 = 1;
const MAP_SHARED: i32 = 1;

const MAILBOX_MAGIC: u32 = 0xC906_C906;
const FLAG_YUV_READY: u32 = 1 << 15;

/// 小核控制消息 0xF0_49_<cmd>_<arg>，由 `trap.rs::handle_mailbox_irq` 解析。
const CTL_PAUSE: u32 = 0xF049_5000; // arg=1 暂停流水线, 0 恢复
const CTL_FMT: u32 = 0xF049_5600;   // arg = IVE 输入 fmt_sel
const CTL_MUTE: u32 = 0xF049_5700;  // arg=1 静音小核串口, 0 恢复

/// 要扫描的 fmt_sel 候选值。
const FMT_CANDIDATES: &[u32] = &[0, 1, 2, 3, 4, 5, 6, 7];

/// 输出平面顺序候选：(R 在第几个平面, G 在第几个, B 在第几个)。
const PERMS: &[(&str, usize, usize, usize)] = &[
    ("RGB", 0, 1, 2),
    ("GBR", 2, 0, 1),
    ("BRG", 1, 2, 0),
    ("RBG", 0, 2, 1),
    ("GRB", 1, 0, 2),
    ("BGR", 2, 1, 0),
];

extern "C" {
    fn mmap(addr: *mut u8, length: usize, prot: i32, flags: i32, fd: i32, offset: i64)
        -> *mut u8;
}

#[derive(Clone, Copy, Default)]
struct Mailbox {
    magic: u32,
    frame_count: u32,
    yuv_size: u32,
    flags: u32,
}

fn read_mailbox(path: &str) -> Option<Mailbox> {
    let mut f = File::open(path).ok()?;
    let mut buf = [0u8; 32];
    let n = f.read(&mut buf).ok()?;
    if n < 16 {
        return None;
    }
    let g = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    let mb = Mailbox {
        magic: g(0),
        frame_count: g(4),
        yuv_size: g(8),
        flags: g(12),
    };
    (mb.magic == MAILBOX_MAGIC).then_some(mb)
}

/// 软件 BT.601 limited range YUV→RGB 参考实现（Q10 定点，与 IVE 同量级）。
///
/// R = 1.164(Y-16)              + 1.596(V-128)
/// G = 1.164(Y-16) - 0.392(U-128) - 0.813(V-128)
/// B = 1.164(Y-16) + 2.017(U-128)
#[inline]
fn csc_ref(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let yf = (y as i32 - 16) * 1192;
    let uf = u as i32 - 128;
    let vf = v as i32 - 128;
    let cl = |t: i32| (t >> 10).clamp(0, 255) as u8;
    (
        cl(yf + 1634 * vf),
        cl(yf - 401 * uf - 832 * vf),
        cl(yf + 2066 * uf),
    )
}

/// YUV422 planar 取样：色度平面 (W/2) × H，水平 2:1，垂直不下采样。
#[inline]
fn sample_yuv422(yuv: *const u8, x: usize, y: usize) -> (u8, u8, u8) {
    unsafe {
        let ci = y * (W / 2) + x / 2;
        (
            *yuv.add(y * W + x),
            *yuv.add(PLANE + ci),
            *yuv.add(PLANE + (W / 2) * H + ci),
        )
    }
}

/// YUV420 planar 取样：色度平面 (W/2) × (H/2)。用于对照确认源格式。
#[inline]
fn sample_yuv420(yuv: *const u8, x: usize, y: usize) -> (u8, u8, u8) {
    unsafe {
        let ci = (y / 2) * (W / 2) + x / 2;
        (
            *yuv.add(y * W + x),
            *yuv.add(PLANE + ci),
            *yuv.add(PLANE + (W / 2) * (H / 2) + ci),
        )
    }
}

struct Score {
    mean_err: f64,
    max_err: u32,
    ok: u32,
    total: u32,
}

fn score(
    yuv: *const u8,
    rgb: *const u8,
    perm: (usize, usize, usize),
    sampler: fn(*const u8, usize, usize) -> (u8, u8, u8),
) -> Score {
    let (pr, pg, pb) = perm;
    let (mut sum, mut max, mut ok, mut n) = (0u64, 0u32, 0u32, 0u32);
    let mut y = 1;
    while y < H {
        let mut x = 1;
        while x < W {
            let (yy, uu, vv) = sampler(yuv, x, y);
            let (rr, gg, bb) = csc_ref(yy, uu, vv);
            let off = y * W + x;
            let (gr, gg2, gb) = unsafe {
                (
                    *rgb.add(pr * PLANE + off),
                    *rgb.add(pg * PLANE + off),
                    *rgb.add(pb * PLANE + off),
                )
            };
            let d = (gr as i32 - rr as i32).unsigned_abs()
                + (gg2 as i32 - gg as i32).unsigned_abs()
                + (gb as i32 - bb as i32).unsigned_abs();
            sum += d as u64;
            if d > max {
                max = d;
            }
            if d <= 9 {
                ok += 1;
            }
            n += 1;
            x += 7;
        }
        y += 5;
    }
    Score {
        mean_err: sum as f64 / n.max(1) as f64,
        max_err: max,
        ok,
        total: n,
    }
}

/// 画面对比度。全黑或纯色帧下所有格式都"看起来对"，结果没有区分度。
fn frame_stddev(yuv: *const u8) -> f64 {
    let (mut sum, mut sq, mut n) = (0u64, 0u64, 0u64);
    let mut i = 0;
    while i < PLANE {
        let v = unsafe { *yuv.add(i) } as u64;
        sum += v;
        sq += v * v;
        n += 1;
        i += 997;
    }
    let mean = sum as f64 / n as f64;
    (sq as f64 / n as f64 - mean * mean).max(0.0).sqrt()
}

fn ctl(base: u32, arg: u32) -> std::io::Result<()> {
    let msg = base | (arg & 0xFF);
    File::options()
        .write(true)
        .open("/dev/cvi-mailbox")?
        .write_all(&msg.to_le_bytes())?;
    std::thread::sleep(std::time::Duration::from_millis(30));
    Ok(())
}

/// 冻结流水线后把两块缓冲整体快照下来。
///
/// 必须冻结：小核是单缓冲，持续覆盖 YUV 和 RGB。不冻结的话
/// 读 YUV 与读 RGB 可能落在不同帧上，比对结果没有意义。
fn snapshot(yuv: *const u8, rgb: *const u8) -> (Vec<u8>, Vec<u8>) {
    let _ = ctl(CTL_PAUSE, 1);
    std::thread::sleep(std::time::Duration::from_millis(120));
    let y = unsafe { std::slice::from_raw_parts(yuv, YUV_MAP_SIZE) }.to_vec();
    let r = unsafe { std::slice::from_raw_parts(rgb, RGB_MAP_SIZE) }.to_vec();
    let _ = ctl(CTL_PAUSE, 0);
    (y, r)
}

/// 等 n 个新帧完成，确保新设的 fmt 已经作用到 RGB 缓冲上。
fn wait_frames(n: u32) -> Option<Mailbox> {
    let start = read_mailbox("/dev/cvi-mailbox")?.frame_count;
    for _ in 0..600 {
        if let Some(mb) = read_mailbox("/dev/cvi-mailbox") {
            if mb.frame_count.wrapping_sub(start) >= n
                && mb.flags & FLAG_YUV_READY != 0
                && mb.yuv_size as usize == YUV_MAP_SIZE
            {
                return Some(mb);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

fn main() {
    let frames: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    let yuv_file = File::open("/dev/cvi-yuv").expect("open /dev/cvi-yuv");
    let fd = yuv_file.as_raw_fd();

    let yuv = unsafe {
        mmap(std::ptr::null_mut(), YUV_MAP_SIZE, PROT_READ, MAP_SHARED, fd, MMAP_OFF_YUV)
    };
    let rgb = unsafe {
        mmap(std::ptr::null_mut(), RGB_MAP_SIZE, PROT_READ, MAP_SHARED, fd, MMAP_OFF_RGB)
    };
    if (yuv as isize) <= 0 || (rgb as isize) <= 0 {
        println!("mmap failed: yuv={:p} rgb={:p}", yuv, rgb);
        return;
    }
    println!("mmap ok: yuv={:p} rgb={:p}", yuv, rgb);
    // 静音小核串口：UART0 共用，小核每 100 帧一行会把下面的表格冲散
    let _ = ctl(CTL_MUTE, 1);

    if wait_frames(2).is_none() {
        println!("no valid frame; small core running?");
        return;
    }

    let (ys0, rs0) = snapshot(yuv, rgb);
    let sd = frame_stddev(ys0.as_ptr());
    println!("frame Y stddev = {:.1} (>10 才有区分度)", sd);
    if sd < 10.0 {
        println!("!! 画面对比度过低，结果不可信");
    }

    // RGB 缓冲是否被写过（全 0 说明 IVE 根本没输出到这里）
    let nonzero = (0..RGB_MAP_SIZE).step_by(1013).filter(|&i| rs0[i] != 0).count();
    println!("RGB 缓冲非零采样点: {}/{}", nonzero, RGB_MAP_SIZE / 1013);
    if nonzero == 0 {
        println!("!! RGB 缓冲全零 —— IVE 没有写入这块内存");
        return;
    }

    // 决定性诊断：RGB 缓冲到底有没有跟着画面变？
    // 变 → IVE 在写，只是配置错；不变 → IVE 根本没写这块内存。
    {
        let _ = ctl(CTL_FMT, 0);
        let _ = wait_frames(3);
        let (y1, r1) = snapshot(yuv, rgb);
        let _ = wait_frames(6);
        let (y2, r2) = snapshot(yuv, rgb);
        let ydiff = (0..YUV_MAP_SIZE).step_by(311).filter(|&i| y1[i] != y2[i]).count();
        let ytot = YUV_MAP_SIZE.div_ceil(311);
        let rdiff = (0..RGB_MAP_SIZE).step_by(311).filter(|&i| r1[i] != r2[i]).count();
        let rtot = RGB_MAP_SIZE.div_ceil(311);
        println!(
            "帧间变化: YUV {}/{} ({:.0}%) | RGB {}/{} ({:.0}%)",
            ydiff, ytot, ydiff as f64 * 100.0 / ytot as f64,
            rdiff, rtot, rdiff as f64 * 100.0 / rtot as f64
        );
        if rdiff * 20 < ydiff {
            println!("!! RGB 几乎不随画面变 —— IVE 可能没写到这块内存");
        }
        // 各平面分别统计非零/均值，看 IVE 到底往哪几个平面写了
        for pl in 0..3 {
            let base = pl * PLANE;
            let (mut nz, mut sum, mut n) = (0u32, 0u64, 0u32);
            let mut i = 0;
            while i < PLANE {
                let v = r1[base + i];
                if v != 0 { nz += 1; }
                sum += v as u64;
                n += 1;
                i += 101;
            }
            println!("  RGB 平面{}: 非零 {:.0}%  均值 {:.1}", pl,
                nz as f64 * 100.0 / n as f64, sum as f64 / n as f64);
        }
        // Y 平面均值作对照
        let (mut ysum, mut yn) = (0u64, 0u32);
        let mut i = 0;
        while i < PLANE { ysum += y1[i] as u64; yn += 1; i += 101; }
        println!("  Y 平面均值 {:.1}（正确的 RGB 各平面均值应与之接近）",
            ysum as f64 / yn as f64);
    }

    println!();
    println!("扫描 fmt_sel × 平面顺序：mean=三通道绝对误差和的均值, ok=误差≤9 的占比");
    println!();

    let mut best: Option<(u32, &str, f64, u32)> = None;
    let mut best_snap: Option<(Vec<u8>, Vec<u8>)> = None;
    for &fmt in FMT_CANDIDATES {
        if ctl(CTL_FMT, fmt).is_err() {
            println!("fmt={}: 写 /dev/cvi-mailbox 失败", fmt);
            continue;
        }
        if wait_frames(frames).is_none() {
            println!("fmt={}: 等帧超时", fmt);
            continue;
        }
        let (ys, rs) = snapshot(yuv, rgb);
        let mut line = format!("fmt={} |", fmt);
        let mut fmt_best = f64::MAX;
        for &(name, pr, pg, pb) in PERMS {
            let s = score(ys.as_ptr(), rs.as_ptr(), (pr, pg, pb), sample_yuv422);
            line.push_str(&format!(
                " {}:{:6.1}/{:3.0}%",
                name,
                s.mean_err,
                s.ok as f64 * 100.0 / s.total.max(1) as f64
            ));
            if s.mean_err < fmt_best {
                fmt_best = s.mean_err;
            }
            if best.map_or(true, |(_, _, m, _)| s.mean_err < m) {
                best = Some((fmt, name, s.mean_err, s.max_err));
                best_snap = Some((ys.clone(), rs.clone()));
            }
        }
        println!("{}", line);
    }

    println!();
    match best {
        Some((fmt, perm, mean, max)) if mean < 15.0 => {
            println!("PASS: fmt_sel={} 平面顺序={} mean={:.2} max={}", fmt, perm, mean, max);
            println!("      IVE 输出与软件 BT.601 参考一致 —— 硬件 CSC 正确");
        }
        Some((fmt, perm, mean, max)) => {
            println!("FAIL: 最佳 fmt_sel={} {} mean={:.2} max={}", fmt, perm, mean, max);
            println!("      误差过大，格式/平面偏移/系数仍有问题");
        }
        None => println!("FAIL: 无可用组合"),
    }

    let (ys, rs) = best_snap.unwrap_or((ys0, rs0));
    let (yuv, rgb) = (ys.as_ptr(), rs.as_ptr());
    println!();
    let s422 = score(yuv, rgb, (0, 1, 2), sample_yuv422);
    let s420 = score(yuv, rgb, (0, 1, 2), sample_yuv420);
    println!(
        "源采样对照(RGB序): 按422 mean={:.2} / 按420 mean={:.2}",
        s422.mean_err, s420.mean_err
    );

    println!();
    println!("像素抽样: YUV -> 软件参考 / IVE 输出(平面0,1,2)");
    for &(x, y) in &[(100usize, 100usize), (320, 240), (500, 400)] {
        let (yy, uu, vv) = sample_yuv422(yuv, x, y);
        let (rr, gg, bb) = csc_ref(yy, uu, vv);
        let off = y * W + x;
        let got = unsafe {
            (*rgb.add(off), *rgb.add(PLANE + off), *rgb.add(2 * PLANE + off))
        };
        println!(
            "  ({:3},{:3}) Y={:3} U={:3} V={:3} -> ref({:3},{:3},{:3}) ive({:3},{:3},{:3})",
            x, y, yy, uu, vv, rr, gg, bb, got.0, got.1, got.2
        );
    }

    // 恢复小核串口
    let _ = ctl(CTL_MUTE, 0);
}
