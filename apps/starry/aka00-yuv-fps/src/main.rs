//! yuv-fps — 中断驱动测量小核 (C906L) UVC+JPU → 大核的 FPS。
//!
//! 用阻塞读 /dev/cvi-mailbox 等待小核中断通知新帧（不轮询）。
//! 每收到一帧即读 /dev/cvi-yuv 获取 YUV 数据。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Mailbox {
    magic: u32,
    frame_count: u32,
    yuv_size: u32,
    flags: u32,
    reply_magic: u32,
    reply_data: u32,
    reply_seq: u32,
    _pad: u32,
}

const MAGIC: u32 = 0xC906_C906;
const FLAG_YUV_READY: u32 = 1 << 15;

fn decode_dims(flags: u32) -> (u32, u32) {
    ((flags >> 20) & 0xFFF, (flags >> 8) & 0xFFF)
}

fn read_mailbox(f: &mut File) -> Option<Mailbox> {
    let mut buf = [0u8; 32];
    f.seek(SeekFrom::Start(0)).ok()?;
    f.read_exact(&mut buf).ok()?;
    Some(Mailbox {
        magic: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
        frame_count: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        yuv_size: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        flags: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        reply_magic: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
        reply_data: u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
        reply_seq: u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]),
        _pad: 0,
    })
}

fn main() {
    let duration = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30);

    let mut mb_file = File::open("/dev/cvi-mailbox").expect("open /dev/cvi-mailbox");
    let mut yuv_file = File::open("/dev/cvi-yuv").ok();

    // 第一次阻塞读——等待小核第一帧
    println!("waiting for first frame (blocking read)...");
    let first_mb = loop {
        let mb = read_mailbox(&mut mb_file).expect("read mailbox");
        if mb.magic == MAGIC && mb.frame_count > 0 {
            break mb;
        }
        // read_at 阻塞返回后如果 magic 不对，短暂重试
        std::thread::sleep(Duration::from_millis(100));
    };

    let (w, h) = decode_dims(first_mb.flags);
    let mjpeg_ready = first_mb.flags & FLAG_YUV_READY != 0;
    println!(
        "first frame: count={} yuv={}B {}x{} mjpeg_ready={}",
        first_mb.frame_count, first_mb.yuv_size, w, h, mjpeg_ready,
    );

    // 中断驱动 FPS 测量：每次阻塞读 = 等待一个中断
    let start = Instant::now();
    let mut frames = 0u32;
    let mut yuv_reads = 0u32;
    let mut max_yuv = 0u32;
    let mut min_yuv = u32::MAX;
    let mut yuv_buf = vec![0u8; 0x98000]; // 622592：≥ 640×480 YUV422 (614400)

    println!("measuring FPS for {}s (interrupt-driven)...", duration);

    while start.elapsed() < Duration::from_secs(duration) {
        // 阻塞读——ISR 收到中断后唤醒
        if let Some(mb) = read_mailbox(&mut mb_file) {
            if mb.magic == MAGIC && mb.yuv_size > 0 {
                frames += 1;
                max_yuv = max_yuv.max(mb.yuv_size);
                min_yuv = min_yuv.min(mb.yuv_size);

                // 读 YUV（长度按邮箱 yuv_size，受缓冲容量裁剪）
                if let Some(ref mut yf) = yuv_file {
                    yf.seek(SeekFrom::Start(0)).ok();
                    let want = (mb.yuv_size as usize).min(yuv_buf.len());
                    if yf.read_exact(&mut yuv_buf[..want]).is_ok() {
                        yuv_reads += 1;
                    }
                }
            }
        }
    }

    let elapsed = start.elapsed();
    let fps = frames as f64 / elapsed.as_secs_f64();

    println!("=== Results ({:.1}s) ===", elapsed.as_secs_f64());
    println!("frames (IRQ-driven) : {}", frames);
    println!("YUV reads OK        : {}", yuv_reads);
    println!("YUV size range      : {}-{} bytes", min_yuv, max_yuv);
    println!("FPS (interrupt)     : {:.2}", fps);
}
