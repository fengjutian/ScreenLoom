//! 完整抓屏流程（`full` feature）。
//!
//! 依赖 `screenloom-capture` 与 `screenloom-ui` 两个 crate 的实现；
//! 在它们落地前本模块不参与编译（由 `full` feature 控制）。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_HOTKEY,
};

use screenloom_capture::{
    capture_virtual_screen, default_backend, list_capture_windows, list_monitors,
    virtual_screen_origin,
};
use screenloom_core::{parse_exclude_entry, Config, Error, Rect, Result, RgbaImage};
use screenloom_ui::{
    post_ai_reply, post_window_picked, render_annotations, AiReply, Overlay, OverlayAction,
    OverlayConfig, OverlayOutcome, PinWindow,
};

/// 热键 ID（任意非零值即可）。
pub const HOTKEY_ID: i32 = 0xBEEF;

/// 常驻模式：注册热键 + 托盘图标，并进入消息循环。
pub fn run_hotkey_loop(cfg: &Config, cfg_path: &std::path::Path) -> Result<()> {
    let hk = crate::hotkey::Hotkey::register(HWND(std::ptr::null_mut()), HOTKEY_ID, &cfg.hotkey)?;
    println!("screenloom 已启动，热键：{}", cfg.hotkey);

    // 托盘图标：左键截图 / 右键菜单（截图·设置·排除窗口·退出）
    let cfg_owned = cfg.clone();
    let path_owned = cfg_path.to_path_buf();
    let tray = crate::tray::Tray::new(move |cmd| match cmd {
        crate::tray::TrayCommand::Capture => {
            if let Err(e) = run_capture(&cfg_owned, false, None, &path_owned) {
                eprintln!("截图失败: {e}");
            }
        }
        crate::tray::TrayCommand::Settings => {
            if let Err(e) = crate::settings::open(&path_owned) {
                eprintln!("设置失败: {e}");
            }
        }
        crate::tray::TrayCommand::ExcludeWindows => {
            if let Err(e) = edit_exclude_windows(&cfg_owned, &path_owned) {
                eprintln!("设置排除窗口失败: {e}");
            }
        }
        crate::tray::TrayCommand::Exit => unsafe {
            windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
        },
    })?;
    println!("托盘图标已就绪（左键截图，右键菜单）");

    // 隐私模式守护：本工具的窗口对屏幕捕获不可见（每隔 1 秒重申一次）。
    let mut _privacy = crate::privacy::PrivacyGuard::start(cfg_path);

    let mut msg = MSG::default();
    loop {
        // SAFETY: 标准消息循环。
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        // SAFETY: 同上。
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if msg.message == WM_HOTKEY {
            if let Err(e) = run_capture(cfg, false, None, cfg_path) {
                eprintln!("截图失败: {e}");
            }
        }
    }
    drop(tray);
    drop(hk);
    Ok(())
}

/// 执行一次完整截图流程：抓屏 → 选区 → 标注 → 按用户选择复制 / 保存 / 贴图。
///
/// `long = true` 时进入长截图（滚动拼接）模式。
/// `sel` 非空时直接用该矩形作为选区（虚拟屏物理坐标），跳过鼠标交互。
pub fn run_capture(
    cfg: &Config,
    long: bool,
    sel: Option<screenloom_core::Rect>,
    cfg_path: &std::path::Path,
) -> Result<()> {
    let backend = default_backend();
    let monitors = list_monitors()?;
    let primary = monitors
        .iter()
        .find(|m| m.is_primary)
        .or_else(|| monitors.first())
        .ok_or_else(|| Error::Capture("未找到任何显示器".into()))?;

    // 抓 **整块虚拟屏**：浮窗铺在虚拟屏上，它要的就是一整张虚拟屏图像
    // （图像内坐标 = 虚拟屏物理坐标 − virtual_origin）。
    let mut frame = capture_virtual_screen(backend.as_ref())?;
    if frame.image.w == 0 || frame.image.h == 0 {
        return Err(Error::Capture("捕获到空图像".into()));
    }

    // 按窗口排除抓屏：把配置里指定的窗口从截图里去掉（内容抹成平均色）。
    apply_excluded_windows(&mut frame.image, cfg, frame.bounds);

    let mut overlay = Overlay::new(OverlayConfig {
        virtual_origin: virtual_screen_origin(),
        scale: primary.scale,
        hide_from_capture: cfg.hide_self,
    })?;
    overlay.show(&frame.image)?;

    // 装上 AI 处理器（问 AI / 翻译 / AI 一键打码）。
    // 没配 API key 就不装——工具栏那三个按钮会返回「未配置 AI 服务」。
    let aicfg = crate::ai::AiConfig::resolve(cfg, None, None, None);
    if aicfg.is_ready() {
        install_ai_handler(&mut overlay, aicfg);
    }

    // 工具栏「设置」按钮 → 打开设置窗口
    let settings_path = cfg_path.to_path_buf();
    overlay.set_settings_handler(move || {
        if let Err(e) = crate::settings::open(&settings_path) {
            eprintln!("设置失败: {e}");
        }
    });

    // 工具栏「提取文字」按钮 → 本地 OCR（**不需要 API key**，无条件装上）
    install_ocr_handler(&mut overlay);

    // 工具栏「识别二维码」按钮 → 本地解码（**不需要 API key**，无条件装上）
    install_qr_handler(&mut overlay);

    // 色值复制 → 剪贴板（剪贴板在 app 层，浮窗只发请求）
    install_clipboard_handler(&mut overlay);

    // 工具栏「窗口选择」按钮 → 弹出窗口选择器，选中的窗口回投给浮窗
    install_window_picker_handler(&mut overlay, frame.bounds);

    // 脚本化模式：直接给定选区（虚拟屏物理坐标），无需鼠标交互。
    if let Some(vrect) = sel {
        let img_rect = screenloom_core::Rect::new(
            vrect.x - frame.bounds.x,
            vrect.y - frame.bounds.y,
            vrect.w,
            vrect.h,
        );
        overlay.set_selection_physical(img_rect);
        overlay.confirm();
    }

    let (region, canvas, action) = match overlay.run()? {
        OverlayOutcome::Cancelled => return Ok(()),
        OverlayOutcome::Selected {
            region,
            canvas,
            action,
        } => (region, canvas, action),
    };

    if region.is_empty() {
        return Ok(());
    }

    let cropped = frame.image.crop(region);

    // 长截图模式：进入滚动拼接循环。
    // `long` 来自 `--long` 命令行；`OverlayAction::LongShot` 来自工具栏「滚动截图」按钮。
    let want_long = long || action == OverlayAction::LongShot;
    let final_image = if want_long {
        // region 是虚拟屏图像内坐标，换算回虚拟屏物理坐标
        let screen_rect = screenloom_core::Rect::new(
            frame.bounds.x + region.x,
            frame.bounds.y + region.y,
            region.w,
            region.h,
        );
        let session = crate::longshot::LongShot {
            screen_rect,
            canvas: cropped,
        };
        let merged = session.run(cfg, backend.as_ref())?;
        // 长截图不叠加编辑器标注（标注在编辑态才有意义），直接用拼接结果
        merged
    } else {
        render_annotations(&cropped, &canvas)
    };

    // 按用户点击的动作执行
    // 「滚动截图」拼完自动保存 + 复制（对齐微信长截图的行为）
    let need_copy = matches!(
        action,
        OverlayAction::Copy | OverlayAction::Confirm | OverlayAction::Pin | OverlayAction::LongShot
    );
    let need_save = matches!(
        action,
        OverlayAction::Save | OverlayAction::Confirm | OverlayAction::Pin | OverlayAction::LongShot
    );

    if need_copy {
        crate::clipboard::write_image(&final_image, cfg.copy_format)?;
        println!("已复制到剪贴板（{}x{}）", final_image.w, final_image.h);
    }
    if need_save {
        let dir = cfg
            .default_dir
            .clone()
            .unwrap_or_else(crate::save::default_dir);
        match crate::save::save_png(&final_image, &dir) {
            Ok(p) => println!("已保存: {}", p.display()),
            Err(e) => eprintln!("保存失败: {e}"),
        }
    }
    if action == OverlayAction::Pin {
        let pin = PinWindow::new(final_image)?;
        pin.show();
    }

    Ok(())
}

/// 给浮窗装上本地 OCR 处理器（工具栏「提取文字」按钮）。
///
/// 与 [`install_ai_handler`] 同样的约定：处理器在 **UI 线程**被调用，必须立刻返回；
/// OCR 放到后台线程，完成后用 `post_ai_reply` 把文本投回浮窗的结果面板。
///
/// 与 AI 不同：OCR 走系统 `Windows.Media.Ocr`，**不需要 API key**，所以无条件装上。
fn install_ocr_handler(overlay: &mut Overlay) {
    // HWND 本身不一定 Send，捕获裸指针值（isize）在线程里重建。
    let hwnd_raw = overlay.hwnd().0 as isize;
    overlay.set_ocr_handler(move |image| {
        let hwnd_raw = hwnd_raw;
        std::thread::spawn(move || {
            let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
            let reply = match crate::ocr::recognize_text(&image) {
                Ok(text) => {
                    // 顺手把结果写进剪贴板，方便直接粘贴
                    let _ = crate::clipboard::write_text(&text);
                    AiReply::Text(text)
                }
                Err(e) => AiReply::Error(format!("本地 OCR 失败: {e}")),
            };
            screenloom_ui::post_ai_reply(hwnd, reply);
        });
    });
}

/// 给浮窗装上 AI 处理器。
///
/// 处理器只在 **UI 线程**被调用，所以它**必须立刻返回**：
/// 真正的网络调用放到后台线程，完成后用 `post_ai_reply` 把结果投回浮窗。
fn install_ai_handler(overlay: &mut Overlay, aicfg: crate::ai::AiConfig) {
    // HWND 本身不一定 Send，捕获裸指针值（isize）在线程里重建。
    let hwnd_raw = overlay.hwnd().0 as isize;
    let client = crate::ai::AiClient::new(aicfg);
    overlay.set_ai_handler(move |req| {
        let client = client.clone();
        let hwnd_raw = hwnd_raw;
        std::thread::spawn(move || {
            let hwnd = HWND(hwnd_raw as *mut core::ffi::c_void);
            let reply = handle_ai_request(&client, req);
            screenloom_ui::post_ai_reply(hwnd, reply);
        });
    });
}

/// 在后台线程里执行一次 AI 请求。
fn handle_ai_request(
    client: &crate::ai::AiClient,
    req: screenloom_ui::AiRequest,
) -> screenloom_ui::AiReply {
    use screenloom_ui::{AiReply, AiRequest};
    match req {
        AiRequest::Ask { question, image } => {
            let png = match image.encode_png() {
                Ok(p) => p,
                Err(e) => return AiReply::Error(format!("图片编码失败: {e}")),
            };
            match client.ask(&question, Some(&crate::ai::ImagePayload::png(png))) {
                Ok(t) => {
                    let _ = crate::clipboard::write_text(&t);
                    AiReply::Text(t)
                }
                Err(e) => AiReply::Error(e.to_string()),
            }
        }
        AiRequest::Translate { image } => {
            let text = match crate::ocr::recognize_text(&image) {
                Ok(t) => t,
                Err(e) => return AiReply::Error(format!("本地 OCR 失败: {e}")),
            };
            if text.trim().is_empty() {
                return AiReply::Error("图片里没有识别到文字".into());
            }
            match client.translate(&text, "英文") {
                Ok(t) => {
                    let _ = crate::clipboard::write_text(&t);
                    AiReply::Text(t)
                }
                Err(e) => AiReply::Error(e.to_string()),
            }
        }
        AiRequest::Mosaic { image } => {
            let (w, h) = (image.w, image.h);
            let png = match image.encode_png() {
                Ok(p) => p,
                Err(e) => return AiReply::Error(format!("图片编码失败: {e}")),
            };
            match client.detect_mosaic(&crate::ai::ImagePayload::png(png)) {
                Ok(regions) => {
                    let rects: Vec<screenloom_ui::MosaicRect> = regions
                        .iter()
                        .map(|r| {
                            let rc = r.to_rect(w, h);
                            screenloom_ui::MosaicRect {
                                x: rc.x,
                                y: rc.y,
                                w: rc.w,
                                h: rc.h,
                            }
                        })
                        .filter(|r| r.w > 0 && r.h > 0)
                        .collect();
                    AiReply::MosaicRegions(rects)
                }
                Err(e) => AiReply::Error(e.to_string()),
            }
        }
    }
}
