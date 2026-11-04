//! 隐私模式：让本工具的**所有**窗口对屏幕捕获不可见。
//!
//! 对齐微信的 `clicfg_screen_privacy` / `gScreenPrivacyEnabled`。
//!
//! 做法：给**本进程自己**的每个顶层窗口设 `WDA_EXCLUDEFROMCAPTURE`。
//!
//! 两个必须写清楚的限制（别让人误以为能排掉别的应用的窗口）：
//!
//! 1. **跨进程无效** —— 微软规定「只有窗口拥有者进程自己」能设置该属性，
//!    对别的进程的窗口调用一定失败。所以本模式只能让**我们自己的**窗口
//!    （浮窗 / 设置窗 / 贴图窗）不被截到，排不掉别的应用。
//! 2. **会被系统静默清除** —— 显示器配置变化、DPI 切换、独占全屏进出等
//!    都会让 Windows 把这个属性悄悄清掉。所以必须**定期重申**，
//!    这里每 [`REASSERT_INTERVAL_MS`] 毫秒重申一次。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use screenloom_capture::{list_own_windows, set_window_capture_excluded};
use screenloom_core::Config;

/// 重申间隔（毫秒）。
///
/// Windows 会在各种显示器 / DPI 变化时静默清掉 `WDA_EXCLUDEFROMCAPTURE`，
/// 1 秒一次是社区常用的折中值。
pub const REASSERT_INTERVAL_MS: u64 = 1000;

/// 隐私模式守护。
///
/// 后台线程每秒检查一次配置；配置里 `privacy_mode = true` 时，
/// 把自己进程的窗口全部打上「从捕获中排除」标记。
pub struct PrivacyGuard {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PrivacyGuard {
    /// 启动守护线程。
    ///
    /// `cfg_path` 用来定期重读配置 —— 用户在设置窗口里改开关后能立刻生效，
    /// 不必重启程序。
    pub fn start(cfg_path: &Path) -> PrivacyGuard {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);
        let path: PathBuf = cfg_path.to_path_buf();
        let handle = std::thread::Builder::new()
            .name("screenloom-privacy".into())
            .spawn(move || privacy_loop(&path, &stop2))
            .ok();
        PrivacyGuard { stop, handle }
    }

    /// 停止守护线程。
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PrivacyGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 守护线程主体。
fn privacy_loop(cfg_path: &Path, stop: &AtomicBool) {
    // 记录「是我们打上排除标记的窗口」—— 关掉隐私模式时只清这些，
    // 免得把 `hide_self` 给浮窗打的标记也一起清了。
    let mut marked: HashSet<isize> = HashSet::new();
    while !stop.load(Ordering::Relaxed) {
        let enabled = Config::load(cfg_path)
            .map(|c| c.privacy_mode)
            .unwrap_or(false);
        let own = list_own_windows();
        let alive: HashSet<isize> = own.iter().map(|w| w.hwnd.0 as isize).collect();

        if enabled {
            for w in &own {
                if set_window_capture_excluded(w.hwnd, true) {
                    marked.insert(w.hwnd.0 as isize);
                }
            }
        } else {
            // 隐私模式关掉了 → 把**我们**打过的标记撤掉
            for h in marked.iter() {
                if let Some(w) = own.iter().find(|w| w.hwnd.0 as isize == *h) {
                    let _ = set_window_capture_excluded(w.hwnd, false);
                }
            }
            marked.clear();
        }
        // 已经消失的窗口要从记录里去掉
        marked.retain(|h| alive.contains(h));

        // 分段睡眠，中途能被 stop 打断
        for _ in 0..10 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(REASSERT_INTERVAL_MS / 10));
        }
    }
}
