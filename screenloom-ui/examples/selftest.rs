//! 自检：
//! 1. `render_annotations` —— 渐变图 + 矩形/箭头/马赛克 → `render_test.png`
//! 2. 浮窗 —— 建窗、贴图、渲染一帧（不跑无限事件循环）→ `overlay_test.png`
//!
//! 运行：
//! `cargo run --manifest-path D:\github\screenloom\Cargo.toml -p screenloom-ui --example selftest`

use screenloom_core::{Annotation, Canvas, Point, Rect, Rgba, RgbaImage, ShapeKind};
use screenloom_ui::{render_annotations, Overlay, OverlayConfig};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, SendMessageW, TranslateMessage, MSG, PM_REMOVE, WM_KEYDOWN,
};

/// 800x600 渐变测试图。
fn gradient(w: u32, h: u32) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w.max(1)) as u8;
            let g = (y * 255 / h.max(1)) as u8;
            let b = ((x + y) * 255 / (w + h).max(1)) as u8;
            img.set_pixel(x, y, Rgba::rgb(r, g, b));
        }
    }
    img
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("workspace root not found")?
        .to_path_buf();
    // 产物统一放 e2e_out/，不污染项目根目录
    let out_dir = root.join("e2e_out");
    std::fs::create_dir_all(&out_dir)?;

    // ---------- A. render_annotations ----------
    let base = gradient(800, 600);
    let mut canvas = Canvas::new();
    canvas.push(Annotation::Shape {
        kind: ShapeKind::Rect,
        rect: Rect::new(120, 120, 260, 160),
        color: Rgba::rgb(0xFA, 0x51, 0x51),
        width: 3.0,
    });
    canvas.push(Annotation::Arrow {
        from: Point::new(430, 420),
        to: Point::new(700, 170),
        color: Rgba::rgb(0x00, 0xC3, 0x75),
        width: 4.0,
    });
    canvas.push(Annotation::Mosaic {
        rect: Rect::new(500, 420, 120, 90),
        block: 12,
    });

    let out = render_annotations(&base, &canvas);
    assert_eq!((out.w, out.h), (800, 600), "输出尺寸必须与 base 一致");

    // 肉眼之外的机器校验：
    // 1) 矩形边框是红色 (#FA5151)
    let border = out.pixel(250, 120);
    assert!(
        border.r > 200 && border.g < 120 && border.b < 120,
        "矩形上边框应为红色，实际 {border:?}"
    );
    // 2) 箭头是绿色 (#00C375)
    let arrow = out.pixel(565, 295); // 箭头线段中点附近
    assert!(
        arrow.g > 140 && arrow.r < 120 && arrow.b < 140,
        "箭头应为绿色，实际 {arrow:?}"
    );
    // 3) 马赛克：块内像素被平均（同一块内取值相同），且与原图不同
    let (bx, by) = (500u32, 420u32);
    let p0 = out.pixel(bx + 1, by + 1);
    let p1 = out.pixel(bx + 2, by + 2);
    assert_eq!(p0, p1, "马赛克块内像素应相同");
    assert_ne!(p0, base.pixel(bx + 1, by + 1), "马赛克区域应与原图不同");

    let p1_path = out_dir.join("render_test.png");
    std::fs::write(&p1_path, out.encode_png()?)?;
    println!("[A] render_annotations -> {}", p1_path.display());

    // ---------- B. 浮窗（选区阶段） ----------
    let mut ov = Overlay::new(OverlayConfig {
        virtual_origin: Point::new(0, 0),
        scale: 1.0,
        hide_from_capture: false,
    })?;
    ov.show(&base)?;
    // 选一个区域，让边框 + 尺寸提示可见
    ov.set_selection(Rect::new(120, 100, 300, 220));
    let frame = ov.render_frame()?;
    assert_eq!((frame.w, frame.h), (800, 600));
    let p2 = out_dir.join("overlay_test.png");
    std::fs::write(&p2, frame.encode_png()?)?;
    println!("[B] overlay frame -> {}", p2.display());

    // ---------- C. 浮窗（编辑阶段：带工具栏） ----------
    // 按 Enter 进入编辑态
    unsafe {
        SendMessageW(ov.hwnd(), WM_KEYDOWN, Some(WPARAM(0x0D)), Some(LPARAM(0)));
    }
    let edit_frame = ov.render_frame()?;
    let p3 = out_dir.join("overlay_edit.png");
    std::fs::write(&p3, edit_frame.encode_png()?)?;
    println!("[C] overlay edit frame -> {}", p3.display());

    // ---------- D. AI 结果面板 ----------
    // 模拟「后台线程调完模型 → post_ai_reply 回投」
    screenloom_ui::post_ai_reply(
        ov.hwnd(),
        screenloom_ui::AiReply::Text(
            "这是一条来自模型的回答，用来验证 AI 结果面板的渲染效果：面板是半透明深色底，\
             标题用强调色，正文自动换行，超长会截断并提示「完整内容已复制到剪贴板」。"
                .into(),
        ),
    );
    pump_messages();
    let panel_frame = ov.render_frame()?;
    let p4 = out_dir.join("overlay_ai_panel.png");
    std::fs::write(&p4, panel_frame.encode_png()?)?;
    println!("[D] overlay AI panel -> {}", p4.display());

    // ---------- E. 工具栏放大图（检查图标是否清晰可辨） ----------
    // 用 scale=3.0 的浮窗渲染，工具栏会按 3 倍物理像素画出来，便于肉眼检查图标。
    let big_bg = gradient(4800, 1800);
    let mut ov3 = Overlay::new(OverlayConfig {
        virtual_origin: Point::new(0, 0),
        scale: 3.0,
        hide_from_capture: false,
    })?;
    ov3.show(&big_bg)?;
    ov3.set_selection(Rect::new(1200, 400, 2400, 700));
    unsafe {
        SendMessageW(ov3.hwnd(), WM_KEYDOWN, Some(WPARAM(0x0D)), Some(LPARAM(0)));
    }
    let big_frame = ov3.render_frame()?;
    // 工具栏居中：逻辑宽 ≈1186 → 物理 ≈3558，左端约在 x=621。
    // 裁出**工具图标区**（最左边 7 个工具）放大看。
    let strip = big_frame.crop(Rect::new(630, 1600, 700, 190));
    let p5 = out_dir.join("toolbar_zoom.png");
    std::fs::write(&p5, strip.encode_png()?)?;
    println!("[E] toolbar zoom -> {}", p5.display());
    ov3.destroy();

    // 不跑事件循环，直接销毁
    ov.destroy();

    println!("selftest ok");
    Ok(())
}

/// 泵一遍消息队列（让浮窗收到刚投过来的 `WM_AI_REPLY`）。
fn pump_messages() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
