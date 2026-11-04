//! 录屏设置窗口（原生控件，独立于截图设置）。
//!
//! 字段：帧率 / 码率 / 倒计时 / 输出目录 / 显示器 / 鼠标指针 / HUD / 完成后打开文件夹。
//! 保存写回同一个 `key=value` 配置文件。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use screenloom_core::{Config, Error, Result};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, SetBkColor, SetTextColor,
    HBRUSH, HDC, HGDIOBJ, PAINTSTRUCT,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::*;

const CLASS_NAME: &str = "ScreenloomRecordSettings";
const ID_SAVE: usize = 4001;
const ID_CANCEL: usize = 4002;
const ID_BROWSE: usize = 4003;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Field {
    Fps,
    Bitrate,
    Countdown,
    Dir,
    Monitor,
    Cursor,
    AudioSystem,
    Hud,
    OpenFolder,
}

struct State {
    hwnd: HWND,
    cfg: Config,
    path: PathBuf,
    k: f32,
    font: windows::Win32::Graphics::Gdi::HFONT,
    bg: HBRUSH,
    field_bg: HBRUSH,
    edits: HashMap<Field, HWND>,
    checks: HashMap<Field, HWND>,
    monitor: HWND,
    monitors: Vec<screenloom_capture::MonitorInfo>,
    saved: bool,
    result: Option<Config>,
}

/// 打开录屏设置。`Some` = 保存，`None` = 取消。
pub fn open(path: &Path) -> Result<Option<Config>> {
    let cfg = Config::load(path).unwrap_or_default();
    ensure_dpi_awareness();
    // 目录选择框走 Shell，需要 COM STA（和截图设置窗口一致）。
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let dpi = system_dpi();
    let k = dpi as f32 / 96.0;

    let bg = unsafe { CreateSolidBrush(COLORREF(0x001E1E1E)) };
    let field_bg = unsafe { CreateSolidBrush(COLORREF(0x002B2B2B)) };
    let font = make_font(k);

    register_record_class(bg)?;
    let class = wide(CLASS_NAME);
    let title = wide("screenloom 录屏设置");
    let (ww, wh) = frame_adjusted(px(560, k), px(430, k), dpi);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ww,
            wh,
            None,
            None,
            Some(module_handle()),
            None,
        )
    }
    .map_err(|e| Error::Window(format!("创建录屏设置窗口失败: {e}")))?;

    let monitors = screenloom_capture::list_monitors().unwrap_or_default();
    let mut state = Box::new(State {
        hwnd,
        cfg,
        path: path.to_path_buf(),
        k,
        font,
        bg,
        field_bg,
        edits: HashMap::new(),
        checks: HashMap::new(),
        monitor: HWND::default(),
        monitors,
        saved: false,
        result: None,
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut *state as *mut State as isize);
    }
    build_controls(&mut state);
    state.fill();
    center(hwnd);
    enable_dark_title(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
    }

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if state.saved || !window_alive(hwnd) {
            break;
        }
    }

    let saved = state.saved;
    let result = state.result.take();
    if window_alive(hwnd) {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    unsafe {
        let _ = DeleteObject(HGDIOBJ(state.font.0));
        let _ = DeleteObject(HGDIOBJ(state.bg.0));
        let _ = DeleteObject(HGDIOBJ(state.field_bg.0));
    }

    if saved {
        if let Some(cfg) = &result {
            cfg.save(&state.path)?;
        }
        if com {
            unsafe { CoUninitialize() };
        }
        Ok(result)
    } else {
        if com {
            unsafe { CoUninitialize() };
        }
        Ok(None)
    }
}

impl State {
    fn fill(&mut self) {
        set_text(self.edit(Field::Fps), &self.cfg.record_fps.to_string());
        set_text(
            self.edit(Field::Bitrate),
            &format!("{:.1}", self.cfg.record_bitrate as f64 / 1_000_000.0),
        );
        set_text(
            self.edit(Field::Countdown),
            &self.cfg.record_countdown.to_string(),
        );
        set_text(
            self.edit(Field::Dir),
            &self
                .cfg
                .record_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        );
        self.set_check(Field::Cursor, self.cfg.record_cursor);
        self.set_check(Field::AudioSystem, self.cfg.record_audio_system);
        self.set_check(Field::Hud, self.cfg.record_show_hud);
        self.set_check(Field::OpenFolder, self.cfg.record_open_folder);

        let sel = if self.cfg.record_monitor >= 0 {
            (self.cfg.record_monitor as usize + 1).min(self.monitors.len())
        } else {
            0
        };
        unsafe {
            let _ = SendMessageW(
                self.monitor,
                CB_SETCURSEL,
                Some(WPARAM(sel)),
                Some(LPARAM(0)),
            );
        }
    }

    fn edit(&self, f: Field) -> HWND {
        self.edits.get(&f).copied().unwrap_or_default()
    }

    fn set_check(&self, f: Field, on: bool) {
        if let Some(h) = self.checks.get(&f) {
            unsafe {
                let _ = SendMessageW(
                    *h,
                    BM_SETCHECK,
                    Some(WPARAM(if on { 1 } else { 0 })),
                    Some(LPARAM(0)),
                );
            }
        }
    }

    fn check(&self, f: Field) -> bool {
        self.checks
            .get(&f)
            .map(|h| unsafe { SendMessageW(*h, BM_GETCHECK, None, None).0 != 0 })
            .unwrap_or(false)
    }

    fn collect(&self) -> Option<Config> {
        let fps = get_text(self.edit(Field::Fps)).trim().parse::<u32>().ok()?;
        if !(1..=120).contains(&fps) {
            return None;
        }
        let mbps = get_text(self.edit(Field::Bitrate))
            .trim()
            .parse::<f64>()
            .ok()?;
        if !(0.1..=100.0).contains(&mbps) {
            return None;
        }
        let countdown = get_text(self.edit(Field::Countdown))
            .trim()
            .parse::<u32>()
            .ok()?;
        if countdown > 10 {
            return None;
        }

        let mut cfg = self.cfg.clone();
        cfg.record_fps = fps;
        cfg.record_bitrate = (mbps * 1_000_000.0).round() as u32;
        cfg.record_countdown = countdown;
        let dir = get_text(self.edit(Field::Dir)).trim().to_string();
        cfg.record_dir = if dir.is_empty() {
            None
        } else {
            Some(PathBuf::from(dir))
        };
        let sel = unsafe { SendMessageW(self.monitor, CB_GETCURSEL, None, None).0 };
        cfg.record_monitor = if sel <= 0 {
            -1
        } else {
            (sel - 1) as i32
        };
        cfg.record_cursor = self.check(Field::Cursor);
        cfg.record_audio_system = self.check(Field::AudioSystem);
        cfg.record_show_hud = self.check(Field::Hud);
        cfg.record_open_folder = self.check(Field::OpenFolder);
        Some(cfg)
    }
}

fn build_controls(st: &mut State) {
    let k = st.k;
    let x_label = px(24, k);
    let x_ctrl = px(180, k);
    let w_ctrl = px(330, k);
    let h = px(26, k);
    let mut y = px(22, k);
    let step = px(38, k);

    let label = |text: &str, y: i32| unsafe {
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide("STATIC").as_ptr()),
            PCWSTR(wide(text).as_ptr()),
            WS_CHILD | WS_VISIBLE,
            x_label,
            y + px(5, k),
            px(140, k),
            px(20, k),
            Some(st.hwnd),
            None,
            Some(module_handle()),
            None,
        )
        .unwrap_or_default();
        let _ = SendMessageW(h, WM_SETFONT, Some(WPARAM(st.font.0 as usize)), Some(LPARAM(0)));
        h
    };
    label("录制帧率（1–120）", y);
    let e = make_edit(st.hwnd, x_ctrl, y, w_ctrl, h, st, false);
    st.edits.insert(Field::Fps, e);
    y += step;

    label("码率（Mbps）", y);
    let e = make_edit(st.hwnd, x_ctrl, y, w_ctrl, h, st, false);
    st.edits.insert(Field::Bitrate, e);
    y += step;

    label("开始前倒计时（秒）", y);
    let e = make_edit(st.hwnd, x_ctrl, y, w_ctrl, h, st, false);
    st.edits.insert(Field::Countdown, e);
    y += step;

    label("输出目录（空 = 视频文件夹）", y);
    let e = make_edit(st.hwnd, x_ctrl, y, px(260, k), h, st, false);
    st.edits.insert(Field::Dir, e);
    let browse = make_button(st.hwnd, "浏览…", x_ctrl + px(268, k), y, px(62, k), h, ID_BROWSE, st);
    let _ = browse;
    y += step;

    label("录制显示器", y);
    let combo = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide("COMBOBOX").as_ptr()),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(0x0003) | WS_VSCROLL,
            x_ctrl,
            y,
            w_ctrl,
            px(200, k),
            Some(st.hwnd),
            Some(HMENU(Field::Monitor as usize as *mut core::ffi::c_void)),
            Some(module_handle()),
            None,
        )
    }
    .unwrap_or_default();
    st.monitor = combo;
    unsafe {
        let _ = SendMessageW(combo, WM_SETFONT, Some(WPARAM(st.font.0 as usize)), Some(LPARAM(0)));
        let _ = SendMessageW(
            combo,
            CB_ADDSTRING,
            Some(WPARAM(0)),
            Some(LPARAM(wide("主显示器（自动）").as_ptr() as isize)),
        );
        for (i, m) in st.monitors.iter().enumerate() {
            let text = format!(
                "显示器 {} · {}x{} · {:.0}%",
                i,
                m.bounds.w,
                m.bounds.h,
                m.scale * 100.0
            );
            let w = wide(&text);
            let _ = SendMessageW(
                combo,
                CB_ADDSTRING,
                Some(WPARAM(0)),
                Some(LPARAM(w.as_ptr() as isize)),
            );
        }
    }
    y += step;

    let mut check = |field: Field, text: &str, y: i32| {
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(wide("BUTTON").as_ptr()),
                PCWSTR(wide(text).as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(0x0003), // BS_AUTOCHECKBOX
                x_ctrl,
                y,
                px(340, k),
                h,
                Some(st.hwnd),
                None,
                Some(module_handle()),
                None,
            )
        }
        .unwrap_or_default();
        unsafe {
            let _ = SendMessageW(h, WM_SETFONT, Some(WPARAM(st.font.0 as usize)), Some(LPARAM(0)));
        }
        st.checks.insert(field, h);
    };
    check(Field::Cursor, "录制鼠标指针", y);
    y += px(30, k);
    check(Field::AudioSystem, "录制系统声音（WASAPI loopback）", y);
    y += px(30, k);
    check(Field::Hud, "录制时显示 HUD（计时/暂停/停止）", y);
    y += px(30, k);
    check(Field::OpenFolder, "结束后打开所在文件夹", y);

    let by = px(372, k);
    make_button(st.hwnd, "保存", px(330, k), by, px(90, k), px(30, k), ID_SAVE, st);
    make_button(st.hwnd, "取消", px(430, k), by, px(90, k), px(30, k), ID_CANCEL, st);
}

fn make_edit(parent: HWND, x: i32, y: i32, w: i32, h: i32, st: &State, password: bool) -> HWND {
    let mut style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(0x0080); // ES_AUTOHSCROLL
    if password {
        style |= WINDOW_STYLE(0x0020); // ES_PASSWORD
    }
    let e = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0x00000200), // WS_EX_CLIENTEDGE
            PCWSTR(wide("EDIT").as_ptr()),
            PCWSTR::null(),
            style,
            x,
            y,
            w,
            h,
            Some(parent),
            None,
            Some(module_handle()),
            None,
        )
    }
    .unwrap_or_default();
    unsafe {
        let _ = SendMessageW(e, WM_SETFONT, Some(WPARAM(st.font.0 as usize)), Some(LPARAM(0)));
    }
    e
}

fn make_button(
    parent: HWND,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: usize,
    st: &State,
) -> HWND {
    let b = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide("BUTTON").as_ptr()),
            PCWSTR(wide(text).as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            x,
            y,
            w,
            h,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            Some(module_handle()),
            None,
        )
    }
    .unwrap_or_default();
    unsafe {
        let _ = SendMessageW(b, WM_SETFONT, Some(WPARAM(st.font.0 as usize)), Some(LPARAM(0)));
    }
    b
}

fn register_record_class(bg: HBRUSH) -> Result<()> {
    let name = wide(CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }.is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(record_wnd_proc),
        hInstance: module_handle(),
        hbrBackground: bg,
        lpszClassName: PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册录屏设置窗口类失败".into()));
    }
    Ok(())
}

unsafe extern "system" fn record_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut State;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    let st = unsafe { &mut *ptr };
    match msg {
        WM_COMMAND => {
            let id = (wp.0 as u32 & 0xFFFF) as usize;
            match id {
                ID_SAVE => {
                    if let Some(cfg) = st.collect() {
                        st.result = Some(cfg);
                        st.saved = true;
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                    } else {
                        message_box("录屏设置不合法：帧率 1–120、码率 0.1–100 Mbps、倒计时 0–10 秒。");
                    }
                    return LRESULT(0);
                }
                ID_CANCEL => {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                    return LRESULT(0);
                }
                ID_BROWSE => {
                    if let Some(dir) = pick_folder(hwnd) {
                        set_text(st.edit(Field::Dir), &dir.display().to_string());
                    }
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let hdc = HDC(wp.0 as *mut core::ffi::c_void);
            unsafe {
                SetTextColor(hdc, COLORREF(0x00E7EAEE));
                SetBkColor(hdc, COLORREF(0x001E1E1E));
            }
            return LRESULT(st.bg.0 as isize);
        }
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wp.0 as *mut core::ffi::c_void);
            unsafe {
                SetTextColor(hdc, COLORREF(0x00E7EAEE));
                SetBkColor(hdc, COLORREF(0x002B2B2B));
            }
            return LRESULT(st.field_bg.0 as isize);
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            let mut rc = windows::Win32::Foundation::RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &mut rc);
                FillRect(hdc, &rc, st.bg);
            }
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn message_box(text: &str) {
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(wide(text).as_ptr()),
            PCWSTR(wide("screenloom 录屏设置").as_ptr()),
            MB_OK | MB_ICONWARNING,
        );
    }
}
