//! 大小核通信邮箱设备 `/dev/cvi-mailbox`（中断驱动 + YUV 共享）。
//!
//! 小核（C906L）：UVC 抓帧 → JPU 解码 YUV420 → 写共享 DRAM (0x8FE00000) → 写邮箱 → 触发中断。
//! 大核（StarryOS）：ISR 读邮箱帧信息 → `read_at` 读 32B 邮箱 → `/dev/cvi-yuv` 读 YUV 数据。

use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use ax_memory_addr::PhysAddr;
use axfs_ng_vfs::{NodeFlags, VfsResult};
use starry_vm::VmMutPtr;

use crate::pseudofs::DeviceOps;

/// DRAM 邮箱（帧信息 + 回复，32 字节）。布局与小核 `bare-metal/src/mailbox.rs` 一致。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Mailbox {
    pub magic: u32,
    pub frame_count: u32,
    pub yuv_size: u32,
    pub flags: u32, // bit0 SOI, bit1 EOI, bit15 YUV ready, bit20-31 width, bit8-19 height
    pub reply_magic: u32,
    pub reply_data: u32,
    pub reply_seq: u32,
    pub _pad: u32,
}

const DRAM_MBOX_PA: usize = 0x8FFF_E000;
const DRAM_MBOX_SIZE: usize = core::mem::size_of::<Mailbox>();

/// YUV 共享缓冲区物理地址（dtb 预留区 0x8FE00000，2MB）。
const YUV_BUF_PA: usize = 0x8FE0_0000;
const YUV_BUF_SIZE: usize = 512 * 1024;

const HW_MBOX_PA: usize = 0x0190_0000;
const HW_MBOX_SIZE: usize = 0x1000;
const HW_MBOX_CTX_OFF: usize = 0x400;
const RECEIVE_CPU: usize = 1; // C906B 大核自己——邮箱寄存器索引 = 接收方 CPU
const SLOT: usize = 0;

pub const MAILBOX_MAGIC: u32 = 0xC906_C906;
pub const REPLY_MAGIC: u32 = 0x52504C59;
pub const FLAG_YUV_READY: u32 = 1 << 15;

fn decode_dims(flags: u32) -> (u32, u32) {
    ((flags >> 20) & 0xFFF, (flags >> 8) & 0xFFF)
}

/// PLIC 中断号。大核(C906B)的邮箱中断 = source 101
/// （dts `rtos_cmdqu { interrupts = <101>; interrupt-names = "mailbox"; }`, riscv,ndev=101）。
/// 小核写 cpu_mbox_en[1]+mbox_set 触发；大核读 cpu_mbox_set[1].int_st。
const MAILBOX_IRQ_HW: u32 = 101;

/// 是否真的 enable PLIC source 101。
/// 已实测：小核写 `cpu_mbox_en[1]`+`mbox_set` 后 `pend101=1`——
/// 邮箱中断确实到达大核 PLIC，可以正常使能。
const ENABLE_MBOX_IRQ: bool = true;

/// 往固定数组里格式化的极简 writer——用来把诊断做成 ASCII 从 read() 返回。
/// 进入用户态后内核日志（含 error!）被抑制，这是唯一稳定可见的输出通道。
struct ProbeStr {
    buf: [u8; 256],
    len: usize,
}

impl ProbeStr {
    fn new() -> Self {
        Self { buf: [0; 256], len: 0 }
    }
}

impl core::fmt::Write for ProbeStr {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

/// 流式诊断行的缓存 + 游标。
struct ProbeBuf {
    buf: [u8; 256],
    len: usize,
    cursor: usize,
}

pub struct CviMailbox {
    dram_va: AtomicUsize,
    hw_va: AtomicUsize,
    /// ISR 收到的最新帧计数。
    latest_frame: core::sync::atomic::AtomicU32,
    /// ISR 被调用次数（证明 HW 邮箱中断是否真的到达大核）。
    isr_count: core::sync::atomic::AtomicU32,
    /// 任务侧最后确认到的 isr_count（用于判断"中断跑飞了没人消费"）。
    isr_ack: core::sync::atomic::AtomicU32,
    /// 小缓冲读的流式诊断缓存。
    probe_buf: ax_sync::Mutex<ProbeBuf>,
}

impl CviMailbox {
    /// 构造设备并在**稳定堆地址**上注册 ISR。
    ///
    /// 不要写成 `Arc::new(CviMailbox::new())` 那种在 `new()` 里注册的形式：
    /// `new()` 按值返回，`init_irq` 抓到的 `self` 是即将被移动走的栈临时量，
    /// ISR 拿到的就是野指针（表现为 isr_count 永远读到 0、清中断写到野地址、
    /// 中断无法 deassert 从而把大核打死）。
    /// 这里先建 Arc，再用 `into_raw` 泄漏一份强引用固定地址，然后才注册。
    pub fn new_arc() -> alloc::sync::Arc<Self> {
        let dev = alloc::sync::Arc::new(Self {
            dram_va: AtomicUsize::new(0),
            hw_va: AtomicUsize::new(0),
            latest_frame: core::sync::atomic::AtomicU32::new(0),
            isr_count: core::sync::atomic::AtomicU32::new(0),
            isr_ack: core::sync::atomic::AtomicU32::new(0),
            probe_buf: ax_sync::Mutex::new(ProbeBuf {
                buf: [0; 256],
                len: 0,
                cursor: 0,
            }),
        });
        // 故意泄漏一份强引用：ISR 会长期持有这个裸指针。
        let raw = alloc::sync::Arc::into_raw(dev.clone());
        unsafe { (*raw).init_irq() };
        dev
    }

    fn dram_va(&self) -> Option<usize> {
        Self::lazy_iomap(&self.dram_va, DRAM_MBOX_PA, DRAM_MBOX_SIZE)
    }

    fn hw_va(&self) -> Option<usize> {
        Self::lazy_iomap(&self.hw_va, HW_MBOX_PA, HW_MBOX_SIZE)
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
                warn!("cvi-mailbox: iomap {pa:#x} failed: {e:?}");
                None
            }
        }
    }

    fn read_dram(&self) -> Option<Mailbox> {
        let va = self.dram_va()?;
        Some(unsafe { core::ptr::read_volatile(va as *const Mailbox) })
    }

    /// 任务侧确认已消费到当前 isr_count（诊断用），并做一次幂等的重新武装。
    /// 正常路径下 ISR 已经靠邮箱控制器 deassert 收尾，这里只是保险：
    /// unmask int_mask + 确认 PLIC source 使能，防止任何一次异常把中断永久关死。
    fn ack_and_rearm(&self) {
        unsafe extern "C" {
            fn somehal_plic_set_enable_ctx1(source: u32, on: u32) -> u32;
        }
        let cnt = self.isr_count.load(Ordering::Relaxed);
        self.isr_ack.store(cnt, Ordering::Relaxed);
        let hw = self.hw_va.load(Ordering::Acquire);
        if hw != 0 {
            unsafe {
                core::ptr::write_volatile((hw + 0x10 + RECEIVE_CPU * 16 + 4) as *mut u32, 0);
            }
        }
        unsafe { somehal_plic_set_enable_ctx1(MAILBOX_IRQ_HW, 1) };
    }

    /// 小缓冲读（shell 的 `read` 内建，通常 1 字节/次）走的流式诊断通道。
    /// 用游标逐字节吐出缓存好的诊断行，读到行尾返回 0(EOF) 并复位——
    /// 每次重新格式化会导致永远读不到 '\n' 而死循环。
    fn probe_stream(&self, out: &mut [u8]) -> usize {
        let mut st = self.probe_buf.lock();
        // 先采样：len==0 表示新一轮，必须在 EOF 判断之前填充，
        // 否则首次调用 cursor(0) >= len(0) 会直接返回 EOF，永远采不到样。
        if st.len == 0 {
            let (buf, len) = self.probe_format();
            st.buf = buf;
            st.len = len;
            st.cursor = 0;
        }
        if st.cursor >= st.len {
            // 一行读完：复位，下次 read 重新采样。
            st.cursor = 0;
            st.len = 0;
            return 0;
        }
        let n = out.len().min(st.len - st.cursor);
        out[..n].copy_from_slice(&st.buf[st.cursor..st.cursor + n]);
        st.cursor += n;
        n
    }

    /// 采样一次诊断并格式化成定长 ASCII 行。
    /// 判定小核 notify 触发的邮箱中断卡在哪一环：
    /// 邮箱控制器已置位(st) → PLIC 已 pending(pend101) → ISR 已跑(isr)。
    fn probe_format(&self) -> ([u8; 256], usize) {
        use core::fmt::Write;
        unsafe extern "C" {
            fn somehal_plic_pending(source: u32) -> u32;
            fn somehal_plic_enabled_ctx1(source: u32) -> u32;
            fn somehal_plic_thresh_prio(source: u32) -> u32;
        }
        let hw = self.hw_va.load(Ordering::Acquire);
        let (st, raw, mask, en, mstatus) = if hw != 0 {
            unsafe {
                (
                    // CPU1 邮箱 int 块 @ 0x10+1*16 = 0x20: clr+0, mask+4, st+8, raw+12
                    core::ptr::read_volatile((hw + 0x28) as *const u32),
                    core::ptr::read_volatile((hw + 0x2c) as *const u32),
                    core::ptr::read_volatile((hw + 0x24) as *const u32),
                    core::ptr::read_volatile((hw + 0x04) as *const u32),
                    core::ptr::read_volatile((hw + 0x64) as *const u32),
                )
            }
        } else {
            (0xffff_ffff, 0xffff_ffff, 0xffff_ffff, 0xffff_ffff, 0xffff_ffff)
        };
        let fc = self.read_dram().map(|m| m.frame_count).unwrap_or(0);
        let tp = unsafe { somehal_plic_thresh_prio(101) };
        let mut s = ProbeStr::new();
        let _ = write!(
            s,
            "isr={} fc={} st={:#x} raw={:#x} mask={:#x} en={:#x} mst={:#x} pend101={} pend61={} en101={} thr={} prio={}\n",
            self.isr_count.load(Ordering::Relaxed),
            fc,
            st,
            raw,
            mask,
            en,
            mstatus,
            unsafe { somehal_plic_pending(101) },
            unsafe { somehal_plic_pending(61) },
            unsafe { somehal_plic_enabled_ctx1(101) },
            tp >> 16,
            tp & 0xFFFF,
        );
        (s.buf, s.len)
    }

    fn init_irq(&self) {
        // 关键：ISR 里绝不能触发 ax_mm::iomap（会在中断上下文里挂死大核）。
        // 所以两个映射都必须在注册 ISR **之前**建立好，让 ISR 只走 Acquire load 快路径。
        let dram = self.dram_va();
        let hw = self.hw_va();
        if hw.is_none() {
            warn!("cvi-mailbox: hw iomap failed, refuse to register ISR (would iomap in IRQ ctx)");
            return;
        }
        info!("cvi-mailbox: premapped dram={:#x?} hw={:#x?}", dram, hw);
        // 存非缓存 DRAM VA 供 timer ISR 轮询（后备方案）
        unsafe extern "C" {
            static MBOX_CTRL_VA: core::sync::atomic::AtomicUsize;
        }
        if let Some(dram) = dram {
            unsafe { MBOX_CTRL_VA.store(dram, core::sync::atomic::Ordering::Release); }
            info!("cvi-mailbox: set MBOX_CTRL_VA(dram)={:#x}", dram);
        }
        // 注册 PLIC source 61 ISR（大核独占——小核不启用 source 61）
        let irq = ax_runtime::hal::irq::IrqId {
            domain: ax_runtime::hal::irq::IrqDomainId(7),
            hwirq: ax_runtime::hal::irq::HwIrq(MAILBOX_IRQ_HW),
        };
        let self_ptr = self as *const Self as usize;
        match super::request_shared_disabled(irq, move |_| {
            let dev = unsafe { &*(self_ptr as *const Self) };
            dev.isr_count.fetch_add(1, Ordering::Relaxed);
            // 注意：**不要**在 ISR 里关 PLIC source 止暴。PLIC 对"未使能的 source"
            // 会忽略 complete 写入，source 会永久停在 in-service，之后再也收不到中断
            // （实测表现：isr 停在 1 不再增长）。止暴要靠下面的邮箱控制器 deassert。
            // 只用已预映射的裸 VA（Acquire load），绝不调用 iomap / info! / 加锁。
            let hw = dev.hw_va.load(Ordering::Acquire);
            if hw != 0 {
                unsafe {
                    // CPU1 int 块 @ 0x10+1*16 = 0x20: clr+0, mask+4, st+8, raw+12
                    let int_st = (hw + 0x10 + RECEIVE_CPU * 16 + 8) as *const u32;
                    let int_clr = (hw + 0x10 + RECEIVE_CPU * 16) as *mut u32;
                    let en_p = (hw + RECEIVE_CPU * 4) as *mut u32;
                    let v = core::ptr::read_volatile(int_st);
                    if v != 0 {
                        // 清 pending bit + 关 en bit —— 让邮箱控制器 deassert。
                        // 小核每帧会重新置 en + mbox_set，所以下一帧照样能来中断。
                        core::ptr::write_volatile(int_clr, v);
                        let old = core::ptr::read_volatile(en_p);
                        core::ptr::write_volatile(en_p, old & !v);
                    }
                }
            }
            // 从 DRAM 邮箱取 frame_count（dram_va 已预映射，非缓存）。
            let dram = dev.dram_va.load(Ordering::Acquire);
            if dram != 0 {
                let magic = unsafe { core::ptr::read_volatile(dram as *const u32) };
                if magic == MAILBOX_MAGIC {
                    let fc = unsafe { core::ptr::read_volatile((dram + 4) as *const u32) };
                    if fc > 0 {
                        dev.latest_frame.store(fc, Ordering::Release);
                    }
                }
            }
            ax_runtime::hal::irq::IrqReturn::Handled
        }) {
            Ok(reg) => {
                if !ENABLE_MBOX_IRQ {
                    // 诊断模式：注册但**不** enable，杜绝中断风暴。用探针读 PLIC pending。
                    info!(
                        "cvi-mailbox: PLIC irq {} registered but NOT enabled (diag mode)",
                        MAILBOX_IRQ_HW
                    );
                } else if let Err(e) = reg.enable() {
                    warn!("cvi-mailbox: enable irq failed: {e:?}");
                } else {
                    info!("cvi-mailbox: PLIC irq {} registered + enabled", MAILBOX_IRQ_HW);
                }
                core::mem::forget(reg);
            }
            Err(e) => {
                warn!("cvi-mailbox: register irq {} failed: {e:?}", MAILBOX_IRQ_HW);
            }
        }
    }
}

impl DeviceOps for CviMailbox {
    /// 非阻塞读：立即返回当前邮箱内容（32B）。
    /// ISR 收到中断时更新 latest_frame 并唤醒 wq。
    fn read_at(&self, buf: &mut [u8], _offset: u64) -> VfsResult<usize> {
        // 任务侧确认 + unmask，和 ISR 的风暴刹车配对。
        self.ack_and_rearm();
        if buf.len() < DRAM_MBOX_SIZE {
            // 小缓冲读（shell 的 `read` 内建）：返回 ASCII 诊断行。
            // 内核日志进用户态后被抑制，这是唯一稳定可见的输出通道。
            return Ok(self.probe_stream(buf));
        }
        // 优先用 ISR（真·邮箱 HW 中断）更新的 latest_frame；没有中断时才退回
        // timer ISR 轮询的 MBOX_LATEST_FC。
        unsafe extern "C" {
            static MBOX_LATEST_FC: core::sync::atomic::AtomicU32;
        }
        let isr_fc = self.latest_frame.load(Ordering::Acquire);
        let timer_fc = unsafe { MBOX_LATEST_FC.load(core::sync::atomic::Ordering::Acquire) };
        let mut mb = self.read_dram().unwrap_or_default();
        let best = isr_fc.max(timer_fc);
        if best > mb.frame_count {
            mb.frame_count = best;
        }
        let src = unsafe {
            core::slice::from_raw_parts(&mb as *const Mailbox as *const u8, DRAM_MBOX_SIZE)
        };
        buf[..DRAM_MBOX_SIZE].copy_from_slice(src);
        Ok(DRAM_MBOX_SIZE)
    }

    fn write_at(&self, buf: &[u8], _offset: u64) -> VfsResult<usize> {
        Ok(buf.len())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn flags(&self) -> NodeFlags {
        NodeFlags::NON_CACHEABLE | NodeFlags::STREAM
    }

    fn open(&self, _exclusive: bool) -> VfsResult<()> {
        // 不要在这里 iomap PLIC（0x70000000）——与 PLIC 驱动已有映射冲突会挂死大核。
        // 诊断走 read_at 的小缓冲通道：`read L < /dev/cvi-mailbox`。
        Ok(())
    }
}
