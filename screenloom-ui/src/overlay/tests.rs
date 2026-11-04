use super::*;
use windows::Win32::Foundation::RECT;

/// 带明显颜色梯度的测试图（便于校验裁剪位置）。
fn test_bg(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.set_pixel(x, y, Rgba::rgb(x as u8, y as u8, 0x40));
        }
    }
    img
}

/// 把 (x, y) 打包成 lParam（低位 x，高位 y）。
fn lparam(x: i32, y: i32) -> LPARAM {
    LPARAM(((((y as u32) & 0xffff) << 16) | ((x as u32) & 0xffff)) as isize)
}

fn send(hwnd: HWND, msg: u32, x: i32, y: i32) {
    unsafe {
        SendMessageW(hwnd, msg, Some(WPARAM(1)), Some(lparam(x, y)));
    }
}

/// 拖拽：从 (x0,y0) 按下，移动到 (x1,y1)，松开。
fn drag(ov: &Overlay, x0: i32, y0: i32, x1: i32, y1: i32) {
    send(ov.hwnd, WM_LBUTTONDOWN, x0, y0);
    send(ov.hwnd, WM_MOUSEMOVE, x1, y1);
    send(ov.hwnd, WM_LBUTTONUP, x1, y1);
}

fn key(ov: &Overlay, vk: u32) {
    unsafe {
        SendMessageW(
            ov.hwnd,
            WM_KEYDOWN,
            Some(WPARAM(vk as usize)),
            Some(LPARAM(0)),
        );
    }
}

fn click(ov: &Overlay, x: i32, y: i32) {
    send(ov.hwnd, WM_LBUTTONDOWN, x, y);
    send(ov.hwnd, WM_LBUTTONUP, x, y);
}

/// 工具栏上某个按钮的中心（客户区像素）。
fn toolbar_point(ov: &Overlay, kind: ItemKind) -> Point {
    let it = ov
        .state
        .toolbar
        .items
        .iter()
        .find(|i| i.kind == kind)
        .expect("工具栏按钮不存在");
    let center = Point::new(it.rect.x + it.rect.w / 2, it.rect.y + it.rect.h / 2);
    ov.state.l2i_point(center)
}

fn make_overlay(origin: Point) -> Overlay {
    let mut ov = Overlay::new(OverlayConfig {
        virtual_origin: origin,
        scale: 1.0,
        hide_from_capture: false,
        auto_scale: false,
    })
    .expect("Overlay::new 失败");
    ov.show(&test_bg(800, 600)).expect("show 失败");
    ov
}

/// 泵一遍消息队列（模拟浮窗的消息循环收到一条消息）。
fn pump() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 鼠标（客户区 = 屏幕 − 窗口原点 = 图像内）坐标 → 图像内物理像素坐标，
/// 并且 `bg.crop(region)` 能裁出正确的区域。
#[test]
fn mouse_coords_map_to_image_pixels_and_crop_correctly() {
    let origin = Point::new(137, 59); // 非零虚拟屏原点
    let bg = test_bg(800, 600);
    let mut ov = Overlay::new(OverlayConfig {
        virtual_origin: origin,
        scale: 1.0,
        hide_from_capture: false,
        auto_scale: false,
    })
    .unwrap();
    ov.show(&bg).expect("show 失败");

    // 浮窗确实放在虚拟屏原点上
    let mut wr = RECT::default();
    unsafe { GetWindowRect(ov.hwnd, &mut wr).unwrap() };
    assert_eq!((wr.left, wr.top), (origin.x, origin.y));

    // 拖出一个选区（客户区坐标 = 图像内像素）
    drag(&ov, 210, 160, 410, 330);

    let region = ov.state.l2i(ov.state.selection.expect("没有选区"));
    assert_eq!(
        region,
        Rect::new(210, 160, 200, 170),
        "region 必须是图像内物理像素"
    );

    // 屏幕坐标 → 图像内坐标：图像内 = 屏幕 − 捕获帧左上角
    let screen = Point::new(wr.left + 210, wr.top + 160);
    assert_eq!(
        Point::new(screen.x - origin.x, screen.y - origin.y),
        Point::new(210, 160)
    );

    // crop 出来的必须是原图对应区域
    let cropped = bg.crop(region);
    assert_eq!((cropped.w, cropped.h), (200, 170));
    assert_eq!(cropped.pixel(0, 0), bg.pixel(210, 160));
    assert_eq!(cropped.pixel(199, 169), bg.pixel(409, 329));

    ov.destroy();
}

/// 四个 OverlayAction 都能从工具栏真实触发并正确返回。
#[test]
fn toolbar_actions_return_expected_action() {
    for (kind, expect) in [
        (ItemKind::Complete, OverlayAction::Confirm),
        (ItemKind::Copy, OverlayAction::Copy),
        (ItemKind::Save, OverlayAction::Save),
        (ItemKind::Pin, OverlayAction::Pin),
    ] {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330);
        key(&ov, 0x0D); // Enter → 编辑态（工具栏出现）
        let p = toolbar_point(&ov, kind);
        click(&ov, p.x, p.y);
        match ov.run().unwrap() {
            OverlayOutcome::Selected { region, action, .. } => {
                assert_eq!(action, expect, "动作不匹配");
                assert_eq!(region, Rect::new(210, 160, 200, 170));
            }
            OverlayOutcome::Cancelled => panic!("不该是 Cancelled"),
        }
        ov.destroy();
    }
}

/// Esc → Cancelled。
#[test]
fn escape_cancels() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330);
    key(&ov, 0x1B);
    assert!(matches!(ov.run().unwrap(), OverlayOutcome::Cancelled));
    ov.destroy();
}

/// 画布里的标注坐标必须是 **裁剪后图像内的像素**（相对选区左上角）。
#[test]
fn canvas_annotations_are_region_relative_pixels() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330); // 选区左上角 = (210,160)
    key(&ov, 0x0D); // 进入编辑态
    ov.state.canvas.tool = Tool::Rect;
    drag(&ov, 250, 200, 350, 260); // 在选区内画一个矩形

    let p = toolbar_point(&ov, ItemKind::Complete);
    click(&ov, p.x, p.y);

    match ov.run().unwrap() {
        OverlayOutcome::Selected { region, canvas, .. } => {
            assert_eq!(region, Rect::new(210, 160, 200, 170));
            let shape = canvas
                .items
                .iter()
                .find_map(|a| match a {
                    Annotation::Shape { rect, .. } => Some(*rect),
                    _ => None,
                })
                .expect("没有矩形标注");
            // 屏幕 (250,200)-(350,260) → 相对选区左上角 (210,160) = (40,40)-(140,100)
            assert_eq!(shape, Rect::new(40, 40, 100, 60));
        }
        OverlayOutcome::Cancelled => panic!("不该是 Cancelled"),
    }
    ov.destroy();
}

// ---------------- AI 接入 ----------------

/// 缩放 != 1 时，标注坐标仍必须落在「裁剪后图像的物理像素」上。
///
/// 回归测试：曾经 `display_canvas` 漏乘 `scale`，本机主屏 1.25 缩放下
/// 所有标注整体偏移 25%。
#[test]
fn canvas_annotations_are_physical_pixels_under_scaled_dpi() {
    let mut ov = Overlay::new(OverlayConfig {
        virtual_origin: Point::new(-1038, -1440),
        scale: 1.25,
        hide_from_capture: false,
        auto_scale: false,
    })
    .unwrap();
    ov.show(&test_bg(2000, 1600)).expect("show 失败");

    drag(&ov, 250, 200, 350, 260); // 选区（客户区像素）
    key(&ov, 0x0D); // 进入编辑态
    ov.state.canvas.tool = Tool::Rect;
    drag(&ov, 300, 250, 400, 310); // 画矩形

    let canvas = ov.state.display_canvas();
    let shape = canvas
        .items
        .iter()
        .find_map(|a| match a {
            Annotation::Shape { rect, .. } => Some(*rect),
            _ => None,
        })
        .expect("没有矩形标注");
    // 选区物理 (250,200)-(350,260)；矩形物理 (300,250)-(400,310)
    // → 相对选区左上角 = (50,50)-(150,110)
    assert_eq!(shape, Rect::new(50, 50, 100, 60));
    ov.destroy();
}

/// 工具栏上有「问 AI / 翻译 / AI 打码」三个按钮；
/// 没装 AI 处理器时点击会给出明确报错（而不是静默无反应）。
#[test]
fn ai_toolbar_buttons_exist_and_report_missing_handler() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330);
    key(&ov, 0x0D); // 进入编辑态（工具栏出现）

    for kind in [ItemKind::AskAi, ItemKind::Translate, ItemKind::AiMosaic] {
        let p = toolbar_point(&ov, kind);
        assert!(p.x >= 0 && p.y >= 0, "按钮位置异常: {p:?}");
    }

    let p = toolbar_point(&ov, ItemKind::Translate);
    click(&ov, p.x, p.y);
    assert!(
        matches!(ov.state.ai_reply, Some(AiReply::Error(_))),
        "没装 handler 时应给出错误提示"
    );
    assert!(!ov.state.ai_pending);
    ov.destroy();
}

/// 工具栏上的「提取文字」按钮：装了 OCR 处理器时把**选区图像**交给它，
/// 没装时给出明确报错（而不是静默无反应）。
#[test]
fn extract_text_button_exists_and_reports_missing_handler() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330);
    key(&ov, 0x0D); // 进入编辑态（工具栏出现）

    let p = toolbar_point(&ov, ItemKind::ExtractText);
    assert!(p.x >= 0 && p.y >= 0, "按钮位置异常: {p:?}");

    click(&ov, p.x, p.y);
    assert_eq!(ov.state.ai_title, "提取文字", "标题应是「提取文字」");
    assert!(
        matches!(ov.state.ai_reply, Some(AiReply::Error(_))),
        "没装 handler 时应给出错误提示"
    );
    assert!(!ov.state.ai_pending);
    ov.destroy();
}

/// 「提取文字」把**选区裁剪后的图像**交给 OCR 处理器，
/// 回投的文本会显示在结果面板里。
#[test]
fn extract_text_button_passes_selection_image_to_handler() {
    use std::sync::{Arc, Mutex};

    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330); // 选区 (210,160) 200x170
    key(&ov, 0x0D);

    let seen: Arc<Mutex<Option<RgbaImage>>> = Arc::new(Mutex::new(None));
    let sink = seen.clone();
    ov.set_ocr_handler(move |img| {
        *sink.lock().unwrap() = Some(img);
    });

    let p = toolbar_point(&ov, ItemKind::ExtractText);
    click(&ov, p.x, p.y);

    let img = seen.lock().unwrap().take().expect("OCR 处理器没被调用");
    assert_eq!(
        (img.w, img.h),
        (200, 170),
        "交给 OCR 的必须是选区裁剪后的图像"
    );
    // 没回投结果时面板处于 pending
    assert!(ov.state.ai_pending, "等待回投时应是 pending");

    // 模拟后台线程回投
    post_ai_reply(ov.hwnd(), AiReply::Text("识别出来的文字".into()));
    pump();
    assert_eq!(
        ov.state.ai_reply,
        Some(AiReply::Text("识别出来的文字".into()))
    );
    assert!(!ov.state.ai_pending);
    ov.destroy();
}

/// 工具栏上的「滚动截图」按钮：点击后以 `OverlayAction::LongShot` 收尾，
/// 选区原样带回给 app 层（app 层拿它去跑滚动拼接）。
#[test]
fn long_shot_button_returns_long_shot_action() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330);
    key(&ov, 0x0D);

    let p = toolbar_point(&ov, ItemKind::LongShot);
    assert!(p.x >= 0 && p.y >= 0, "按钮位置异常: {p:?}");
    click(&ov, p.x, p.y);

    match ov.run().unwrap() {
        OverlayOutcome::Selected { region, action, .. } => {
            assert_eq!(action, OverlayAction::LongShot, "动作不匹配");
            assert_eq!(region, Rect::new(210, 160, 200, 170));
        }
        OverlayOutcome::Cancelled => panic!("不该是 Cancelled"),
    }
    ov.destroy();
}

/// 浮窗退出后**不能**给下一个消息循环留一个 `WM_QUIT`。
///
/// 坑：`finish()` 投 `WM_QUIT`，但事件循环是**先看到 outcome 才 break** 的，
/// 那条 `WM_QUIT` 留在队列里 → 紧接着的长截图按键循环一 `GetMessageW` 就退出，
/// 长截图功能形同虚设（只截到第一帧）。
#[test]
fn overlay_exit_leaves_no_pending_quit_message() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330);
    key(&ov, 0x0D);
    let p = toolbar_point(&ov, ItemKind::LongShot);
    click(&ov, p.x, p.y);
    let out = ov.run().unwrap();
    assert!(
        matches!(out, OverlayOutcome::Selected { .. }),
        "应该正常收尾而不是 Cancelled"
    );

    let mut msg = MSG::default();
    let pending = unsafe { PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_NOREMOVE).as_bool() };
    assert!(!pending, "WM_QUIT 残留会让下一个消息循环立刻退出");
    ov.destroy();
}

/// 后台线程回投的 AI 结果能被浮窗收到并应用。
#[test]
fn ai_reply_posted_from_thread_is_applied() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330);
    key(&ov, 0x0D);

    // 模拟后台线程：post_ai_reply 把结果塞进消息队列
    post_ai_reply(ov.hwnd(), AiReply::Text("这是回答".into()));
    pump();

    assert_eq!(ov.state.ai_reply, Some(AiReply::Text("这是回答".into())));
    assert!(!ov.state.ai_pending);
    ov.destroy();
}

/// AI 打码的结果会变成画布上的马赛克标注（坐标相对裁剪后图像左上角）。
#[test]
fn ai_mosaic_reply_creates_canvas_mosaic_annotations() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330); // 选区 (210,160)
    key(&ov, 0x0D);

    post_ai_reply(
        ov.hwnd(),
        AiReply::MosaicRegions(vec![MosaicRect {
            x: 10,
            y: 20,
            w: 30,
            h: 40,
        }]),
    );
    pump();

    let canvas = ov.state.display_canvas();
    let regions = canvas.mosaic_regions();
    assert_eq!(regions.len(), 1, "应该有一个马赛克区域");
    // 模型给的是「相对裁剪图左上角」的偏移
    assert_eq!(regions[0].0, Rect::new(10, 20, 30, 40));
    ov.destroy();
}

/// 马赛克工具下，线宽 +/- 应改为调节马赛克块大小。
#[test]
fn mosaic_tool_width_buttons_change_block_size() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 210, 160, 410, 330);
    key(&ov, 0x0D);

    ov.state.canvas.tool = Tool::Mosaic;
    ov.state.canvas.mosaic_block = 12;
    ov.state.on_toolbar(ItemKind::WidthUp);
    assert_eq!(ov.state.canvas.mosaic_block, 14);
    ov.state.on_toolbar(ItemKind::WidthDown);
    assert_eq!(ov.state.canvas.mosaic_block, 12);
    ov.destroy();
}

/// 裁剪工具：在编辑态拖一个更小的框，输出选区应缩小到该框。
#[test]
fn crop_tool_shrinks_selection() {
    let mut ov = make_overlay(Point::new(0, 0));
    drag(&ov, 100, 100, 400, 300);
    key(&ov, 0x0D); // 进入编辑态

    ov.state.canvas.tool = Tool::Crop;
    drag(&ov, 150, 150, 300, 250);
    assert_eq!(ov.state.selection, Some(Rect::new(150, 150, 150, 100)));

    ov.confirm();
    match ov.state.outcome.take() {
        Some(OverlayOutcome::Selected { region, .. }) => {
            assert_eq!(region, Rect::new(150, 150, 150, 100));
        }
        other => panic!("expected Selected, got {other:?}"),
    }
    ov.destroy();
}
