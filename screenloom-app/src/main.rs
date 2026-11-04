//! screenloom —— 微信 4.x 截图工具（`screenloom`）的 Rust 复刻。
//!
//! 用法：
//! ```text
//! screenloom              # 常驻，注册全局热键（默认 Alt+A）
//! screenloom --once       # 截一次图，复制到剪贴板并存盘后退出
//! screenloom --hotkey F9  # 临时覆盖热键
//! screenloom --config PATH
//! screenloom --version
//! ```
//!
//! AI 能力（DeepSeek / MiniMax，见 `ai.rs`）：
//! ```text
//! screenloom --ask "这张图里是什么"        # 问 AI（图片取自剪贴板）
//! screenloom --translate 英文              # 翻译图里的文字
//! screenloom --ai-mosaic                   # AI 一键打码
//! ```
//!
//! 模块划分见 `doc/实现契约.md`。

mod ai;
mod aicli;
mod clipboard;
mod exclude;
mod hotkey;
mod http;
mod ocr;
mod overlay_handlers;
mod privacy;
mod record;
mod save;
mod settings;
mod theme;
mod tray;
mod windowpicker;

#[cfg(feature = "full")]
mod flow;
#[cfg(feature = "full")]
mod longshot;

/// 命令行解析结果。
#[derive(Default)]
struct Args {
    once: bool,
    long: bool,
    ocr_only: bool,
    hotkey: Option<String>,
    sel: Option<screenloom_core::Rect>,
    /// `--ask` 的问题
    ask: Option<String>,
    /// `--translate` 的目标语言
    translate: Option<String>,
    /// `--ai-mosaic`
    ai_mosaic: bool,
    /// `--ai-probe`：只测端点连通性
    ai_probe: bool,
    /// `--record SECONDS`：录屏时长（秒）
    record: Option<u64>,
    /// `--record-out PATH`：录制输出文件（.mp4）
    record_out: Option<std::path::PathBuf>,
    /// `--record-monitor N`：录制目标显示器下标
    record_monitor: Option<usize>,
    /// `--record-fps N`：录制帧率
    record_fps: Option<u32>,
    /// `--record-bitrate BPS`：录制码率
    record_bitrate: Option<u32>,
    /// `--record-countdown SECS`：开始前倒计时
    record_countdown: Option<u32>,
    /// `--settings`：打开设置窗口
    open_settings: bool,
    provider: Option<ai::Provider>,
    model: Option<String>,
    endpoint: Option<String>,
    /// 输入图片路径（默认取剪贴板）
    image: Option<std::path::PathBuf>,
    /// `--ai-mosaic` 的输出目录
    ai_out: Option<std::path::PathBuf>,
    /// 马赛克块大小（像素）
    ai_block: u32,
    cfg_path: Option<std::path::PathBuf>,
    /// `--theme PATH`：主题覆盖 JSON（配色 / 圆角热重载）
    theme_path: Option<std::path::PathBuf>,
}

fn main() {
    let mut args = Args {
        ai_block: 12,
        ..Args::default()
    };

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--once" => args.once = true,
            "--long" => {
                args.long = true;
                args.once = true;
            }
            "--ocr" => args.ocr_only = true,
            "--version" | "-V" => {
                println!("screenloom {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                print_help();
                return;
            }
            "--config" => {
                i += 1;
                match argv.get(i) {
                    Some(p) => args.cfg_path = Some(std::path::PathBuf::from(p)),
                    None => die("--config 需要一个路径参数"),
                }
            }
            "--theme" => {
                i += 1;
                match argv.get(i) {
                    Some(p) => args.theme_path = Some(std::path::PathBuf::from(p)),
                    None => die("--theme 需要一个路径参数"),
                }
            }
            "--hotkey" => {
                i += 1;
                match argv.get(i) {
                    Some(k) => args.hotkey = Some(k.clone()),
                    None => die("--hotkey 需要一个键位参数，例如 Alt+A"),
                }
            }
            "--sel" => {
                // --sel x,y,w,h —— 虚拟屏物理坐标的选区，脚本化截图（不需要鼠标）
                i += 1;
                let Some(s) = argv.get(i) else {
                    die("--sel 需要一个参数，例如 --sel 100,200,800,600");
                };
                let parts: Vec<i32> = s
                    .split(',')
                    .filter_map(|p| p.trim().parse::<i32>().ok())
                    .collect();
                if parts.len() != 4 || parts[2] <= 0 || parts[3] <= 0 {
                    die(&format!("--sel 格式应为 x,y,w,h（物理像素），收到: {s}"));
                }
                args.sel = Some(screenloom_core::Rect::new(
                    parts[0], parts[1], parts[2], parts[3],
                ));
                args.once = true;
            }
            // ---- AI 能力 ----
            "--ask" => {
                i += 1;
                let Some(q) = argv.get(i) else {
                    die("--ask 需要一个问题，例如 --ask \"这张图里是什么\"");
                };
                args.ask = Some(q.clone());
            }
            "--translate" => {
                // 目标语言可选：下一个 token 不以 '-' 开头时当作语言
                let mut target = "英文".to_string();
                if let Some(next) = argv.get(i + 1) {
                    if !next.starts_with('-') {
                        target = next.clone();
                        i += 1;
                    }
                }
                args.translate = Some(target);
            }
            "--ai-mosaic" => args.ai_mosaic = true,
            "--ai-probe" => args.ai_probe = true,
            // ---- 录屏 ----
            "--record" => {
                i += 1;
                let Some(s) = argv.get(i) else {
                    die("--record 需要一个秒数，例如 --record 10");
                };
                match s.trim().parse::<u64>() {
                    Ok(v) if v > 0 => args.record = Some(v),
                    _ => die(&format!("--record 需要一个正整数秒数，收到: {s}")),
                }
            }
            "--record-out" => {
                i += 1;
                let Some(p) = argv.get(i) else {
                    die("--record-out 需要一个文件路径");
                };
                args.record_out = Some(std::path::PathBuf::from(p));
            }
            "--record-monitor" => {
                i += 1;
                let Some(s) = argv.get(i) else {
                    die("--record-monitor 需要一个显示器下标");
                };
                match s.trim().parse::<usize>() {
                    Ok(v) => args.record_monitor = Some(v),
                    Err(_) => die(&format!("--record-monitor 需要一个非负整数，收到: {s}")),
                }
            }
            "--record-fps" => {
                i += 1;
                let Some(s) = argv.get(i) else {
                    die("--record-fps 需要一个帧率，例如 30");
                };
                match s.trim().parse::<u32>() {
                    Ok(v) if (1..=120).contains(&v) => args.record_fps = Some(v),
                    _ => die(&format!("--record-fps 需要 1–120 的整数，收到: {s}")),
                }
            }
            "--record-bitrate" => {
                i += 1;
                let Some(s) = argv.get(i) else {
                    die("--record-bitrate 需要一个码率（bps），例如 8000000");
                };
                match s.trim().parse::<u32>() {
                    Ok(v) if v >= 100_000 => args.record_bitrate = Some(v),
                    _ => die(&format!("--record-bitrate 需要 ≥100000 的整数，收到: {s}")),
                }
            }
            "--record-countdown" => {
                i += 1;
                let Some(s) = argv.get(i) else {
                    die("--record-countdown 需要秒数，例如 3");
                };
                match s.trim().parse::<u32>() {
                    Ok(v) if v <= 10 => args.record_countdown = Some(v),
                    _ => die(&format!("--record-countdown 需要 0–10 的整数，收到: {s}")),
                }
            }
            "--settings" => args.open_settings = true,
            "--provider" => {
                i += 1;
                let Some(p) = argv.get(i) else {
                    die("--provider 需要一个值，例如 --provider deepseek");
                };
                match ai::Provider::parse(p) {
                    Some(pr) => args.provider = Some(pr),
                    None => {
                        let names: Vec<&str> = ai::Provider::ALL.iter().map(|p| p.name()).collect();
                        die(&format!(
                            "未知的 AI 服务方: {p}（可选：{}）",
                            names.join(" / ")
                        ));
                    }
                }
            }
            "--model" => {
                i += 1;
                match argv.get(i) {
                    Some(m) => args.model = Some(m.clone()),
                    None => die("--model 需要一个模型名，例如 --model MiniMax-M3"),
                }
            }
            "--endpoint" => {
                i += 1;
                match argv.get(i) {
                    Some(e) => args.endpoint = Some(e.clone()),
                    None => die("--endpoint 需要一个 URL"),
                }
            }
            "--image" => {
                i += 1;
                match argv.get(i) {
                    Some(p) => args.image = Some(std::path::PathBuf::from(p)),
                    None => die("--image 需要一个图片路径"),
                }
            }
            "--ai-out" => {
                i += 1;
                match argv.get(i) {
                    Some(p) => args.ai_out = Some(std::path::PathBuf::from(p)),
                    None => die("--ai-out 需要一个目录路径"),
                }
            }
            "--ai-block" => {
                i += 1;
                match argv.get(i) {
                    Some(b) => match b.trim().parse::<u32>() {
                        Ok(v) if v > 0 => args.ai_block = v,
                        _ => die("--ai-block 需要一个正整数（马赛克块边长）"),
                    },
                    None => die("--ai-block 需要一个正整数（马赛克块边长）"),
                }
            }
            other => {
                eprintln!("未知参数: {other}");
                print_help();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // OCR 独立于配置，先跑。
    if args.ocr_only {
        match ocr::from_clipboard() {
            Ok(text) => {
                println!("识别结果已复制到剪贴板:\n{text}");
                return;
            }
            Err(e) => {
                eprintln!("OCR 失败: {e}");
                std::process::exit(1);
            }
        }
    }

    // 主题覆盖文件（`--theme PATH`）：给路径就热重载配色 / 圆角
    theme::set_override_path(args.theme_path.clone());

    // 旧版配置目录（改名前）如有配置且新版缺失，则自动迁移一份过来。
    if args.cfg_path.is_none() {
        if let Some(migrated) = screenloom_core::Config::migrate_legacy_config() {
            eprintln!("已从旧版配置目录迁移配置: {}", migrated.display());
        }
    }

    let cfg_path = args
        .cfg_path
        .clone()
        .unwrap_or_else(screenloom_core::Config::default_path);
    let mut cfg = match screenloom_core::Config::load(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("读取配置失败（使用默认值）: {e}");
            screenloom_core::Config::default()
        }
    };
    if let Some(k) = args.hotkey.clone() {
        cfg.hotkey = k;
    }

    // ---- 录屏 ----
    if let Some(secs) = args.record {
        let mut opts = match record::RecordOptions::from_config(&cfg) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("录制初始化失败: {e}");
                std::process::exit(1);
            }
        };
        if let Some(idx) = args.record_monitor {
            opts.target = record::RecordTarget::Monitor(idx);
        }
        if let Some(p) = args.record_out.clone() {
            opts.out_path = p;
        }
        if let Err(e) = record::check_out_ext(&opts.out_path) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        opts.duration = Some(std::time::Duration::from_secs(secs));
        if let Some(fps) = args.record_fps {
            opts.fps = fps;
        }
        if let Some(bps) = args.record_bitrate {
            opts.bitrate = bps;
        }
        // CLI 是脚本化入口，默认不等倒计时；显式传了才等。
        opts.countdown = std::time::Duration::from_secs(
            args.record_countdown
                .unwrap_or(cfg.record_countdown.min(10)) as u64,
        );
        eprintln!(
            "开始录制 {:.0} 秒 → {}（{} fps / {} bps）",
            secs as f64,
            opts.out_path.display(),
            opts.fps,
            opts.bitrate
        );
        match record::record(&opts) {
            Ok(o) => {
                println!("已保存: {}", o.path.display());
                println!(
                    "{} 帧 / {:.1} 秒 / {:.1} MB / 目标 {} fps",
                    o.frames,
                    o.elapsed.as_secs_f64(),
                    o.bytes as f64 / 1_048_576.0,
                    opts.fps
                );
            }
            Err(e) => {
                eprintln!("录制失败: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // ---- AI 命令 ----
    if args.ask.is_some() || args.translate.is_some() || args.ai_mosaic || args.ai_probe {
        let aicfg = ai::AiConfig::resolve(
            &cfg,
            args.provider,
            args.model.as_deref(),
            args.endpoint.as_deref(),
        );
        eprintln!(
            "使用 {} / {}（{}）",
            aicfg.provider.name(),
            aicfg.model,
            aicfg.endpoint
        );

        // 连通性探针：不需要 key，用来验证 DNS/TLS/HTTP 链路。
        if args.ai_probe {
            match ai::probe(&aicfg) {
                Ok((status, body)) => {
                    println!("HTTP {status}");
                    println!("{body}");
                    if status == 401 || status == 403 {
                        println!("\n→ 链路正常，返回 401/403 说明只是 API key 不对。");
                    } else if (200..300).contains(&status) {
                        println!("\n→ 链路正常，而且 key 有效。");
                    }
                }
                Err(e) => {
                    eprintln!("探测失败: {e}");
                    std::process::exit(1);
                }
            }
            return;
        }

        if !aicfg.is_ready() {
            eprintln!("{}", aicfg.missing_key_hint());
            std::process::exit(1);
        }
        let result = if let Some(q) = &args.ask {
            aicli::run_ask(&aicfg, q, args.image.as_deref())
        } else if let Some(t) = &args.translate {
            aicli::run_translate(&aicfg, t, args.image.as_deref())
        } else {
            aicli::run_ai_mosaic(
                &aicfg,
                args.image.as_deref(),
                args.ai_out.as_deref(),
                args.ai_block,
            )
        };
        if let Err(e) = result {
            eprintln!("AI 调用失败: {e}");
            std::process::exit(1);
        }
        return;
    }

    // ---- 设置窗口 ----
    if args.open_settings {
        match settings::open(&cfg_path) {
            Ok(Some(new_cfg)) => {
                println!("设置已保存到 {}", cfg_path.display());
                // 立即生效：热键与选项改完就能用
                cfg.hotkey = new_cfg.hotkey.clone();
            }
            Ok(None) => println!("已取消。"),
            Err(e) => {
                eprintln!("设置失败: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    #[cfg(feature = "full")]
    {
        let result = if args.once {
            flow::run_capture(&cfg, args.long, args.sel, &cfg_path)
        } else {
            flow::run_hotkey_loop(&cfg, &cfg_path)
        };
        if let Err(e) = result {
            eprintln!("screenloom 退出: {e}");
            std::process::exit(1);
        }
    }

    #[cfg(not(feature = "full"))]
    {
        let _ = (args.once, args.long);
        eprintln!(
            "screenloom 当前为精简构建：抓屏层(screenloom-capture)与渲染层(screenloom-ui)尚未集成。\n\
             完整功能请用 `cargo build --features full` 构建。"
        );
    }
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

fn print_help() {
    println!(
        "screenloom {} —— 微信 4.x 截图工具的 Rust 复刻\n\n\
         用法:\n  \
         screenloom                常驻，注册全局热键（默认 Alt+A）\n  \
         screenloom --once         截一次图后退出\n  \
         screenloom --long         长截图（滚动拼接）模式\n  \
         screenloom --sel x,y,w,h  脚本化截图：直接截取指定区域（虚拟屏物理坐标）\n  \
         screenloom --ocr          识别剪贴板图片里的文字\n  \
         screenloom --record N     录屏 N 秒，输出 H.264/MP4（默认 %USERPROFILE%\\Videos\\Screenloom）\n  \
         screenloom --record-out P 指定录制输出文件（.mp4）\n  \
         screenloom --record-monitor N 录制指定显示器下标（默认主显示器）\n  \
         screenloom --record-fps N     录制帧率（1–120，默认 30）\n  \
         screenloom --record-bitrate B 录制码率（bps，默认 8000000）\n  \
         screenloom --record-countdown N 开始前倒计时秒数（0–10）\n  \
         screenloom --hotkey KEY   指定热键，如 Ctrl+Shift+A / Alt+F9\n  \
         screenloom --config PATH  指定配置文件\n  \
         screenloom --theme PATH   主题覆盖 JSON（配色/圆角，文件改了自动热重载）\n  \
         screenloom --version      显示版本\n\n\
         AI 能力（DeepSeek / MiniMax，key 走环境变量 MINIMAX_API_KEY / DEEPSEEK_API_KEY\n\
         或配置文件里的 ai_key_minimax / ai_key_deepseek）:\n  \
         screenloom --ask \"问题\"        问 AI（图片默认取剪贴板）\n  \
         screenloom --translate [语言]   翻译图里的文字（默认英文）\n  \
         screenloom --ai-mosaic         AI 一键打码（打码后存盘并复制到剪贴板）\n  \
         screenloom --image PATH        指定输入图片（默认剪贴板）\n  \
         screenloom --provider NAME     minimax（默认） / deepseek\n  \
         screenloom --model NAME        指定模型\n  \
         screenloom --endpoint URL      指定端点\n  \
         screenloom --ai-out DIR        --ai-mosaic 的输出目录\n  \
         screenloom --ai-block N        马赛克块边长（像素，默认 12）\n  \
         screenloom --ai-probe          只测端点连通性（不需要 key，用来验 DNS/TLS/HTTP）",
        env!("CARGO_PKG_VERSION")
    );
}
