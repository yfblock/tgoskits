use alloc::{format, vec, vec::Vec};
use core::{num::NonZeroU32, ptr::NonNull};

use ax_riscv_plic::{PLICRegs, Plic, PlicIrqHandler};
use kernutil::StaticCell;
use rdif_intc::Interface;
use rdrive::{
    Device, DriverGeneric, Phandle, module_driver,
    probe::{OnProbeError, fdt::NodeType},
    register::{FdtInfo, ProbeFdt},
};
use riscv::register::{sie, sip};
use sbi_rt::HartMask;

use crate::{
    common::ioremap,
    irq_routing::{
        RISCV_S_EXT_IRQ, RISCV_S_SOFT_IRQ, RISCV_S_TIMER_IRQ, RiscvTrapIrq, classify_riscv_trap,
        riscv_plic_hwirq_from_source, riscv_source_from_plic_hwirq,
    },
};

const SUPERVISOR_EXTERNAL_INTERRUPT: u32 = 9;
const DEFAULT_PRIORITY: u32 = 1;
const DEFAULT_PLIC_SIZE: usize = 0x400_0000;

static IRQ_HANDLER: StaticCell<RiscvPlicIrqHandler> = StaticCell::uninit();

module_driver!(
    name: "RISC-V PLIC",
    level: ProbeLevel::PreKernel,
    priority: ProbePriority::INTC,
    probe_kinds: &[ProbeKind::Fdt {
        compatibles: &[
            "riscv,plic0",
            "sifive,plic-1.0.0",
            "starfive,jh7110-plic",
        ],
        on_probe: probe_plic
    }],
);

pub fn systick_irq() -> rdrive::IrqId {
    RISCV_S_TIMER_IRQ.into()
}

/// 查询某 PLIC source 是否 pending（用于诊断邮箱中断是否真的到了 PLIC）。
pub fn plic_is_pending(source: u32) -> Option<bool> {
    let src = NonZeroU32::new(source)?;
    with_plic("querying pending", |plic| plic.inner.is_pending(src))
}

/// PLIC 基址 VA（probe 时记录），供无锁诊断读寄存器用。
static PLIC_BASE_VA: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// 诊断入口：供 starry-kernel（非 somehal 依赖方）用 `extern "C"` 调用。
/// **无锁**——直接 volatile 读 PLIC pending 寄存器。
/// 不要用 `with_plic`（取 rdrive 设备锁会挂死调用方），也不要新建
/// `iomap(0x70000000)`（与 PLIC 驱动已有映射冲突同样会挂死）。
/// 返回 1=pending, 0=not pending, 0xFFFF_FFFF=PLIC 未就绪。
#[unsafe(no_mangle)]
pub extern "C" fn somehal_plic_pending(source: u32) -> u32 {
    let base = PLIC_BASE_VA.load(core::sync::atomic::Ordering::Acquire);
    if base == 0 || source == 0 {
        return 0xFFFF_FFFF;
    }
    // pending 位图 @ +0x1000，每 32 个 source 一个 word
    let word = (base + 0x1000 + (source as usize / 32) * 4) as *const u32;
    let bit = source % 32;
    let v = unsafe { core::ptr::read_volatile(word) };
    (v >> bit) & 1
}

/// 全局非缓存邮箱控制器 VA（由 starry_kernel 的 cvi_mailbox 设置，timer ISR 读取）。
#[unsafe(no_mangle)]
pub static MBOX_CTRL_VA: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// timer ISR 检测到的最新 frame_count（无锁，中断上下文安全）。
#[unsafe(no_mangle)]
pub static MBOX_LATEST_FC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn local_irq_set_enable(irq: rdrive::IrqId, enable: bool) -> Result<(), crate::irq::IrqError> {
    let raw: usize = irq.into();
    match raw {
        RISCV_S_TIMER_IRQ => unsafe {
            if enable {
                sie::set_stimer();
            } else {
                sie::clear_stimer();
            }
            Ok(())
        },
        RISCV_S_SOFT_IRQ => unsafe {
            if enable {
                sie::set_ssoft();
            } else {
                sie::clear_ssoft();
            }
            Ok(())
        },
        RISCV_S_EXT_IRQ => unsafe {
            if enable {
                sie::set_sext();
            } else {
                sie::clear_sext();
            }
            Ok(())
        },
        other => {
            warn!("unsupported RISC-V local IRQ {other:#x}");
            Err(crate::irq::IrqError::InvalidIrq)
        }
    }
}

pub fn irq_set_affinity(
    hwirq: rdif_intc::HwIrq,
    affinity: crate::irq::IrqAffinity,
) -> Result<(), crate::irq::IrqError> {
    let source = NonZeroU32::new(hwirq.0).ok_or(crate::irq::IrqError::InvalidIrq)?;
    with_plic("setting PLIC IRQ affinity", |plic| {
        plic.set_source_affinity(source, affinity)
    })
    .flatten()
    .ok_or(crate::irq::IrqError::InvalidIrq)
}

enum Completion {
    None,
    Plic(NonZeroU32),
}

pub struct ActiveIrq {
    irq: rdrive::IrqId,
    completion: Completion,
}

impl ActiveIrq {
    pub fn id(&self) -> rdrive::IrqId {
        self.irq
    }
}

impl Drop for ActiveIrq {
    fn drop(&mut self) {
        if let Completion::Plic(source) = self.completion {
            complete_external_irq_source(source);
        }
    }
}

pub fn begin_irq(raw: usize) -> Option<ActiveIrq> {
    match classify_riscv_trap(raw) {
        RiscvTrapIrq::Timer => {
            // 邮箱改用真·HW 中断（PLIC source 101）。这里的轮询后备暂时关掉，
            // 否则无法分辨帧号是中断送来的还是定时器轮询出来的。
            // 若需恢复后备方案，把 MBOX_POLL_FALLBACK 改回 true。
            const MBOX_POLL_FALLBACK: bool = false;
            if MBOX_POLL_FALLBACK {
                static TCNT: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let tn = TCNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if tn % 50 == 0 {
                    let va = MBOX_CTRL_VA.load(core::sync::atomic::Ordering::Acquire);
                    if va != 0 {
                        let magic = unsafe { core::ptr::read_volatile(va as *const u32) };
                        if magic == 0xC906_C906 {
                            let fc = unsafe { core::ptr::read_volatile((va + 4) as *const u32) };
                            MBOX_LATEST_FC.store(fc, core::sync::atomic::Ordering::Release);
                        }
                    }
                }
            }
            Some(ActiveIrq {
                irq: RISCV_S_TIMER_IRQ.into(),
                completion: Completion::None,
            })
        }
        RiscvTrapIrq::Ipi => {
            unsafe {
                sip::clear_ssoft();
            }
            static IPI_ONCE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let n = IPI_ONCE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 3 {
                info!("cvi-irq: SSI (IPI) received #{}", n);
            }
            Some(ActiveIrq {
                irq: RISCV_S_SOFT_IRQ.into(),
                completion: Completion::None,
            })
        }
        RiscvTrapIrq::External => begin_external_irq(),
        RiscvTrapIrq::UnknownInterrupt { cause } => {
            warn!("unsupported RISC-V interrupt cause {cause}");
            None
        }
        RiscvTrapIrq::BareSource(source) => {
            warn!("ignore bare RISC-V PLIC source {source} outside external interrupt claim path");
            None
        }
    }
}

fn begin_external_irq() -> Option<ActiveIrq> {
    let source = claim_external_irq_source()?;
    let s = source.get() as usize;
    static SEEN: [core::sync::atomic::AtomicBool; 128] =
        [const { core::sync::atomic::AtomicBool::new(false) }; 128];
    if s < 128 && !SEEN[s].swap(true, core::sync::atomic::Ordering::SeqCst) {
        info!("cvi-irq: claimed PLIC source {}", s);
    }
    Some(ActiveIrq {
        irq: (source.get() as usize).into(),
        completion: Completion::Plic(source),
    })
}

fn complete_external_irq_source(source: NonZeroU32) {
    if let Some(handler) = get_irq_handler() {
        handler.complete_current(source);
    } else {
        warn!("RISC-V PLIC IRQ handler is not registered when completing external IRQ");
    }
}

pub fn secondary_init_intc(cpu_idx: usize) {
    if let Some(handler) = get_irq_handler() {
        handler.init_context(cpu_idx);
    }
    enable_local_interrupts();
}

pub fn send_ipi_to_cpu(cpu_id: usize) {
    let Some(hart_id) = someboot::smp::cpu_idx_to_id(cpu_id) else {
        warn!("failed to resolve hart id for logical CPU {cpu_id}");
        return;
    };
    let res = sbi_rt::send_ipi(HartMask::from_mask_base(1, hart_id));
    if !res.is_ok() {
        warn!("send_ipi to hart {hart_id} failed: {res:?}");
    }
}

/// 诊断入口：无锁读 context 1(大核 S-mode) 的 enable 位。
/// 返回 1=enabled, 0=disabled, 0xFFFF_FFFF=PLIC 未就绪。
#[unsafe(no_mangle)]
pub extern "C" fn somehal_plic_enabled_ctx1(source: u32) -> u32 {
    let base = PLIC_BASE_VA.load(core::sync::atomic::Ordering::Acquire);
    if base == 0 || source == 0 {
        return 0xFFFF_FFFF;
    }
    // enable 位图 @ +0x2000 + context*0x80
    let word = (base + 0x2000 + 0x80 + (source as usize / 32) * 4) as *const u32;
    let bit = source % 32;
    let v = unsafe { core::ptr::read_volatile(word) };
    (v >> bit) & 1
}

/// 诊断入口：无锁读 context 1 的 threshold 与某 source 的 priority。
/// 返回 `(threshold << 16) | priority`。
#[unsafe(no_mangle)]
pub extern "C" fn somehal_plic_thresh_prio(source: u32) -> u32 {
    let base = PLIC_BASE_VA.load(core::sync::atomic::Ordering::Acquire);
    if base == 0 {
        return 0xFFFF_FFFF;
    }
    let thresh = unsafe {
        core::ptr::read_volatile((base + 0x20_0000 + 0x1000) as *const u32)
    };
    let prio = unsafe { core::ptr::read_volatile((base + source as usize * 4) as *const u32) };
    ((thresh & 0xFFFF) << 16) | (prio & 0xFFFF)
}

/// 诊断/止暴入口：无锁改 context 1(大核 S-mode) 的 enable 位。
/// 供中断上下文调用——`with_plic` 会取 rdrive 锁，在 ISR 里用必然死锁。
/// ISR 里关掉 source、任务上下文再打开，可以把中断速率钳死成"每次消费一个"，
/// 无论外设那边 deassert 有没有生效都不会形成风暴。
#[unsafe(no_mangle)]
pub extern "C" fn somehal_plic_set_enable_ctx1(source: u32, on: u32) -> u32 {
    let base = PLIC_BASE_VA.load(core::sync::atomic::Ordering::Acquire);
    if base == 0 || source == 0 {
        return 0xFFFF_FFFF;
    }
    let word = (base + 0x2000 + 0x80 + (source as usize / 32) * 4) as *mut u32;
    let bit = 1u32 << (source % 32);
    unsafe {
        let v = core::ptr::read_volatile(word);
        let nv = if on != 0 { v | bit } else { v & !bit };
        core::ptr::write_volatile(word, nv);
    }
    0
}

fn probe_plic(probe: ProbeFdt<'_>) -> Result<(), OnProbeError> {
    let (info, dev) = probe.into_parts();
    let reg = info
        .node
        .regs()
        .into_iter()
        .next()
        .ok_or_else(|| OnProbeError::other(format!("[{}] has no reg", info.node.name())))?;
    let mmio = ioremap(
        reg.address,
        reg.size.unwrap_or(DEFAULT_PLIC_SIZE as u64) as usize,
    )
    .map_err(|err| OnProbeError::other(format!("failed to map PLIC: {err:?}")))?;
    let plic = unsafe {
        Plic::new(
            NonNull::new(mmio.as_ptr() as *mut PLICRegs)
                .ok_or_else(|| OnProbeError::other("PLIC MMIO mapping is null"))?,
        )
    };
    // 记下 PLIC 基址 VA，供无锁诊断读 pending 位（见 somehal_plic_pending）。
    PLIC_BASE_VA.store(mmio.as_ptr() as usize, core::sync::atomic::Ordering::Release);
    let ndev = info
        .node
        .as_node()
        .get_property("riscv,ndev")
        .and_then(|prop| prop.get_u32())
        .unwrap_or(1024) as usize;
    let mut plic = plic;
    plic.disable_all_sources(ndev);
    let contexts = parse_supervisor_contexts(&info);
    for context in contexts.iter().filter_map(|context| *context) {
        plic.disable_context_sources(context);
    }

    let irq_handler = RiscvPlicIrqHandler {
        inner: plic.irq_handler(),
        context_by_cpu: contexts.clone(),
    };
    IRQ_HANDLER.init(irq_handler);
    if let Some(handler) = get_irq_handler() {
        handler.reset_all_contexts();
    }
    let plic = RiscvPlic {
        inner: plic,
        context_by_cpu: contexts,
        affinity_by_source: vec![crate::irq::IrqAffinity::Any; ndev.saturating_add(1)],
        enabled_by_source: vec![false; ndev.saturating_add(1)],
        sources: ndev,
    };
    enable_local_interrupts();

    let domain = crate::irq::alloc_irq_domain(
        dev.descriptor.device_id(),
        crate::irq::IrqDomainKind::RiscvPlic,
    )
    .map_err(|err| OnProbeError::other(format!("failed to register PLIC domain: {err:?}")))?;
    dev.register(rdif_intc::Intc::new(domain, plic));
    Ok(())
}

fn parse_supervisor_contexts(info: &FdtInfo<'_>) -> Vec<Option<usize>> {
    let mut contexts = Vec::new();
    let Some(prop) = info.node.as_node().get_property("interrupts-extended") else {
        return contexts;
    };

    let mut reader = prop.as_reader();
    let mut context = 0;
    while let (Some(phandle), Some(interrupt)) = (reader.read_u32(), reader.read_u32()) {
        if interrupt == SUPERVISOR_EXTERNAL_INTERRUPT
            && let Some(cpu_idx) = cpu_idx_from_intc_phandle(info, Phandle::from(phandle))
        {
            if contexts.len() <= cpu_idx {
                contexts.resize(cpu_idx + 1, None);
            }
            contexts[cpu_idx] = Some(context);
        }
        context += 1;
    }
    contexts
}

fn cpu_idx_from_intc_phandle(info: &FdtInfo<'_>, phandle: Phandle) -> Option<usize> {
    let intc = info.get_by_phandle(phandle)?;
    if let Some(cpu_idx) = intc.parent().and_then(|cpu| cpu_idx_from_cpu_node(&cpu)) {
        return Some(cpu_idx);
    }
    let cpu = info.get_by_phandle(intc.as_node().interrupt_parent()?)?;
    cpu_idx_from_cpu_node(&cpu)
}

fn cpu_idx_from_cpu_node(cpu: &NodeType<'_>) -> Option<usize> {
    let hart_id = cpu.regs().first()?.address as usize;
    someboot::smp::cpu_id_to_idx(hart_id)
}

fn enable_local_interrupts() {
    unsafe {
        sie::set_ssoft();
        sie::set_stimer();
        sie::set_sext();
    }
}

fn claim_external_irq_source() -> Option<NonZeroU32> {
    let Some(handler) = get_irq_handler() else {
        warn!("RISC-V PLIC IRQ handler is not registered for external IRQ");
        return None;
    };
    handler.claim_current()
}

fn with_plic<R>(op: &str, f: impl FnOnce(&mut RiscvPlic) -> R) -> Option<R> {
    let Some(intc) = get_plic() else {
        warn!("RISC-V PLIC is not registered when {op}");
        return None;
    };
    let Ok(mut intc) = intc.lock() else {
        warn!("failed to lock RISC-V PLIC when {op}");
        return None;
    };
    let Some(plic) = intc.typed_mut::<RiscvPlic>() else {
        warn!("registered interrupt controller is not RISC-V PLIC when {op}");
        return None;
    };
    Some(f(plic))
}

fn get_plic() -> Option<Device<rdif_intc::Intc>> {
    if !rdrive::is_initialized() {
        return None;
    }
    rdrive::get_one()
}

fn get_irq_handler() -> Option<&'static RiscvPlicIrqHandler> {
    if IRQ_HANDLER.is_init() {
        Some(&IRQ_HANDLER)
    } else {
        None
    }
}

struct RiscvPlic {
    inner: Plic,
    context_by_cpu: Vec<Option<usize>>,
    affinity_by_source: Vec<crate::irq::IrqAffinity>,
    enabled_by_source: Vec<bool>,
    sources: usize,
}

struct RiscvPlicIrqHandler {
    inner: PlicIrqHandler,
    context_by_cpu: Vec<Option<usize>>,
}

impl RiscvPlicIrqHandler {
    /// 无锁读 source 是否 pending（直接读 PLIC 寄存器，不走 with_plic mutex——
    /// 可在 IRQ path 里安全调用）。
    fn is_pending_raw(&self, source: u32) -> bool {
        NonZeroU32::new(source)
            .map(|s| self.inner.is_pending(s))
            .unwrap_or(false)
    }

    fn current_context(&self) -> Option<usize> {
        current_context(&self.context_by_cpu)
    }

    fn init_context(&self, cpu_idx: usize) {
        if let Some(context) = self.context_by_cpu.get(cpu_idx).and_then(|ctx| *ctx) {
            self.init_context_by_context_id(context);
        } else {
            warn!("PLIC supervisor context for logical CPU {cpu_idx} is not found");
        }
    }

    fn init_context_by_context_id(&self, context: usize) {
        self.inner.init_by_context(context);
        trace!("PLIC context {context} initialized");
    }

    fn reset_all_contexts(&self) {
        for context in self.context_by_cpu.iter().filter_map(|context| *context) {
            self.reset_context_by_context_id(context);
        }
    }

    fn reset_context_by_context_id(&self, context: usize) {
        self.inner.reset_context(context);
        trace!("PLIC context {context} reset");
    }

    fn claim_current(&self) -> Option<NonZeroU32> {
        let Some(context) = self.current_context() else {
            warn_missing_current_context();
            return None;
        };
        let Some(source) = self.inner.claim(context) else {
            debug!("Spurious external IRQ");
            return None;
        };
        Some(source)
    }

    fn complete_current(&self, source: NonZeroU32) {
        let Some(context) = self.current_context() else {
            warn_missing_current_context();
            return;
        };
        self.inner.complete(context, source);
    }
}

impl RiscvPlic {
    fn hwirq_from_source(&self, source: usize) -> Result<rdif_intc::HwIrq, crate::irq::IrqError> {
        riscv_plic_hwirq_from_source(source, self.sources)
    }

    fn source_from_hwirq(&self, hwirq: rdif_intc::HwIrq) -> Result<usize, crate::irq::IrqError> {
        riscv_source_from_plic_hwirq(hwirq, self.sources)
    }

    fn enable_source(&mut self, source: NonZeroU32) -> Result<(), crate::irq::IrqError> {
        if source.get() as usize > self.sources {
            warn!("skip enabling out-of-range PLIC source {}", source.get());
            return Err(crate::irq::IrqError::InvalidIrq);
        }
        self.enabled_by_source[source.get() as usize] = true;
        // 给 source 101(邮箱) 最高优先级 7，避免被 cvsd(source 36, 117/s, priority 1) 饥饿。
        // PLIC 同优先级下选 source ID 最小的——36 < 101，所以 101 必须比 36 高才能被交付。
        let priority = if source.get() == 101 { 7 } else { DEFAULT_PRIORITY };
        self.inner.set_priority(source, priority);
        let current = current_context(&self.context_by_cpu);
        let ctxs = self.contexts_for_source(source);
        info!(
            "cvi-irq: enable_source {} sources_max={} current_ctx={:?} ctxs={:?} prio={}",
            source.get(),
            self.sources,
            current,
            ctxs,
            priority
        );
        for context in ctxs {
            self.inner.enable(source, context);
        }
        if current.is_none() {
            warn_missing_current_context();
        }
        Ok(())
    }

    fn disable_source(&mut self, source: NonZeroU32) -> Result<(), crate::irq::IrqError> {
        if source.get() as usize > self.sources {
            warn!("skip disabling out-of-range PLIC source {}", source.get());
            return Err(crate::irq::IrqError::InvalidIrq);
        }
        self.enabled_by_source[source.get() as usize] = false;
        self.disable_source_contexts(source);
        Ok(())
    }

    fn disable_source_contexts(&mut self, source: NonZeroU32) {
        for context in self.context_by_cpu.iter().filter_map(|context| *context) {
            self.inner.disable(source, context);
        }
    }

    fn set_source_affinity(
        &mut self,
        source: NonZeroU32,
        affinity: crate::irq::IrqAffinity,
    ) -> Option<()> {
        if source.get() as usize > self.sources {
            warn!(
                "skip setting affinity for out-of-range PLIC source {}",
                source.get()
            );
            return None;
        }
        if let crate::irq::IrqAffinity::Fixed { cpu_id } = affinity
            && self
                .context_by_cpu
                .get(cpu_id)
                .and_then(|ctx| *ctx)
                .is_none()
        {
            warn!("PLIC supervisor context for affinity CPU {cpu_id} is not found");
            return None;
        }

        let was_enabled = self.enabled_by_source[source.get() as usize];
        self.disable_source_contexts(source);
        self.affinity_by_source[source.get() as usize] = affinity;
        if was_enabled {
            for context in self.contexts_for_source(source) {
                self.inner.enable(source, context);
            }
        }
        Some(())
    }

    fn contexts_for_source(&self, source: NonZeroU32) -> Vec<usize> {
        match self.affinity_by_source[source.get() as usize] {
            crate::irq::IrqAffinity::Any => {
                self.context_by_cpu.iter().filter_map(|ctx| *ctx).collect()
            }
            crate::irq::IrqAffinity::Fixed { cpu_id } => self
                .context_by_cpu
                .get(cpu_id)
                .and_then(|ctx| *ctx)
                .into_iter()
                .collect(),
        }
    }
}

fn current_context(context_by_cpu: &[Option<usize>]) -> Option<usize> {
    let cpu_idx = crate::cpu::current_cpu_idx()?;
    context_by_cpu.get(cpu_idx).and_then(|ctx| *ctx)
}

fn warn_missing_current_context() {
    if let Some(cpu_idx) = crate::cpu::current_cpu_idx() {
        warn!("PLIC supervisor context for logical CPU {cpu_idx} is not found");
    } else {
        warn!("PLIC supervisor context for current logical CPU is not found");
    }
}

pub fn source_from_hwirq(hwirq: rdif_intc::HwIrq) -> Result<usize, crate::irq::IrqError> {
    with_plic("validating PLIC hardware IRQ", |plic| {
        plic.source_from_hwirq(hwirq)
    })
    .ok_or(crate::irq::IrqError::Controller)?
}

impl DriverGeneric for RiscvPlic {
    fn name(&self) -> &str {
        "RISC-V PLIC"
    }
}

impl Interface for RiscvPlic {
    fn translate_fdt(
        &self,
        irq_prop: &[u32],
    ) -> Result<rdif_intc::ControllerIrqTranslation, rdif_intc::IrqError> {
        let Some(source) = irq_prop.first().copied() else {
            warn!("empty PLIC interrupt specifier");
            return Err(rdif_intc::IrqError::InvalidIrq);
        };
        Ok(rdif_intc::ControllerIrqTranslation::new(
            self.hwirq_from_source(source as usize)?,
        ))
    }

    fn set_enabled(
        &mut self,
        hwirq: rdif_intc::HwIrq,
        enabled: bool,
    ) -> Result<(), rdif_intc::IrqError> {
        let source = NonZeroU32::new(self.source_from_hwirq(hwirq)? as u32)
            .ok_or(rdif_intc::IrqError::InvalidIrq)?;
        if enabled {
            self.enable_source(source)
        } else {
            self.disable_source(source)
        }
    }
}
