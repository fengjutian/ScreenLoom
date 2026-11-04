//! 原生 `EDIT` 文字输入框：创建、子类化过程、结果回传。

use super::*;

impl OverlayState {
    pub(crate) fn open_text_input(&mut self, at: Point) {
        self.text_anchor = at;
        let p = self.l2i_point(at);
        let x = self.cfg.virtual_origin.x + p.x;
        let y = self.cfg.virtual_origin.y + p.y;
        let h = ((self.canvas.font_size * 2.0) as i32 + 12).max(30);
        let bg = self.canvas.color;
        let fg = contrast_color(bg);
        self.spawn_edit(x, y, 280, h, bg, fg, TextPurpose::Annotation);
        self.dirty = true;
    }

    /// 创建一个浮在浮窗之上的 EDIT 输入框。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_edit(
        &self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        bg: Rgba,
        fg: Rgba,
        purpose: TextPurpose,
    ) {
        let class = win::wide("EDIT");
        let title = win::wide("");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WINDOW_STYLE(WS_POPUP.0 | WS_BORDER.0 | 0x0080), // ES_AUTOHSCROLL
                x,
                y,
                w,
                h,
                None,
                None,
                Some(win::module_handle()),
                None,
            )
        };
        let Ok(hwnd) = hwnd else {
            return;
        };

        let proc: TextProc = text_wnd_proc;
        let old = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as usize as isize) } as usize;
        let brush = unsafe { CreateSolidBrush(COLORREF(bg.to_colorref())) };
        let fg = COLORREF(fg.to_colorref());
        TEXT_PROCS.with(|m| {
            m.borrow_mut().insert(
                hwnd.0 as isize,
                TextInfo {
                    old: unsafe { std::mem::transmute::<usize, TextProc>(old) },
                    brush,
                    fg,
                    purpose,
                },
            );
        });

        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
    }

    pub(crate) fn apply_text_result(&mut self, res: TextResult) {
        match res {
            TextResult::Commit(text) => {
                let text = text.trim_end_matches(['\r', '\n']).to_string();
                if !text.is_empty() {
                    let color = self.canvas.color;
                    let size = self.canvas.font_size;
                    self.canvas.push(Annotation::Text {
                        at: self.text_anchor,
                        text,
                        color,
                        size,
                    });
                }
            }
            TextResult::Cancel => {}
            TextResult::AiCommit(text) => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    if let Some(image) = self.selection_image() {
                        self.submit_ai(AiRequest::Ask {
                            question: text,
                            image,
                        });
                    }
                }
            }
            TextResult::AiCancel => {}
        }
        self.dirty = true;
    }
}

// ---------------------------------------------------------------- 文字输入

pub(crate) type TextProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// 输入框的用途（决定 ENTER / ESC 的语义）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextPurpose {
    /// 标注文字
    Annotation,
    /// 「问 AI」的提问
    AiPrompt,
}

#[derive(Clone, Copy)]
pub(crate) struct TextInfo {
    old: TextProc,
    brush: windows::Win32::Graphics::Gdi::HBRUSH,
    fg: COLORREF,
    purpose: TextPurpose,
}

pub(crate) enum TextResult {
    Commit(String),
    Cancel,
    /// 「问 AI」的提问内容
    AiCommit(String),
    AiCancel,
}

thread_local! {
    static TEXT_RESULT: RefCell<Option<TextResult>> = const { RefCell::new(None) };
    static TEXT_PROCS: RefCell<HashMap<isize, TextInfo>> = RefCell::new(HashMap::new());
}

pub(crate) fn set_text_result(r: Option<TextResult>) {
    TEXT_RESULT.with(|c| {
        *c.borrow_mut() = r;
    });
}

pub(crate) fn take_text_result() -> Option<TextResult> {
    TEXT_RESULT.with(|c| c.borrow_mut().take())
}

unsafe extern "system" fn text_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let vk = (wp.0 as u32) & 0xFF;
            let purpose =
                TEXT_PROCS.with(|m| m.borrow().get(&(hwnd.0 as isize)).map(|i| i.purpose));
            if vk == 0x0D {
                // ENTER → 提交
                let mut buf = [0u16; 1024];
                let n = GetWindowTextW(hwnd, &mut buf).clamp(0, buf.len() as i32 - 1) as usize;
                let text = String::from_utf16_lossy(&buf[..n]);
                let result = match purpose {
                    Some(TextPurpose::AiPrompt) => TextResult::AiCommit(text),
                    _ => TextResult::Commit(text),
                };
                set_text_result(Some(result));
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            } else if vk == 0x1B {
                // ESC → 取消
                let result = match purpose {
                    Some(TextPurpose::AiPrompt) => TextResult::AiCancel,
                    _ => TextResult::Cancel,
                };
                set_text_result(Some(result));
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            if (wp.0 as u32) == 0x0D {
                return LRESULT(0); // 吃掉回车
            }
        }
        WM_CTLCOLOREDIT => {
            let info = TEXT_PROCS.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied());
            if let Some(info) = info {
                let hdc = HDC(wp.0 as *mut core::ffi::c_void);
                unsafe {
                    SetTextColor(hdc, info.fg);
                }
                return LRESULT(info.brush.0 as isize);
            }
        }
        WM_DESTROY => {
            let info = TEXT_PROCS.with(|m| m.borrow_mut().remove(&(hwnd.0 as isize)));
            if let Some(info) = info {
                unsafe {
                    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(info.brush.0));
                }
            }
        }
        _ => {}
    }

    let info = TEXT_PROCS.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied());
    match info {
        Some(info) => unsafe { CallWindowProcW(Some(info.old), hwnd, msg, wp, lp) },
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
