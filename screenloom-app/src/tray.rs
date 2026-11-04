//! 系统托盘：图标 + 右键菜单（截图 / 设置 / 退出）。
//!
//! 托盘图标直接取 **exe 自己的 PE 资源**（`build.rs` 注入的 `screenloom.ico`），
//! 所以托盘里看到的图标和资源管理器里 exe 的图标**永远是同一个**。
//! 资源缺失时（构建时没找到 `rc.exe`）退化为系统应用图标。

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    NOTIFY_ICON_DATA_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
    GetClassInfoExW, GetCursorPos, LoadIconW, LoadImageW, RegisterClassExW, SetForegroundWindow,
    TrackPopupMenu, HICON, HWND_MESSAGE, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE, MF_SEPARATOR,
    MF_STRING, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_COMMAND, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NCCREATE, WM_RBUTTONUP,
    WNDCLASSEXW,
};

use screenloom_core::{Error, Result};

const CLASS_NAME: &str = "ScreenloomTrayWindow";

/// 托盘菜单命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    /// 立即截图
    Capture,
    /// 打开设置窗口
    Settings,
    /// 选择要从截图中排除的窗口
    ExcludeWindows,
    /// 开始录屏
    RecordStart,
    /// 录制一个窗口
    RecordWindow,
    /// 停止录屏
    RecordStop,
    /// 暂停/继续录屏
    RecordTogglePause,
    /// 打开录像文件夹
    OpenRecordFolder,
    /// 打开录屏设置
    RecordSettings,
    /// 退出程序
    Exit,
}

const MENU_CAPTURE: usize = 2001;
const MENU_SETTINGS: usize = 2002;
const MENU_EXCLUDE: usize = 2003;
const MENU_RECORD_START: usize = 2004;
const MENU_RECORD_WINDOW: usize = 2010;
const MENU_RECORD_STOP: usize = 2005;
const MENU_RECORD_PAUSE: usize = 2006;
const MENU_RECORD_FOLDER: usize = 2007;
const MENU_RECORD_SETTINGS: usize = 2008;
const MENU_EXIT: usize = 2009;

/// bit0 = 录制中，bit1 = 已暂停。
static RECORD_STATE: AtomicU8 = AtomicU8::new(0);

/// 供录制控制器更新托盘菜单文案。
pub fn set_record_state(recording: bool, paused: bool) {
    let v = (recording as u8) | ((paused as u8) << 1);
    RECORD_STATE.store(v, Ordering::Relaxed);
}

fn record_state() -> (bool, bool) {
    let v = RECORD_STATE.load(Ordering::Relaxed);
    (v & 1 != 0, v & 2 != 0)
}

/// 托盘回调消息（Shell 发给托盘窗口）。
pub const WM_TRAY: u32 = WM_APP + 20;

/// 托盘图标 ID。
const TRAY_ID: u32 = 1;

/// 托盘菜单命令的回调类型。
type TrayHandler = Box<dyn Fn(TrayCommand)>;

thread_local! {
    /// 菜单命令回调（托盘窗口过程与 Tray 实例在同一线程）。
    static HANDLER: RefCell<Option<TrayHandler>> = const { RefCell::new(None) };
}

fn fire(cmd: TrayCommand) {
    HANDLER.with(|h| {
        if let Some(f) = h.borrow().as_ref() {
            f(cmd);
        }
    });
}

/// UTF-16（带结尾 NUL）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 系统托盘图标。
pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    /// 图标是我们自己加载的（需要 DestroyIcon）；系统共享的不用。
    owned_icon: bool,
    nid: NOTIFYICONDATAW,
}

impl Tray {
    /// 创建托盘图标。
    ///
    /// `on_command` 在用户点菜单/左键图标时于**本线程**被调用。
    pub fn new<F>(on_command: F) -> Result<Tray>
    where
        F: Fn(TrayCommand) + 'static,
    {
        HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(on_command)));

        register_class()?;

        let class = wide(CLASS_NAME);
        let title = wide("screenloom");
        // 消息专用窗口：不显示、不出现在任务栏
        let hwnd = unsafe {
            windows::Win32::UI::WindowsAndMessaging::CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(module_handle()),
                None,
            )
        }
        .map_err(|e| Error::Window(format!("创建托盘窗口失败: {e}")))?;

        let (icon, owned_icon) = load_app_icon().unwrap_or_else(|| {
            // 退化：系统应用图标
            let h = unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default();
            (h, false)
        });

        let mut tip = [0u16; 128];
        for (i, c) in "screenloom — 微信截图工具".encode_utf16().take(127).enumerate() {
            tip[i] = c;
        }

        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NOTIFY_ICON_DATA_FLAGS(NIF_MESSAGE.0 | NIF_ICON.0 | NIF_TIP.0),
            uCallbackMessage: WM_TRAY,
            hIcon: icon,
            szTip: tip,
            ..Default::default()
        };

        let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool();
        if !ok {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(Error::Window("Shell_NotifyIconW(NIM_ADD) 失败".into()));
        }

        Ok(Tray {
            hwnd,
            icon,
            owned_icon,
            nid,
        })
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid);
            let _ = DestroyWindow(self.hwnd);
            if self.owned_icon {
                let _ = DestroyIcon(self.icon);
            }
        }
        HANDLER.with(|h| *h.borrow_mut() = None);
    }
}

/// 从 exe 的 PE 资源里取应用图标（资源 ID 1，由 `build.rs` 注入）。
fn load_app_icon() -> Option<(HICON, bool)> {
    unsafe {
        // MAKEINTRESOURCEW(1)
        let name = PCWSTR(1usize as *const u16);
        // hinst 必须给**本模块**的实例句柄：给 None 会去系统资源里找 ID 1，
        // 那是找不到的（会退化成 IDI_APPLICATION，托盘里就不是我们的图标了）。
        LoadImageW(
            Some(module_handle()),
            name,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE,
        )
        .ok()
        .map(|h| (HICON(h.0), true))
    }
}

fn register_class() -> Result<()> {
    let name = wide(CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(tray_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        ..WNDCLASSEXW::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册托盘窗口类失败".into()));
    }
    Ok(())
}

fn module_handle() -> HINSTANCE {
    HINSTANCE(
        unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
            .unwrap_or_default()
            .0,
    )
}

/// 弹出托盘右键菜单。
unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    let _ = AppendMenuW(menu, MF_STRING, MENU_CAPTURE, PCWSTR(wide("截图").as_ptr()));
    let (recording, paused) = record_state();
    if recording {
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            MENU_RECORD_STOP,
            PCWSTR(wide("停止录屏").as_ptr()),
        );
        let label = if paused { "继续录屏" } else { "暂停录屏" };
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            MENU_RECORD_PAUSE,
            PCWSTR(wide(label).as_ptr()),
        );
    } else {
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            MENU_RECORD_START,
            PCWSTR(wide("开始录屏").as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            MENU_RECORD_WINDOW,
            PCWSTR(wide("录制窗口…").as_ptr()),
        );
    }
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_RECORD_FOLDER,
        PCWSTR(wide("打开录像文件夹").as_ptr()),
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_RECORD_SETTINGS,
        PCWSTR(wide("录屏设置…").as_ptr()),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_EXCLUDE,
        PCWSTR(wide("排除窗口…").as_ptr()),
    );
    let _ = AppendMenuW(
        menu,
        MF_STRING,
        MENU_SETTINGS,
        PCWSTR(wide("设置…").as_ptr()),
    );
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, PCWSTR(wide("退出").as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // 菜单弹出前必须把焦点抢回来，否则点外面菜单不会消失
    let _ = SetForegroundWindow(hwnd);

    // TPM_RETURNCMD：直接返回被选中的命令 ID（不发 WM_COMMAND）
    let cmd = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
        pt.x,
        pt.y,
        None,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);

    match cmd.0 as usize {
        MENU_CAPTURE => fire(TrayCommand::Capture),
        MENU_EXCLUDE => fire(TrayCommand::ExcludeWindows),
        MENU_RECORD_START => fire(TrayCommand::RecordStart),
        MENU_RECORD_WINDOW => fire(TrayCommand::RecordWindow),
        MENU_RECORD_STOP => fire(TrayCommand::RecordStop),
        MENU_RECORD_PAUSE => fire(TrayCommand::RecordTogglePause),
        MENU_RECORD_FOLDER => fire(TrayCommand::OpenRecordFolder),
        MENU_RECORD_SETTINGS => fire(TrayCommand::RecordSettings),
        MENU_SETTINGS => fire(TrayCommand::Settings),
        MENU_EXIT => fire(TrayCommand::Exit),
        _ => {}
    }
}

unsafe extern "system" fn tray_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            // 未设置 NOTIFYICON_VERSION_4 时，lParam 就是鼠标事件
            match lp.0 as u32 {
                WM_LBUTTONUP => fire(TrayCommand::Capture),
                WM_LBUTTONDBLCLK => fire(TrayCommand::Capture),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_COMMAND => {
            let id = (wp.0 as u32 & 0xFFFF) as usize;
            match id {
                MENU_CAPTURE => fire(TrayCommand::Capture),
                MENU_EXCLUDE => fire(TrayCommand::ExcludeWindows),
                MENU_RECORD_START => fire(TrayCommand::RecordStart),
                MENU_RECORD_WINDOW => fire(TrayCommand::RecordWindow),
                MENU_RECORD_STOP => fire(TrayCommand::RecordStop),
                MENU_RECORD_PAUSE => fire(TrayCommand::RecordTogglePause),
                MENU_RECORD_FOLDER => fire(TrayCommand::OpenRecordFolder),
                MENU_RECORD_SETTINGS => fire(TrayCommand::RecordSettings),
                MENU_SETTINGS => fire(TrayCommand::Settings),
                MENU_EXIT => fire(TrayCommand::Exit),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_NCCREATE => {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
