//! yuv-fps — 中断驱动测量小核 (C906L) UVC+JPU → 大核的 FPS。
//!
//! 用阻塞读 /dev/cvi-mailbox 等待小核中断通知新帧（不轮询）。
//! YUV 数据通过 mmap /dev/cvi-yuv 直接读共享 DRAM，零拷贝。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::io::AsRawFd;
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
const YUV_SLOT_SIZE: usize = 0x96000; // 614400

fn decode_dims(flags: u32) -> (u32, u32) {
    ((flags >> 20) & 0xFFF, (flags >> 8) & 0xFFF)
}

fn decode_slot(flags: u32) -> usize {
    ((flags >> 2) & 1) as usize
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

/// mmap 常量（Linux RISC-V 通用）。
const PROT_READ: i32 = 1;
const MAP_SHARED: i32 = 1;
const MAP_FAILED: isize = -1;

extern "C" {
    fn mmap(
        addr: *mut core::ffi::c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut core::ffi::c_void;
}

/// mmap 两个 YUV slot，返回 [slot0_ptr, slot1_ptr]。
fn mmap_yuv_slots(fd: &File) -> [*mut u8; 2] {
    let raw_fd = fd.as_raw_fd();
    let p0 = unsafe { mmap(std::ptr::null_mut(), YUV_SLOT_SIZE, PROT_READ, MAP_SHARED, raw_fd, 0) };
    let p1 = unsafe {
        mmap(
            std::ptr::null_mut(),
            YUV_SLOT_SIZE,
            PROT_READ,
            MAP_SHARED,
            raw_fd,
            YUV_SLOT_SIZE as i64,
        )
    };
    if p0 as isize == MAP_FAILED || p1 as isize == MAP_FAILED {
        panic!("mmap /dev/cvi-yuv failed");
    }
    [p0 as *mut u8, p1 as *mut u8]
}

fn main() {
    let duration = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(30);

    let mut mb_file = File::open("/dev/cvi-mailbox").expect("open /dev/cvi-mailbox");
    let yuv_file = File::open("/dev/cvi-yuv").expect("open /dev/cvi-yuv");

    // mmap 两个 slot —— 零拷贝，用户态直接读非缓存映射
    let yuv_slots = mmap_yuv_slots(&yuv_file);

    // 第一次阻塞读——等待小核第一帧
    println!("waiting for first frame (blocking read)...");
    let first_mb = loop {
        let mb = read_mailbox(&mut mb_file).expect("read mailbox");
        if mb.magic == MAGIC && mb.frame_count > 0 {
            break mb;
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    let (w, h) = decode_dims(first_mb.flags);
    let mjpeg_ready = first_mb.flags & FLAG_YUV_READY != 0;
    let slot = decode_slot(first_mb.flags);
    println!(
        "first frame: count={} yuv={}B {}x{} mjpeg_ready={} slot={}",
        first_mb.frame_count, first_mb.yuv_size, w, h, mjpeg_ready, slot,
    );

    // 中断驱动 FPS 测量：每次阻塞读 = 等待一个中断
    let start = Instant::now();
    let mut frames = 0u32;
    let mut yuv_access = 0u32;
    let mut max_yuv = 0u32;
    let mut min_yuv = u32::MAX;
    let mut prev_slot: usize = 2; // 不可能值，第一帧一定不等

    println!("measuring FPS for {}s (mmap, zero-copy)...", duration);

    let mut last_fc: u32 = first_mb.frame_count;
    while start.elapsed() < Duration::from_secs(duration) {
        if let Some(mb) = read_mailbox(&mut mb_file) {
            if mb.magic == MAGIC && mb.yuv_size > 0 && mb.frame_count != last_fc {
                last_fc = mb.frame_count;
                frames += 1;
                max_yuv = max_yuv.max(mb.yuv_size);
                min_yuv = min_yuv.min(mb.yuv_size);

                let slot = decode_slot(mb.flags);
                // 直接读 mmap 指向的 slot —— 零拷贝，无需 read()
                let ptr = yuv_slots[slot];
                let head = unsafe {
                    u32::from_le_bytes([*ptr, *ptr.add(1), *ptr.add(2), *ptr.add(3)])
                };
                let tail_off = (mb.yuv_size as usize).min(YUV_SLOT_SIZE) - 4;
                let tail = unsafe {
                    u32::from_le_bytes([
                        *ptr.add(tail_off),
                        *ptr.add(tail_off + 1),
                        *ptr.add(tail_off + 2),
                        *ptr.add(tail_off + 3),
                    ])
                };
                yuv_access += 1;

                if yuv_access % 100 == 0 {
                    println!(
                        "mmap[{}] fc={} slot={} yuv={}B head={:#x} tail={:#x}",
                        yuv_access, mb.frame_count, slot, mb.yuv_size, head, tail,
                    );
                }
            }
        }
    }

    let elapsed = start.elapsed();
    let fps = frames as f64 / elapsed.as_secs_f64();

    println!("=== Results ({:.1}s) ===", elapsed.as_secs_f64());
    println!("frames (IRQ-driven) : {}", frames);
    println!("YUV mmap access     : {}", yuv_access);
    println!("YUV size range      : {}-{} bytes", min_yuv, max_yuv);
    println!("FPS (interrupt)     : {:.2}", fps);
}
