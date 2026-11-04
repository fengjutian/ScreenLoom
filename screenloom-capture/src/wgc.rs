//! WGC（`Windows.Graphics.Capture`）主后端。
//!
//! 关键点：
//! - 帧是 **BGRA** 排列，必须经 [`RgbaImage::from_bgra`] 转成 RGBA（搞反会整体发蓝/发红）。
//! - 帧是 GPU 纹理，必须 `CopyResource` 到 `D3D11_USAGE_STAGING` 纹理再 `Map` 读回，
//!   不能直接访问纹理内存。
//! - 捕获线程需要 COM/WinRT 多线程套间（`CoInitializeEx(COINIT_MULTITHREADED)`），
//!   且 **套间必须活得比 WinRT 对象久** —— 否则拿已卸载套间的对象去调用会直接访问违例。
//! - 目标尺寸变化会让帧池失效，需要捕获后重建帧池。

use std::ptr;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use screenloom_core::{Error, Result, RgbaImage};
use windows::core::{IInspectable, Interface};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
    GraphicsCaptureSession, IGraphicsCaptureSessionStatics,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

use crate::errors::{capture_error, is_api_unavailable, is_size_changed};
use crate::monitors::{list_monitors, monitor_handles, window_rect_physical};
use crate::{CaptureBackend, CaptureFrame, CaptureTarget};

/// 帧池缓冲数量。
const FRAME_BUFFER_COUNT: i32 = 2;
/// 等待 WGC 帧的最长时间。
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
/// 目标尺寸变化时重建帧池的最大次数。
const MAX_SIZE_RETRIES: u32 = 4;

/// WGC 抓屏后端。
///
/// 字段声明顺序 = 析构顺序：`winrt_device`（WinRT 对象）必须先于 `_apartment` 释放，
/// 这样它析构时套间仍然有效。
pub struct WgcBackend {
    /// D3D11 设备。`pub(crate)` 是给同 crate 的 [`crate::stream`]（录屏帧源）用的。
    pub(crate) device: ID3D11Device,
    /// D3D11 上下文。同上。
    pub(crate) context: ID3D11DeviceContext,
    /// WinRT 设备（帧池要用它创建）。`pub(crate)` 给同 crate 的 [`crate::stream`] 用。
    pub(crate) winrt_device: IDirect3DDevice,
    /// 套间守卫：随本结构体一起存活，保证 WinRT 对象的生命周期落在套间之内。
    _apartment: ComApartment,
}

impl WgcBackend {
    /// 初始化 D3D11 设备与 WinRT 设备。
    ///
    /// 失败（例如系统不支持 WGC、没有可用的 D3D11 硬件设备）时返回错误，
    /// 调用方可以退回 [`GdiBackend`](crate::GdiBackend)。
    pub fn new() -> Result<Self> {
        // 套间要先于任何 WinRT 对象建立，并一直活到本结构体析构。
        let apartment = ComApartment::enter();

        match wgc_supported() {
            Ok(true) => {}
            Ok(false) => {
                return Err(Error::Capture(
                    "当前系统不支持 Windows.Graphics.Capture".into(),
                ))
            }
            Err(e) => return Err(Error::Capture(format!("查询 WGC 支持状态失败: {e}"))),
        }

        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        // SAFETY: 输出参数都是合法的 Option<&mut _>。
        unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE(ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| Error::Capture(format!("D3D11CreateDevice 失败: {e}")))?;

        let device = device.ok_or_else(|| Error::Capture("D3D11CreateDevice 未返回设备".into()))?;
        let context =
            context.ok_or_else(|| Error::Capture("D3D11CreateDevice 未返回上下文".into()))?;

        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|e| Error::Capture(format!("获取 IDXGIDevice 失败: {e}")))?;
        // SAFETY: dxgi 是有效的 IDXGIDevice。
        let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .map_err(|e| Error::Capture(format!("CreateDirect3D11DeviceFromDXGIDevice 失败: {e}")))?
            .cast()
            .map_err(|e| Error::Capture(format!("转换为 IDirect3DDevice 失败: {e}")))?;

        Ok(Self {
            device,
            context,
            winrt_device: winrt,
            _apartment: apartment,
        })
    }

    /// 捕获显示器。
    fn capture_monitor(&self, index: usize) -> Result<CaptureFrame> {
        let monitors = list_monitors()?;
        let info = monitors.get(index).ok_or_else(|| {
            Error::Capture(format!(
                "显示器下标越界: {index}（本机共 {} 个显示器）",
                monitors.len()
            ))
        })?;
        let handles = monitor_handles()?;
        let hmon = *handles
            .get(index)
            .ok_or_else(|| Error::Capture(format!("显示器下标越界: {index}（句柄列表为空）")))?;

        match create_item_for_monitor(hmon) {
            Ok(item) => {
                let image = self.capture_item(&item)?;
                Ok(CaptureFrame {
                    image,
                    bounds: info.bounds,
                })
            }
            Err(e) if is_api_unavailable(e.code().0) => {
                // 该系统不支持 `CreateForMonitor`（需要 Win10 2004+）→ 显示器降级到 GDI。
                crate::gdi::capture_monitor(index)
            }
            Err(e) => Err(capture_error("创建显示器捕获项失败", &e)),
        }
    }

    /// 捕获窗口。
    fn capture_window(&self, hwnd: HWND) -> Result<CaptureFrame> {
        let bounds = window_rect_physical(hwnd)?;
        let item =
            create_item_for_window(hwnd).map_err(|e| capture_error("创建窗口捕获项失败", &e))?;
        let image = self.capture_item(&item)?;
        Ok(CaptureFrame { image, bounds })
    }

    /// 抓一帧；目标尺寸变化时重建帧池重试。
    fn capture_item(&self, item: &GraphicsCaptureItem) -> Result<RgbaImage> {
        let mut last: Option<Error> = None;
        for _ in 0..MAX_SIZE_RETRIES {
            let size = item_size(item)?;
            match self.capture_once(item, size) {
                Ok(image) => return Ok(image),
                Err(CaptureError::SizeChanged) => {
                    last = Some(Error::Capture("捕获目标尺寸已变化，正在重建帧池".into()));
                    continue;
                }
                Err(CaptureError::Fatal(e)) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| Error::Capture("捕获失败: 目标尺寸持续变化".into())))
    }

    /// 建立帧池 + 会话，等一帧并读回像素。
    fn capture_once(
        &self,
        item: &GraphicsCaptureItem,
        size: SizeInt32,
    ) -> std::result::Result<RgbaImage, CaptureError> {
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &self.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            FRAME_BUFFER_COUNT,
            size,
        )
        .map_err(|e| CaptureError::Fatal(capture_error("创建 WGC 帧池失败", &e)))?;

        // 会话由帧池创建（windows 0.61 的 `GraphicsCaptureItem` 上没有这个方法）。
        let session = pool
            .CreateCaptureSession(item)
            .map_err(|e| CaptureError::Fatal(capture_error("创建 WGC 捕获会话失败", &e)))?;
        // 老系统不支持这些属性，忽略失败即可。
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);

        let (tx, rx) = mpsc::channel::<()>();
        let handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
            move |_sender, _args| {
                let _ = tx.send(());
                Ok(())
            },
        );
        pool.FrameArrived(&handler)
            .map_err(|e| CaptureError::Fatal(capture_error("注册 WGC 帧事件失败", &e)))?;
        session
            .StartCapture()
            .map_err(|e| CaptureError::Fatal(capture_error("启动 WGC 捕获失败", &e)))?;

        let deadline = Instant::now() + CAPTURE_TIMEOUT;
        let outcome: std::result::Result<RgbaImage, CaptureError> = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break Err(CaptureError::Fatal(Error::Capture(format!(
                    "捕获超时: {CAPTURE_TIMEOUT:?} 内未收到 WGC 帧"
                ))));
            }
            match rx.recv_timeout(remaining) {
                Ok(()) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    break Err(CaptureError::Fatal(Error::Capture(
                        "捕获超时: 等待 WGC 帧超时".into(),
                    )));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err(CaptureError::Fatal(Error::Capture(
                        "捕获失败: WGC 帧事件通道已断开".into(),
                    )));
                }
            }

            match pool.TryGetNextFrame() {
                Ok(frame) => {
                    // 帧到手后确认尺寸没变（变了说明目标尺寸变了，需要重建帧池）。
                    if let Ok(cur) = frame.ContentSize() {
                        if cur.Width != size.Width || cur.Height != size.Height {
                            let _ = frame.Close();
                            break Err(CaptureError::SizeChanged);
                        }
                    }
                    let read = self.read_frame(&frame);
                    let _ = frame.Close();
                    break match read {
                        Ok(image) => Ok(image),
                        Err(e) => Err(CaptureError::Fatal(e)),
                    };
                }
                Err(e) => {
                    let code = e.code().0;
                    if is_size_changed(code) {
                        break Err(CaptureError::SizeChanged);
                    }
                    if code == 0 {
                        // 暂时没有可用帧，继续等。
                        continue;
                    }
                    break Err(CaptureError::Fatal(capture_error("读取 WGC 帧失败", &e)));
                }
            }
        };

        let _ = session.Close();
        let _ = pool.Close();

        outcome
    }

    /// 把 GPU 帧纹理拷到 staging 纹理并读回像素（**原始 BGRA**，不做通道转换）。
    ///
    /// 录屏路径需要 BGRA（编码器要 NV12，中间不该多一次 BGRA→RGBA 转换），
    /// 所以这里只做「GPU→CPU 读回」，通道转换交给调用方。
    pub(crate) fn read_frame_bgra(
        &self,
        frame: &Direct3D11CaptureFrame,
    ) -> Result<(u32, u32, Vec<u8>)> {
        // SAFETY: 以下全部是对 frame 内部 COM 对象的合法调用。
        unsafe {
            let surface = frame
                .Surface()
                .map_err(|e| Error::Capture(format!("获取 WGC 帧表面失败: {e}")))?;
            let access: IDirect3DDxgiInterfaceAccess = surface
                .cast()
                .map_err(|e| Error::Capture(format!("获取帧表面 DXGI 接口失败: {e}")))?;
            let src: ID3D11Texture2D = access
                .GetInterface()
                .map_err(|e| Error::Capture(format!("获取 WGC 帧纹理失败: {e}")))?;

            let mut desc = D3D11_TEXTURE2D_DESC::default();
            src.GetDesc(&mut desc);
            let w = desc.Width;
            let h = desc.Height;
            if w == 0 || h == 0 {
                return Err(Error::Capture("WGC 帧尺寸非法".into()));
            }

            // staging 纹理：CPU 可读，格式/尺寸与源纹理一致。
            let mut sdesc = desc;
            sdesc.Usage = D3D11_USAGE_STAGING;
            sdesc.BindFlags = 0;
            sdesc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            sdesc.MiscFlags = 0;
            sdesc.MipLevels = 1;
            sdesc.ArraySize = 1;

            let mut staging: Option<ID3D11Texture2D> = None;
            self.device
                .CreateTexture2D(&sdesc, None, Some(&mut staging))
                .map_err(|e| Error::Capture(format!("创建 staging 纹理失败: {e}")))?;
            let staging = staging.ok_or_else(|| Error::Capture("创建 staging 纹理失败".into()))?;

            self.context.CopyResource(&staging, &src);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| Error::Capture(format!("映射 staging 纹理失败: {e}")))?;

            let row_pitch = mapped.RowPitch as usize;
            let bytes_per_row = w as usize * 4;
            let mut data = vec![0u8; bytes_per_row * h as usize];
            let base = mapped.pData as *const u8;
            for y in 0..h as usize {
                ptr::copy_nonoverlapping(
                    base.add(y * row_pitch),
                    data.as_mut_ptr().add(y * bytes_per_row),
                    bytes_per_row,
                );
            }
            self.context.Unmap(&staging, 0);

            Ok((w, h, data))
        }
    }

    /// 把 GPU 帧纹理拷到 staging 纹理并读回像素。
    fn read_frame(&self, frame: &Direct3D11CaptureFrame) -> Result<RgbaImage> {
        let (w, h, data) = self.read_frame_bgra(frame)?;
        // WGC 帧是 BGRA，交给 from_bgra 做通道转换。
        Ok(RgbaImage::from_bgra(w, h, &data))
    }
}

impl CaptureBackend for WgcBackend {
    fn capture(&self, target: CaptureTarget) -> Result<CaptureFrame> {
        // 捕获线程需要处于 MTA 套间（外层套间由 self._apartment 维持）。
        let _apartment = ComApartment::enter();
        match target {
            CaptureTarget::Monitor(index) => self.capture_monitor(index),
            CaptureTarget::Window(hwnd) => self.capture_window(hwnd),
        }
    }
}

/// 一次捕获尝试的失败原因。
enum CaptureError {
    /// 目标尺寸变了，需要重建帧池。
    SizeChanged,
    Fatal(Error),
}

/// 捕获项当前尺寸。
fn item_size(item: &GraphicsCaptureItem) -> Result<SizeInt32> {
    item.Size()
        .map_err(|e| Error::Capture(format!("获取捕获目标尺寸失败: {e}")))
}

/// 查询系统是否支持 WGC。
///
/// **不能用 `GraphicsCaptureSession::IsSupported()`**：windows-rs 的静态 `FactoryCache`
/// 会缓存一个带 **套间亲和性** 的工厂指针，套间卸载（`RoUninitialize`）之后再次
/// 调用会直接访问违例（0xC0000005）。这里绕过缓存：现取工厂、用完即释放。
fn wgc_supported() -> windows::core::Result<bool> {
    let statics: IGraphicsCaptureSessionStatics =
        windows::core::factory::<GraphicsCaptureSession, IGraphicsCaptureSessionStatics>()?;
    let mut supported = false;
    // SAFETY: statics 是有效的工厂接口，vtable 槽位与 WinRT 元数据一致。
    unsafe {
        (Interface::vtable(&statics).IsSupported)(Interface::as_raw(&statics), &mut supported)
    }
    .ok()?;
    Ok(supported)
}

/// 由窗口句柄创建捕获项。
pub(crate) fn create_item_for_window(hwnd: HWND) -> windows::core::Result<GraphicsCaptureItem> {
    let interop = capture_item_interop()?;
    // SAFETY: hwnd 已校验存在性。
    unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(hwnd) }
}

/// 由显示器句柄创建捕获项（需要 Win10 2004+）。
pub(crate) fn create_item_for_monitor(
    hmon: windows::Win32::Graphics::Gdi::HMONITOR,
) -> windows::core::Result<GraphicsCaptureItem> {
    let interop = capture_item_interop()?;
    // SAFETY: hmon 来自 EnumDisplayMonitors，是有效句柄。
    unsafe { interop.CreateForMonitor::<GraphicsCaptureItem>(hmon) }
}

/// 获取 `IGraphicsCaptureItemInterop` 激活工厂。
fn capture_item_interop() -> windows::core::Result<IGraphicsCaptureItemInterop> {
    windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
}

/// COM / WinRT 套间守卫。
///
/// 进入时确保当前线程是 MTA（`CoInitializeEx(COINIT_MULTITHREADED)`），
/// 离开时配对反初始化。若线程本来已经是 STA，则不改动（WinRT 已按 STA 初始化）。
///
/// **必须让它活得比在套间里创建的 WinRT 对象更久**，否则析构 WinRT 对象时套间已经没了。
pub(crate) struct ComApartment {
    com: bool,
    winrt: bool,
}

impl ComApartment {
    fn enter() -> Self {
        const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;
        let mut com = false;
        let mut winrt = false;
        // SAFETY: CoInitializeEx / RoInitialize 在当前线程上配对调用。
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.0 != RPC_E_CHANGED_MODE {
                // S_OK 与 S_FALSE 都需要配对一次 CoUninitialize。
                com = true;
                // RoInitialize 返回 S_FALSE（已是同模型）时同样要配对 RoUninitialize。
                if RoInitialize(RO_INIT_MULTITHREADED).is_ok() {
                    winrt = true;
                }
            }
        }
        Self { com, winrt }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: 与 enter() 配对。
        unsafe {
            if self.winrt {
                RoUninitialize();
            }
            if self.com {
                CoUninitialize();
            }
        }
    }
}
