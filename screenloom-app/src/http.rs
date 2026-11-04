//! 最小 HTTPS 客户端：走 Windows 自带的 **WinHTTP**。
//!
//! **为什么不用第三方 HTTP crate**：TLS 后端要么引 `aws-lc-rs`/`ring`
//! （要 C 编译器 + cmake + nasm，机器上没装完整 MSVC C 工具链时会直接编译失败），
//! 要么引 `native-tls`（还得挑证书后端、绑死 Schannel/OpenSSL）。
//! WinHTTP 是系统自带、用**系统证书库**，本项目因此**不新增任何 HTTP 依赖**。
//!
//! 只实现真正用得到的一小块：`POST` 一个 JSON 出去、把 JSON 文本拿回来。

use std::ffi::c_void;

use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_FLAG_SECURE_DEFAULTS, WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_CONNECT_TIMEOUT,
    WINHTTP_OPTION_RECEIVE_TIMEOUT, WINHTTP_OPTION_SEND_TIMEOUT, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE,
};

use screenloom_core::{Error, Result};

/// HTTP 响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// HTTP 状态码（200 / 401 / 429 …）。
    pub status: u16,
    /// 响应体（UTF-8 文本）。
    pub body: String,
}

impl HttpResponse {
    /// 2xx 视为成功。
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// 解析后的 URL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// 是否 https。
    pub secure: bool,
    /// 主机名（不含端口）。
    pub host: String,
    /// 端口。
    pub port: u16,
    /// 路径 + 查询串，不含主机部分；至少是 `/`。
    pub path_and_query: String,
}

impl Url {
    /// 解析 `https://host[:port]/path?query`。
    ///
    /// 只支持 http/https；不支持 userinfo 之外的怪写法（足够覆盖两个 API 端点）。
    pub fn parse(raw: &str) -> Result<Url> {
        let s = raw.trim();
        let (secure, rest) = if let Some(r) = s.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = s.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(Error::Network(format!(
                "不支持的 URL（仅 http/https）：{s}"
            )));
        };

        let (hostport, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        // 去掉 userinfo（api key 不该出现在 URL 里，但别因此解析失败）
        let hostport = hostport.rsplit('@').next().unwrap_or(hostport);

        let default_port = if secure { 443 } else { 80 };
        let (host, port) = match hostport.rfind(':') {
            Some(i) => {
                let p = hostport[i + 1..].parse::<u16>().unwrap_or(default_port);
                (&hostport[..i], p)
            }
            None => (hostport, default_port),
        };
        if host.is_empty() {
            return Err(Error::Network(format!("URL 缺少主机名：{s}")));
        }
        Ok(Url {
            secure,
            host: host.to_string(),
            port,
            path_and_query: path.to_string(),
        })
    }
}

/// 发送一个 `POST`（JSON 体），返回状态码与响应文本。
///
/// `headers` 是完整的请求头列表（如 `Content-Type`、`Authorization`）。
pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &[u8],
    timeout_secs: u32,
) -> Result<HttpResponse> {
    let u = Url::parse(url)?;

    // 会话
    // SAFETY: 句柄由 Handle 守卫负责关闭；参数均为合法的 WinHTTP 值。
    let session = unsafe {
        WinHttpOpen(
            w!("screenloom"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            WINHTTP_FLAG_SECURE_DEFAULTS,
        )
    };
    if session.is_null() {
        return Err(Error::Network("WinHttpOpen 失败（会话创建失败）".into()));
    }
    let session = Handle(session);

    // 超时（WinHTTP 用毫秒）
    let timeout = timeout_secs.max(1).saturating_mul(1000).to_le_bytes();
    unsafe {
        let _ = WinHttpSetOption(
            Some(session.0),
            WINHTTP_OPTION_CONNECT_TIMEOUT,
            Some(&timeout),
        );
        let _ = WinHttpSetOption(Some(session.0), WINHTTP_OPTION_SEND_TIMEOUT, Some(&timeout));
        let _ = WinHttpSetOption(
            Some(session.0),
            WINHTTP_OPTION_RECEIVE_TIMEOUT,
            Some(&timeout),
        );
    }

    // 连接
    let host_w: Vec<u16> = u.host.encode_utf16().chain(std::iter::once(0)).collect();
    let connect = unsafe { WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), u.port, 0) };
    if connect.is_null() {
        return Err(Error::Network(format!("WinHttpConnect 失败：{}", u.host)));
    }
    let connect = Handle(connect);

    // 请求
    let path_w: Vec<u16> = u
        .path_and_query
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let flags = if u.secure {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };
    let request = unsafe {
        WinHttpOpenRequest(
            connect.0,
            w!("POST"),
            PCWSTR(path_w.as_ptr()),
            w!("HTTP/1.1"),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        )
    };
    if request.is_null() {
        return Err(Error::Network("WinHttpOpenRequest 失败".into()));
    }
    let request = Handle(request);

    // 附加请求头：每行 "Key: Value\r\n"，末尾再加一个空行表示「头结束」。
    // 内存里补两个 NUL 作终止符，但**长度不能算进去**——
    // WinHttpSendRequest 的 dwHeadersLength 是「字符数」，
    // 把终止符算进去会让 WinHTTP 多读 2 个字符并返回 ERROR_INVALID_PARAMETER(0x80070057)。
    let mut head: Vec<u16> = Vec::new();
    for (k, v) in headers {
        head.extend(format!("{k}: {v}\r\n").encode_utf16());
    }
    head.extend([b'\r' as u16, b'\n' as u16]);
    head.push(0);
    head.push(0);
    let head_text = &head[..head.len() - 2];

    // SAFETY: 缓冲区均为本次调用内的活对象，长度与实际一致。
    unsafe {
        WinHttpSendRequest(
            request.0,
            Some(head_text),
            Some(body.as_ptr() as *const c_void),
            body.len() as u32,
            body.len() as u32,
            0,
        )
    }
    .map_err(|e| Error::Network(format!("发送请求失败：{e}")))?;

    // SAFETY: 同上。
    unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }
        .map_err(|e| Error::Network(format!("接收响应失败：{e}")))?;

    let status = unsafe { query_status(&request) };

    let mut buf: Vec<u8> = Vec::new();
    loop {
        let mut avail: u32 = 0;
        // SAFETY: avail 是合法指针。
        unsafe { WinHttpQueryDataAvailable(request.0, &mut avail) }
            .map_err(|e| Error::Network(format!("查询数据长度失败：{e}")))?;
        if avail == 0 {
            break;
        }
        let mut chunk = vec![0u8; avail as usize];
        let mut read: u32 = 0;
        // SAFETY: chunk 至少有 avail 字节；read 是合法指针。
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr() as *mut c_void,
                avail,
                &mut read,
            )
        }
        .map_err(|e| Error::Network(format!("读取响应体失败：{e}")))?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read as usize]);
    }

    Ok(HttpResponse {
        status,
        body: String::from_utf8_lossy(&buf).into_owned(),
    })
}

/// 取 HTTP 状态码。
unsafe fn query_status(request: &Handle) -> u16 {
    let mut code: u32 = 0;
    let mut len: u32 = std::mem::size_of::<u32>() as u32;
    let mut index: u32 = 0;
    let ok = WinHttpQueryHeaders(
        request.0,
        WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
        PCWSTR::null(),
        Some(&mut code as *mut u32 as *mut c_void),
        &mut len,
        &mut index,
    );
    if ok.is_ok() {
        code as u16
    } else {
        0
    }
}

/// WinHTTP 句柄守卫：析构时关闭。
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: 句柄由本 guard 独占。
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_url() {
        let u = Url::parse("https://api.deepseek.com/chat/completions").unwrap();
        assert!(u.secure);
        assert_eq!(u.host, "api.deepseek.com");
        assert_eq!(u.port, 443);
        assert_eq!(u.path_and_query, "/chat/completions");
    }

    #[test]
    fn parses_explicit_port_and_query() {
        let u = Url::parse("https://api.minimax.cn:8443/v1/chat/completions?x=1").unwrap();
        assert_eq!(u.port, 8443);
        assert_eq!(u.path_and_query, "/v1/chat/completions?x=1");
    }

    #[test]
    fn no_path_becomes_root() {
        let u = Url::parse("https://example.com").unwrap();
        assert_eq!(u.path_and_query, "/");
        assert_eq!(u.port, 443);
    }

    #[test]
    fn http_defaults_to_port_80() {
        let u = Url::parse("http://example.com/x").unwrap();
        assert!(!u.secure);
        assert_eq!(u.port, 80);
    }

    #[test]
    fn rejects_unknown_scheme() {
        assert!(Url::parse("ftp://example.com").is_err());
        assert!(Url::parse("example.com").is_err());
    }
}
