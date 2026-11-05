//! 父进程侧的编码子进程客户端。
//!
//! 设计见 `doc/编码进程隔离方案.md`：
//! - 帧走命名共享内存（3 槽环形），控制走 stdin/stdout 的 JSON lines；
//! - 子进程卡死时 `kill` 并报错，调用方用 partial 分片文件直接 remux；
//! - 子进程是**父进程自己**（`--encoder-host`），不依赖额外分发二进制；
//! - 拉不起来时上层回退进程内编码（见 [`super::encoder::Encoder::new`]）。
//!
//! 环境变量：
//! - `SCREENLOOM_ENCODER=inproc` → 强制进程内编码（排障用）；
//! - `SCREENLOOM_ENCODER_HOST=<exe>` → 指定独立编码子进程 exe（默认不用）；
//! - `SCREENLOOM_HOST_HANG=write|finalize` → 注入卡死（自测用，子进程会继承）。

use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use screenloom_core::{Error, Result};
use screenloom_encoder_protocol::{Message, DEFAULT_SLOTS};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

/// 等子进程 `ready` 的上限。
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// 有在途帧但这么久没有任何消息 → 判定卡死。
const STALL_TIMEOUT: Duration = Duration::from_secs(3);
/// 等 `stopped` 的上限（必须小于会话看门狗的 8 秒，这样 kill 发生在看门狗之前）。
const STOP_TIMEOUT: Duration = Duration::from_secs(4);

/// 子进程侧编码器。
pub struct EncoderClient {
    child: Child,
    stdin: ChildStdin,
    shm: Shm,
    shared: Arc<Shared>,
    next_seq: u64,
    dropped: u64,
    finished: bool,
    /// 编码子进程已经死了（卡死 / 崩溃 / 被 kill）的原因。
    ///
    /// 必须单独记：只看 `finished` 分不清"正常收尾完成"和"半路被杀"，
    /// 两种情况后面的 `flush` / `finalize` 行为完全不同 ——
    /// 前者成功，后者必须报错，否则上层会以为收尾正常，
    /// 把一段被截断的录像当成功交付。
    dead_reason: Option<String>,
}

struct Shared {
    ready: AtomicBool,
    stopped: AtomicBool,
    /// 已确认的帧数（= 下一个期望收到的 seq）。单调递增。
    ///
    /// 注意这里存的是**计数**而不是"最后一个 ack 的 seq"：seq 0 既可能是
    /// "还没 ack 过"，也可能是"ack 过第 0 帧"，用 seq 表示无法区分，
    /// 会让在途帧数少算 1 帧 —— 环形槽只有 3 个，少算一帧就可能
    /// 在子进程还没取走旧帧时被覆盖。
    acked: AtomicU64,
    last_msg_ms: AtomicU64,
    error: Mutex<Option<String>>,
}

impl EncoderClient {
    pub fn new(path: &Path, w: u32, h: u32, fps: u32, bitrate: u32) -> Result<EncoderClient> {
        let frame_bytes = w * h * 3 / 2;
        let shm_name = format!(
            r"Local\ScreenloomEnc-{}-{}",
            std::process::id(),
            next_shm_id()
        );
        let shm = Shm::create(&shm_name, DEFAULT_SLOTS, frame_bytes as usize)
            .map_err(|e| Error::Encoder(format!("创建共享内存失败: {e}")))?;

        let cmd = host_command()?;
        let mut child = Command::new(&cmd.program)
            .args(&cmd.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::Encoder(format!("拉起编码子进程失败: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Encoder("编码子进程 stdin 不可用".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Encoder("编码子进程 stdout 不可用".into()))?;
        register_host(child.id());

        let shared = Arc::new(Shared {
            ready: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            acked: AtomicU64::new(0),
            last_msg_ms: AtomicU64::new(now_ms()),
            error: Mutex::new(None),
        });

        // 读线程：只更新共享状态，绝不阻塞录制线程。
        {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("screenloom-encoder-reader".into())
                .spawn(move || {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        let Ok(line) = line else { break };
                        shared.last_msg_ms.store(now_ms(), Ordering::Release);
                        match Message::decode(line.trim_end()) {
                            Ok(Message::Ack { seq }) => {
                                // 记成"已确认帧数"：seq 是 0 基的，所以 +1。
                                let _ = shared.acked.fetch_update(
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                    |cur| Some(cur.max(seq + 1)),
                                );
                            }
                            Ok(Message::Stopped { .. }) => {
                                shared.stopped.store(true, Ordering::Release);
                            }
                            Ok(Message::Ready { .. }) => {
                                shared.ready.store(true, Ordering::Release);
                            }
                            Ok(Message::Error { code, msg }) => {
                                *shared.error.lock().unwrap_or_else(|e| e.into_inner()) =
                                    Some(format!("编码子进程错误 [{code}]: {msg}"));
                            }
                            Ok(_) => {}
                            Err(e) => {
                                *shared.error.lock().unwrap_or_else(|e| e.into_inner()) =
                                    Some(format!("编码子进程协议错误: {e}"));
                            }
                        }
                    }
                })
                .map_err(|e| Error::Encoder(format!("启动编码读线程失败: {e}")))?;
        }

        let mut client = EncoderClient {
            child,
            stdin,
            shm,
            shared,
            next_seq: 0,
            dropped: 0,
            finished: false,
            dead_reason: None,
        };

        client.send(&Message::Init {
            w,
            h,
            fps,
            bitrate,
            path: path
                .to_str()
                .ok_or_else(|| Error::Encoder("录制路径不是合法 UTF-8".into()))?
                .to_string(),
            shm: shm_name,
            slots: DEFAULT_SLOTS,
            frame_bytes,
        })?;

        // 等 ready（或错误）。
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Some(e) = client.take_error() {
                client.kill_with(e.clone());
                return Err(Error::Encoder(e));
            }
            if client.shared.ready.load(Ordering::Acquire) {
                break;
            }
            if Instant::now() >= deadline {
                client.kill_with("编码子进程初始化超时".to_string());
                return Err(Error::Encoder("编码子进程初始化超时".into()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(client)
    }

    /// 送一帧 NV12。子进程在途槽满时丢帧（计数），绝不阻塞录制线程。
    pub fn write_frame(&mut self, nv12: &[u8], pts_100ns: i64) -> Result<()> {
        if self.finished {
            return Err(self.dead_error("已收尾"));
        }
        if let Some(e) = self.take_error() {
            self.kill_with(e.clone());
            return Err(Error::Encoder(e));
        }
        let outstanding = self.outstanding();
        if outstanding > 0
            && now_ms().saturating_sub(self.shared.last_msg_ms.load(Ordering::Acquire))
                >= STALL_TIMEOUT.as_millis() as u64
        {
            self.kill_with("无响应（心跳超时）".to_string());
            return Err(Error::Encoder("无响应（心跳超时）".into()));
        }

        // 槽满：先等一小会儿让 ack 追上；仍满就丢这一帧。
        if outstanding >= DEFAULT_SLOTS as u64 {
            let deadline = Instant::now() + Duration::from_millis(4);
            while Instant::now() < deadline {
                if self.outstanding() < DEFAULT_SLOTS as u64 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if self.outstanding() >= DEFAULT_SLOTS as u64 {
                self.dropped += 1;
                return Ok(());
            }
        }

        let slot = (self.next_seq % DEFAULT_SLOTS as u64) as u32;
        self.shm.write_slot(slot, nv12);
        let seq = self.next_seq;
        if let Err(e) = self.send(&Message::Frame {
            seq,
            slot,
            pts: pts_100ns,
            size: nv12.len() as u32,
        }) {
            // 管道写失败 = 子进程已经没了，必须收掉，别留孤儿。
            let msg = e.to_string();
            self.kill_with(msg.clone());
            return Err(Error::Encoder(msg));
        }
        self.next_seq += 1;
        Ok(())
    }

    /// 等所有已发帧被 ack（不触发收尾）。
    pub fn flush(&mut self) -> Result<()> {
        if self.finished {
            return Err(self.dead_error("已收尾"));
        }
        if let Some(e) = self.take_error() {
            self.kill_with(e.clone());
            return Err(Error::Encoder(e));
        }
        let deadline = Instant::now() + STALL_TIMEOUT;
        while self.outstanding() > 0 {
            if let Some(e) = self.take_error() {
                self.kill_with(e.clone());
                return Err(Error::Encoder(e));
            }
            if Instant::now() >= deadline {
                self.kill_with("Flush 超时（无响应）".to_string());
                return Err(Error::Encoder("Flush 超时（无响应）".into()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    /// 发 `stop` 并等 `stopped`；超时则 kill 并报错（partial 仍可用）。
    pub fn finalize(&mut self) -> Result<()> {
        if self.finished {
            return Err(self.dead_error("已收尾"));
        }
        if let Some(e) = self.take_error() {
            self.kill_with(e.clone());
            return Err(Error::Encoder(e));
        }
        if let Err(e) = self.send(&Message::Stop) {
            let msg = e.to_string();
            self.kill_with(msg.clone());
            return Err(Error::Encoder(msg));
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            if let Some(e) = self.take_error() {
                self.kill_with(e.clone());
                return Err(Error::Encoder(e));
            }
            if self.shared.stopped.load(Ordering::Acquire) {
                self.finished = true;
                let pid = self.child.id();
                let _ = self.child.wait();
                unregister_host(pid);
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.kill_with("收尾超时（无响应）".to_string());
                return Err(Error::Encoder("收尾超时（无响应）".into()));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn frames_written(&self) -> u64 {
        self.next_seq
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 在途帧数（已发出但还没被子进程确认）。
    fn outstanding(&self) -> u64 {
        self.next_seq
            .saturating_sub(self.shared.acked.load(Ordering::Acquire))
    }

    fn send(&mut self, m: &Message) -> Result<()> {
        writeln!(self.stdin, "{}", m.encode())
            .map_err(|e| Error::Encoder(format!("写编码子进程失败: {e}")))?;
        self.stdin
            .flush()
            .map_err(|e| Error::Encoder(format!("刷新编码子进程管道失败: {e}")))
    }

    fn take_error(&self) -> Option<String> {
        self.shared
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// 杀掉编码子进程，并记住原因。
    fn kill_with(&mut self, reason: String) {
        let pid = self.child.id();
        let _ = self.child.kill();
        let _ = self.child.wait();
        unregister_host(pid);
        self.finished = true;
        if self.dead_reason.is_none() {
            self.dead_reason = Some(reason);
        }
    }

    /// 编码器已经不可用时的错误描述。
    fn dead_error(&self, fallback: &str) -> Error {
        match &self.dead_reason {
            Some(r) => Error::Encoder(r.clone()),
            None => Error::Encoder(fallback.to_string()),
        }
    }
}

impl Drop for EncoderClient {
    fn drop(&mut self) {
        if !self.finished {
            // 正常路径已 finalize；异常路径必须收掉子进程，避免孤儿。
            let pid = self.child.id();
            let _ = self.child.kill();
            let _ = self.child.wait();
            unregister_host(pid);
        }
        unregister_host(self.child.id());
    }
}

/// 编码子进程的启动方式。
struct HostCommand {
    program: PathBuf,
    args: Vec<String>,
}

/// 决定怎么拉起编码子进程。
///
/// 默认是**拉起自己**（`screenloom.exe --encoder-host`）：任何分发形态下
/// 都保证有编码子进程可用，不依赖"旁边恰好有个 host.exe"。
/// `SCREENLOOM_ENCODER_HOST` 可以显式指向独立 exe（调试 / 打包场景）。
fn host_command() -> Result<HostCommand> {
    if let Some(p) = std::env::var_os("SCREENLOOM_ENCODER_HOST") {
        let p = PathBuf::from(p);
        if !p.exists() {
            return Err(Error::Encoder(format!(
                "SCREENLOOM_ENCODER_HOST 指向的文件不存在: {}",
                p.display()
            )));
        }
        return Ok(HostCommand {
            program: p,
            args: Vec::new(),
        });
    }
    let exe =
        std::env::current_exe().map_err(|e| Error::Encoder(format!("取当前程序路径失败: {e}")))?;
    Ok(HostCommand {
        program: exe,
        args: vec!["--encoder-host".to_string()],
    })
}

fn next_shm_id() -> u64 {
    use std::sync::atomic::AtomicU64 as Id;
    static ID: Id = Id::new(0);
    ID.fetch_add(1, Ordering::Relaxed)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 在册的子进程 PID。看门狗中止会话时要把它们一起收掉。
static HOSTS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn register_host(pid: u32) {
    HOSTS.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
}

fn unregister_host(pid: u32) {
    HOSTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|p| *p != pid);
}

/// 强杀所有在册的编码子进程。
///
/// 看门狗判定卡死时会先调这个：子进程正卡在 MF 里，读不到 stdin EOF，
/// 如果父进程先退出就会留下孤儿进程占着 `.partial.mp4`。
pub fn kill_all_hosts() {
    let pids: Vec<u32> = {
        let mut guard = HOSTS.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    };
    for pid in pids {
        terminate_process(pid);
    }
}

fn terminate_process(pid: u32) {
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    // SAFETY: 句柄由 OpenProcess 返回，用完即关。
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
        }
    }
}

/// 父进程侧共享内存（只创建 + 写）。
struct Shm {
    handle: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    slot_size: usize,
}

impl Shm {
    fn create(name: &str, slots: u32, slot_size: usize) -> std::result::Result<Shm, String> {
        let total = slots as usize * slot_size;
        let wname: Vec<u16> = OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                total as u32,
                PCWSTR(wname.as_ptr()),
            )
        }
        .map_err(|e| format!("CreateFileMapping: {e}"))?;
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, total) };
        if view.Value.is_null() {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err("MapViewOfFile 返回空指针".into());
        }
        Ok(Shm {
            handle,
            view,
            slot_size,
        })
    }

    fn write_slot(&self, slot: u32, data: &[u8]) {
        debug_assert!(data.len() <= self.slot_size);
        // SAFETY: 调用方保证槽位与长度；映射区为 slots*slot_size。
        unsafe {
            let dst = (self.view.Value as *mut u8).add(slot as usize * self.slot_size);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
        }
    }
}
