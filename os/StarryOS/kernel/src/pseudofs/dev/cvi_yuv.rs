//! YUV 共享帧缓冲读取设备 `/dev/cvi-yuv`（大核侧）。
//!
//! 小核（C906L）抓 MJPEG → JPU 解码 YUV420 → 写共享 DRAM 单缓冲 0x8FE90000
//! → 写 DRAM 邮箱（0x8FFFE000，含 yuv_size/dims）→ HW 邮箱中断通知大核。
//!
//! 大核本设备 iomap DRAM 邮箱 + YUV 缓冲，`read_at` 时：
//! 1. 读邮箱拿 `(frame_count, yuv_size, dims, flags)`；
//! 2. 从 0x8FE90000 拷 `yuv_size` 字节到用户缓冲；
//! 3. 重读邮箱 frame_count——若推进 ≥2 说明该帧已被小核覆盖（单缓冲窗口耗尽），
//!    重试拿最新帧再拷（seqlock 风格，最多 3 次）。
//!
//! 本设备**不**触碰 JPU 寄存器——JPU 所有权归小核（见 `bare-metal/src/jpu.rs`）。

use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use ax_memory_addr::PhysAddr;
use axfs_ng_vfs::{NodeFlags, VfsResult};
use starry_vm::VmMutPtr;

use crate::pseudofs::DeviceOps;

/// DRAM 邮箱（与小核 `bare-metal/src/mailbox.rs` 布局一致，32 字节）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mailbox {
    pub magic: u32,
    pub frame_count: u32,
    pub yuv_size: u32,
    pub flags: u32, // bit0 SOI, bit1 EOI, bit15 YUV ready, bit20-31 w, bit8-19 h
    pub reply_magic: u32,
    pub reply_data: u32,
    pub reply_seq: u32,
    pub _pad: u32,
}

const DRAM_MBOX_PA: usize = 0x8FFF_E000;
const DRAM_MBOX_SIZE: usize = core::mem::size_of::<Mailbox>();

/// YUV 共享缓冲区物理地址（rtos_region 内，与小核 `bare-metal/src/yuv_buf.rs` 一致）。
const YUV_BUF_PA: usize = 0x8FE8_8000;
const YUV_BUF_SIZE: usize = 0x96000; // 640×480 YUV422 = 614400

const MAILBOX_MAGIC: u32 = 0xC906_C906;
const FLAG_YUV_READY: u32 = 1 << 15;

fn decode_dims(flags: u32) -> (u32, u32) {
    ((flags >> 20) & 0xFFF, (flags >> 8) & 0xFFF)
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
        Self::lazy_iomap(&self.yuv_va, YUV_BUF_PA, YUV_BUF_SIZE)
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
    /// 读一帧 YUV 到 `buf`。返回拷贝的字节数（= 邮箱 yuv_size，受 buf/缓冲容量裁剪）。
    /// 若邮箱无有效帧（magic 不符或无 YUV_READY），返回 0。
    fn read_at(&self, buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        let yuv_va = match self.yuv_va() {
            Some(v) => v,
            None => return Err(ax_errno::AxError::Io),
        };

        // seqlock 风格：读邮箱 → 拷 YUV → 重读邮箱验证帧未被覆盖。
        let mut last_n = 0usize;
        for _ in 0..3 {
            let mb = match self.read_valid_mbox() {
                Some(m) => m,
                None => return Ok(0),
            };
            let n = (mb.yuv_size as usize).min(buf.len()).min(YUV_BUF_SIZE);
            if n == 0 {
                return Ok(0);
            }
            unsafe {
                core::ptr::copy_nonoverlapping(yuv_va as *const u8, buf.as_mut_ptr(), n);
            }
            last_n = n;

            // 重读邮箱：若 frame_count 推进 ≥2，该帧 YUV 可能已被小核覆盖，重试。
            if let Some(mb2) = self.read_mbox() {
                let delta = mb2.frame_count.wrapping_sub(mb.frame_count);
                if delta < 2 {
                    return Ok(n);
                }
                continue; // 单缓冲窗口耗尽——用最新邮箱重试
            }
            return Ok(n);
        }
        // 重试耗尽：返回最后一次拷贝（可能略有撕裂，但仍有数据）。
        Ok(last_n)
    }

    fn write_at(&self, _buf: &[u8], _offset: u64) -> VfsResult<usize> {
        Ok(0)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE | NodeFlags::STREAM
    }
}
