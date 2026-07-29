//! YUV 共享帧缓冲设备 `/dev/cvi-yuv`（大核侧）。
//!
//! 小核（C906L）抓 MJPEG → JPU 解码 YUV422 → IVE 硬件 CSC 转 RGB888 →
//! 两者都 DMA 直写共享 DRAM → 写邮箱（含 yuv_size/dims）→ HW 邮箱中断通知大核。
//!
//! 大核两种读法：
//! - **mmap**（推荐）：`offset=0` → YUV422 帧，`offset=0x96000` → IVE 转出的
//!   RGB888 planar 帧。用户态直接读非缓存映射，零拷贝。
//! - **read_at**（向后兼容）：拷贝 YUV 帧，带重试。
//!
//! 当前是单缓冲：小核 `decode_to_shared()` 固定写同一片 YUV 缓冲，
//! IVE 也固定写同一片 RGB 缓冲。

use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use ax_memory_addr::{PhysAddr, PhysAddrRange};
use axfs_ng_vfs::{NodeFlags, VfsResult};
use starry_vm::VmMutPtr;

use crate::pseudofs::{DeviceMmap, DeviceOps};

/// DRAM 邮箱（与小核 `bare-metal/src/mailbox.rs` 布局一致，32 字节）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mailbox {
    pub magic: u32,
    pub frame_count: u32,
    pub yuv_size: u32,
    pub flags: u32, // bit0 SOI, bit1 EOI, bit2 slot, bit15 YUV ready, bit8-19 h, bit20-31 w
    pub reply_magic: u32,
    pub reply_data: u32,
    pub reply_seq: u32,
    pub _pad: u32,
}

const DRAM_MBOX_PA: usize = 0x9004_0000;
const DRAM_MBOX_SIZE: usize = core::mem::size_of::<Mailbox>();

/// YUV 帧缓冲：JPU DMA 输出（与小核 `yuv_buf.rs: YUV_BUF_PA` 一致）。
const YUV_SLOT0_PA: usize = 0x8FE8_8000;
const YUV_SLOT_SIZE: usize = 0x96000; // 640×480 YUV422 = 614400

/// RGB888 planar 帧缓冲：IVE CSC 输出（与小核 `yuv_buf.rs: RGB_BUF_PA` 一致）。
/// R/G/B 三平面各 640×480 = 307200，共 921600。
const RGB_BUF_PA: usize = 0x8FF5_E000;
const RGB_BUF_SIZE: usize = 921600;

/// mmap 偏移分配。小核已改单缓冲，原 slot1 偏移改指向 RGB 缓冲——
/// 旧的 `YUV_SLOT1_PA = 0x8FF1E000` 在内存重排后已经是 JPU 内存池，
/// 继续保留会让用户态 mmap 到 JPU 的 stream buffer。
const MMAP_OFF_YUV: usize = 0;
const MMAP_OFF_RGB: usize = YUV_SLOT_SIZE;

/// 向后兼容。
const YUV_BUF_PA: usize = YUV_SLOT0_PA;
const YUV_BUF_SIZE: usize = YUV_SLOT_SIZE;

const MAILBOX_MAGIC: u32 = 0xC906_C906;
const FLAG_YUV_READY: u32 = 1 << 15;

fn decode_dims(flags: u32) -> (u32, u32) {
    ((flags >> 20) & 0xFFF, (flags >> 8) & 0xFFF)
}

fn decode_slot(flags: u32) -> u32 {
    (flags >> 2) & 1
}

pub struct CviYuv {
    mbox_va: AtomicUsize,
    yuv_va: AtomicUsize,
}

impl CviYuv {
    pub fn new() -> Self {
        Self {
            mbox_va: AtomicUsize::new(0),
            yuv_va: AtomicUsize::new(0),
        }
    }

    fn mbox_va(&self) -> Option<usize> {
        Self::lazy_iomap(&self.mbox_va, DRAM_MBOX_PA, DRAM_MBOX_SIZE)
    }

    fn yuv_va(&self) -> Option<usize> {
        Self::lazy_iomap(&self.yuv_va, YUV_BUF_PA, YUV_SLOT_SIZE)
    }

    fn lazy_iomap(slot: &AtomicUsize, pa: usize, size: usize) -> Option<usize> {
        let v = slot.load(Ordering::Acquire);
        if v != 0 {
            return Some(v);
        }
        match ax_mm::iomap(PhysAddr::from_usize(pa), size) {
            Ok(va) => {
                let v = va.as_usize();
                slot.store(v, Ordering::Release);
                Some(v)
            }
            Err(e) => {
                warn!("cvi-yuv: iomap {pa:#x} failed: {e:?}");
                None
            }
        }
    }

    fn read_mbox(&self) -> Option<Mailbox> {
        let va = self.mbox_va()?;
        Some(unsafe { core::ptr::read_volatile(va as *const Mailbox) })
    }

    /// 读邮箱并校验：magic + YUV_READY + yuv_size>0。
    fn read_valid_mbox(&self) -> Option<Mailbox> {
        let mb = self.read_mbox()?;
        if mb.magic == MAILBOX_MAGIC && mb.flags & FLAG_YUV_READY != 0 && mb.yuv_size != 0 {
            Some(mb)
        } else {
            None
        }
    }
}

impl DeviceOps for CviYuv {
    /// 读一帧 YUV 到 `buf`（向后兼容路径，推荐改用 mmap）。
    /// 按邮箱 slot 选 PA，seqlock 风格重试。双缓冲后撕裂概率极低。
    fn read_at(&self, buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        let yuv_base = match self.yuv_va() {
            Some(v) => v,
            None => return Err(ax_errno::AxError::Io),
        };

        let mut last_n = 0usize;
        for _ in 0..3 {
            let mb = match self.read_valid_mbox() {
                Some(m) => m,
                None => return Ok(0),
            };
            let n = (mb.yuv_size as usize).min(buf.len()).min(YUV_SLOT_SIZE);
            if n == 0 {
                return Ok(0);
            }
            // 小核已改单缓冲（decode_to_shared 固定写 YUV_BUF_PA），恒为 slot 0
            let src = yuv_base;
            unsafe {
                core::ptr::copy_nonoverlapping(src as *const u8, buf.as_mut_ptr(), n);
            }
            last_n = n;

            // 双缓冲下 frame_count 推进 ≥2 才可能撕裂（同一 slot 被覆盖）。
            if let Some(mb2) = self.read_mbox() {
                let delta = mb2.frame_count.wrapping_sub(mb.frame_count);
                if delta < 2 {
                    return Ok(n);
                }
                continue;
            }
            return Ok(n);
        }
        Ok(last_n)
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> VfsResult<usize> {
        Ok(0)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    /// mmap：offset 0 → slot 0，offset 0x96000 → slot 1。
    /// DeviceMmap::Physical 自动加 UNCACHED，与 ax_mm::iomap 一致。
    fn mmap(&self, offset: u64, _length: u64) -> DeviceMmap {
        match offset as usize {
            // offset 0 → JPU 解出的 YUV422 帧
            MMAP_OFF_YUV => DeviceMmap::Physical(
                PhysAddrRange::from_start_size(
                    PhysAddr::from_usize(YUV_SLOT0_PA),
                    YUV_SLOT_SIZE,
                ),
                None,
            ),
            // offset 0x96000 → IVE CSC 输出的 RGB888 planar 帧
            MMAP_OFF_RGB => DeviceMmap::Physical(
                PhysAddrRange::from_start_size(
                    PhysAddr::from_usize(RGB_BUF_PA),
                    RGB_BUF_SIZE,
                ),
                None,
            ),
            _ => DeviceMmap::None,
        }
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE | NodeFlags::STREAM
    }
}
