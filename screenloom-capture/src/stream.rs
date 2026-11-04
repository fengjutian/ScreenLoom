//! 持续抓屏帧源（录屏用）。
//!
//! # 与 [`WgcBackend::capture`] 的区别
//!
//! `WgcBackend::capture` 是「抓一帧就停」——建帧池、收一帧、拆掉。
//! 录屏需要的是**帧池常驻**：一直有帧进来，只保留**最新**的那帧，
//! 中间积压的旧帧直接丢掉（画面追不上时宁可丢帧也不能让视频变成慢动作）。
//!
//! # 线程模型
//!
//! 帧池用 `CreateFreeThreaded` 创建，`FrameArrived` 事件在**线程池线程**上触发，
//! 所以本类型**不是** `Send` 约束之外的自由对象，但可以整体搬到另一个线程使用
//! （前提是那个线程自己进了 COM 套间）。
//!
//! `WgcBackend` 内部持有 COM 套间守卫，本结构体把它放在**最后一个字段**，
//! 保证析构时帧池/会话（WinRT 对象）先于套间释放。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows::core::IInspectable;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
    GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;

use screenloom_core::{Error, Result};

use crate::monitors::{list_monitors, monitor_handles};
use crate::wgc::{create_item_for_monitor, create_item_for_window, WgcBackend};
use crate::CaptureTarget;

/// 帧池缓冲数量。比截图多一个，给编码线程留出读回缓冲。
const FRAME_BUFFER_COUNT: i32 = 3;

/// 一帧 BGRA 像素（内存序 **B, G, R, A**，与 WGC 原生格式一致）。
#[derive(Clone, Debug)]
pub struct BgraFrame {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// 持续抓屏帧源。
pub struct ScreenStream {
    // ↓ 声明顺序 = 析构顺序：WinRT 对象必须先于 backend（内含套间守卫）释放。
    pool: Direct3D11CaptureFramePool,
    /// 捕获会话。必须**保持存活**，否则捕获会停；用 `_` 前缀表示「只为持有」。
    _session: GraphicsCaptureSession,
    _handler: TypedEventHandler<Direct3D11CaptureFramePool, IInspectable>,
    /// 有新帧到达时置位。
    dirty: Arc<AtomicBool>,
    size: SizeInt32,
    backend: WgcBackend,
}

impl ScreenStream {
    /// 建立持续捕获会话（不录鼠标指针）。
    ///
    /// `target` 与 [`crate::default_backend`] 的语义一致：按显示器下标或窗口句柄。
    ///
    /// **调用线程必须自己进入 COM MTA 套间**（[`WgcBackend::new`] 内部会进入并持有守卫）。
    pub fn start(target: CaptureTarget) -> Result<ScreenStream> {
        Self::start_with_cursor(target, false)
    }

    /// 同 [`ScreenStream::start`]，可指定是否把鼠标指针合成进帧。
    ///
    /// 注意：只有 WGC 路径有效；GDI 兜底路径拿不到硬件光标。
    pub fn start_with_cursor(target: CaptureTarget, cursor: bool) -> Result<ScreenStream> {
        let backend = WgcBackend::new()?;
        let item = create_item(target)?;
        let size = item
            .Size()
            .map_err(|e| Error::Capture(format!("获取捕获目标尺寸失败: {e}")))?;

        // NV12 的色度是 2×2 降采样，宽高必须为偶数。
        if size.Width % 2 != 0 || size.Height % 2 != 0 {
            return Err(Error::Capture(format!(
                "录制目标尺寸 {width}x{height} 不是偶数，无法转成 NV12",
                width = size.Width,
                height = size.Height
            )));
        }

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &backend.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            FRAME_BUFFER_COUNT,
            size,
        )
        .map_err(|e| Error::Capture(format!("创建 WGC 帧池失败: {e}")))?;

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| Error::Capture(format!("创建 WGC 捕获会话失败: {e}")))?;
        // 老系统不支持这些属性，忽略失败即可。
        let _ = session.SetIsCursorCaptureEnabled(cursor);
        let _ = session.SetIsBorderRequired(false);

        let dirty = Arc::new(AtomicBool::new(false));
        let flag = dirty.clone();
        let handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
            move |_sender, _args| {
                flag.store(true, Ordering::Release);
                Ok(())
            },
        );
        pool.FrameArrived(&handler)
            .map_err(|e| Error::Capture(format!("注册 WGC 帧事件失败: {e}")))?;
        session
            .StartCapture()
            .map_err(|e| Error::Capture(format!("启动 WGC 捕获失败: {e}")))?;

        Ok(ScreenStream {
            pool,
            _session: session,
            _handler: handler,
            dirty,
            size,
            backend,
        })
    }

    /// 帧尺寸（物理像素）。
    pub fn size(&self) -> (u32, u32) {
        (self.size.Width as u32, self.size.Height as u32)
    }

    /// 取**最新**一帧；积压的旧帧直接丢弃。
    ///
    /// - `Ok(None)`：自上次调用以来没有新帧（画面没变）。
    /// - `Err`：目标尺寸变了 —— 帧池已失效，必须重建（上层应结束当前录制）。
    pub fn take_latest(&mut self) -> Result<Option<BgraFrame>> {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return Ok(None);
        }

        // 连续取帧，只保留最后一帧（旧的立刻 Close，否则池里塞不下）。
        let mut latest: Option<Direct3D11CaptureFrame> = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            if let Some(old) = latest {
                let _ = old.Close();
            }
            latest = Some(frame);
        }
        let Some(frame) = latest else {
            return Ok(None);
        };

        // 尺寸变了 → 帧池失效（wgc.rs 里单帧捕获路径会重建帧池重试）。
        if let Ok(cur) = frame.ContentSize() {
            if cur.Width != self.size.Width || cur.Height != self.size.Height {
                let _ = frame.Close();
                return Err(Error::Capture(format!(
                    "录制过程中目标尺寸从 {}x{} 变成了 {}x{}，需要重新开始录制",
                    self.size.Width, self.size.Height, cur.Width, cur.Height
                )));
            }
        }

        let read = self.backend.read_frame_bgra(&frame);
        let _ = frame.Close();
        let (w, h, data) = read?;
        Ok(Some(BgraFrame { w, h, data }))
    }
}

/// 由捕获目标创建 WGC 捕获项。
fn create_item(target: CaptureTarget) -> Result<GraphicsCaptureItem> {
    match target {
        CaptureTarget::Monitor(index) => {
            let monitors = list_monitors()?;
            if index >= monitors.len() {
                return Err(Error::Capture(format!(
                    "显示器下标越界: {index}（本机共 {} 个显示器）",
                    monitors.len()
                )));
            }
            let handles = monitor_handles()?;
            let hmon = handles.get(index).copied().ok_or_else(|| {
                Error::Capture(format!("显示器下标越界: {index}（句柄列表为空）"))
            })?;
            create_item_for_monitor(hmon)
                .map_err(|e| Error::Capture(format!("创建显示器捕获项失败: {e}")))
        }
        CaptureTarget::Window(hwnd) => create_item_for_window(hwnd)
            .map_err(|e| Error::Capture(format!("创建窗口捕获项失败: {e}"))),
    }
}
