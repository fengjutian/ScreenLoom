//! 常驻态录制控制器：把热键/托盘/HUD 的命令收敛到一个地方。
//!
//! 只允许一个活动 session；事件通过隐藏宿主窗口的消息队列回到 UI 线程，
//! worker 绝不直接操作 HUD/tray。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use screenloom_core::{Config, Error, Result};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::hud::RecorderHud;
use super::stats::SharedStats;
use super::{RecordEvent, RecordOptions, RecordSession, StopReason};

/// HUD → controller 的命令消息。
pub const WM_RECORDER_HUD: u32 = WM_APP + 31;
/// worker → controller 的事件消息。
pub const WM_RECORDER_EVENT: u32 = WM_APP + 32;

pub const HUD_CMD_PAUSE: usize = 1;
pub const HUD_CMD_STOP: usize = 2;

const CLASS_NAME: &str = "ScreenloomRecorderHost";

struct Controller {
    hwnd: HWND,
    cfg: Config,
    cfg_path: PathBuf,
    session: Option<RecordSession>,
    hud: Option<RecorderHud>,
    stats: Option<Arc<SharedStats>>,
    last_outcome: Option<super::RecordOutcome>,
}

/// 对外句柄。
pub struct RecorderController {
    hwnd: HWND,
}

impl RecorderController {
    /// 创建隐藏宿主窗口。必须在 UI 线程调用（窗口消息循环所在线程）。
    pub fn new(cfg: Config, cfg_path: &Path) -> Result<Self> {
        register_class()?;
        let class = wide(CLASS_NAME);
        let title = wide("screenloom recorder");
        let hwnd = unsafe {
            CreateWindowExW(
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
        .map_err(|e| Error::Window(format!("创建录制宿主窗口失败: {e}")))?;

        let ctl = Box::new(Controller {
            hwnd,
            cfg,
            cfg_path: cfg_path.to_path_buf(),
            session: None,
            hud: None,
            stats: None,
            last_outcome: None,
        });
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(ctl) as isize);
        }
        Ok(RecorderController { hwnd })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn toggle(&self) {
        with_controller(self.hwnd, |c| {
            if c.session.is_some() {
                c.stop(StopReason::User);
            } else if let Err(e) = c.start() {
                eprintln!("录屏启动失败: {e}");
                message_box(&format!("录屏启动失败:\n{e}"));
            }
        });
    }

    pub fn start(&self) {
        self.start_target(None);
    }

    /// 从托盘/热键启动，可覆盖录制目标。
    pub fn start_target(&self, target: Option<super::RecordTarget>) {
        with_controller(self.hwnd, |c| {
            if let Err(e) = c.start_with(target) {
                eprintln!("录屏启动失败: {e}");
                message_box(&format!("录屏启动失败:\n{e}"));
            }
        });
    }

    pub fn start_window(&self, hwnd: isize) {
        self.start_target(Some(super::RecordTarget::Window(hwnd)));
    }

    pub fn stop(&self) {
        with_controller(self.hwnd, |c| c.stop(StopReason::User));
    }

    pub fn toggle_pause(&self) {
        with_controller(self.hwnd, |c| c.toggle_pause());
    }

    pub fn is_recording(&self) -> bool {
        with_controller(self.hwnd, |c| c.session.is_some()).unwrap_or(false)
    }

    pub fn is_paused(&self) -> bool {
        with_controller(self.hwnd, |c| c.session.as_ref().is_some_and(|s| s.is_paused()))
            .unwrap_or(false)
    }

    pub fn open_last_folder(&self) {
        with_controller(self.hwnd, |c| {
            let path = c
                .last_outcome
                .as_ref()
                .map(|o| o.path.clone())
                .or_else(|| c.session.as_ref().map(|s| s.path().clone()));
            if let Some(p) = path {
                open_folder(&p);
            }
        });
    }
}

impl Drop for RecorderController {
    fn drop(&mut self) {
        with_controller(self.hwnd, |c| c.stop(StopReason::AppExit));
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

impl Controller {
    fn start(&mut self) -> Result<()> {
        self.start_with(None)
    }

    fn start_with(&mut self, target: Option<super::RecordTarget>) -> Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let mut opts = RecordOptions::from_config(&self.cfg)?;
        if let Some(t) = target {
            opts.target = t;
        }
        let stats = Arc::new(SharedStats::new());
        let hwnd_raw = self.hwnd.0 as isize;
        let session = RecordSession::start(&opts, Arc::clone(&stats), move |event| {
            let raw = Box::into_raw(Box::new(event)) as isize;
            let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_RECORDER_EVENT, WPARAM(0), LPARAM(raw));
            }
        })?;
        if self.cfg.record_show_hud {
            match RecorderHud::show(Arc::clone(&stats), self.hwnd) {
                Ok(hud) => self.hud = Some(hud),
                Err(e) => {
                    session.stop(StopReason::EncoderError);
                    let _ = session.join();
                    return Err(e);
                }
            }
        }
        self.stats = Some(stats);
        self.session = Some(session);
        crate::tray::set_record_state(true, false);
        println!("录屏已开始（{}）", self.cfg.record_hotkey);
        Ok(())
    }

    fn stop(&mut self, reason: StopReason) {
        if let Some(s) = self.session.as_ref() {
            s.stop(reason);
        }
    }

    fn toggle_pause(&mut self) {
        let Some(s) = self.session.as_ref() else {
            return;
        };
        if s.is_paused() {
            s.resume();
            crate::tray::set_record_state(true, false);
        } else {
            s.pause();
            crate::tray::set_record_state(true, true);
        }
    }

    fn on_hud(&mut self, cmd: usize) {
        match cmd {
            HUD_CMD_PAUSE => self.toggle_pause(),
            HUD_CMD_STOP => self.stop(StopReason::User),
            _ => {}
        }
    }

    fn on_event(&mut self, event: RecordEvent) {
        // 先让 HUD 消失，再处理收尾结果。
        self.hud.take();
        let reason = match &event {
            RecordEvent::Finished(_) => None,
            RecordEvent::Failed(_) => None,
            RecordEvent::Discarded(r) => Some(*r),
        };
        if let Some(s) = self.session.take() {
            let _ = s.join();
        }
        match event {
            RecordEvent::Finished(out) => {
                println!(
                    "录屏已保存: {}（{} 帧 / {:.1} 秒 / {:.1} MB）",
                    out.path.display(),
                    out.frames,
                    out.elapsed.as_secs_f64(),
                    out.bytes as f64 / 1_048_576.0
                );
                if self.cfg.record_open_folder {
                    open_folder(&out.path);
                }
                self.last_outcome = Some(out);
            }
            RecordEvent::Failed(e) => {
                eprintln!("录屏失败: {e}");
                message_box(&format!("录屏失败:\n{e}"));
            }
            RecordEvent::Discarded(r) => {
                println!("录屏已放弃（{}）", r.text());
            }
        }
        self.stats = None;
        crate::tray::set_record_state(false, false);
        let _ = reason;
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(s) = self.session.take() {
            s.stop(StopReason::AppExit);
            let _ = s.join();
        }
        self.hud.take();
    }
}

fn with_controller<R>(hwnd: HWND, f: impl FnOnce(&mut Controller) -> R) -> Option<R> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Controller;
    if ptr.is_null() {
        return None;
    }
    Some(f(unsafe { &mut *ptr }))
}

fn register_class() -> Result<()> {
    let name = wide(CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }.is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(host_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册录制宿主窗口类失败".into()));
    }
    Ok(())
}

unsafe extern "system" fn host_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_RECORDER_HUD => {
            with_controller(hwnd, |c| c.on_hud(wp.0));
            return LRESULT(0);
        }
        WM_RECORDER_EVENT => {
            let event = unsafe { Box::from_raw(lp.0 as *mut RecordEvent) };
            with_controller(hwnd, |c| c.on_event(*event));
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Controller;
            if !ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ptr));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
            }
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn open_folder(file: &Path) {
    let _ = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", file.display()))
        .spawn();
}

fn message_box(text: &str) {
    let title = wide("screenloom 录屏");
    let body = wide(text);
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn module_handle() -> windows::Win32::Foundation::HINSTANCE {
    unsafe { windows::Win32::Foundation::HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
