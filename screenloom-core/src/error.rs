//! 错误类型。

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// 捕获失败（WGC 不可用、被保护窗口、帧池创建失败等）
    Capture(String),
    /// 渲染/窗口相关失败
    Window(String),
    /// 图像编解码失败
    Image(String),
    /// 配置读写失败
    Config(String),
    /// 功能不可用（如未实现的 AI 能力）
    Unsupported(String),
    /// 网络/远端服务失败（HTTP、模型 API 等）
    Network(String),
    /// 音频设备/采集失败
    Audio(String),
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Capture(m) => write!(f, "捕获失败: {m}"),
            Error::Window(m) => write!(f, "窗口失败: {m}"),
            Error::Image(m) => write!(f, "图像失败: {m}"),
            Error::Config(m) => write!(f, "配置失败: {m}"),
            Error::Unsupported(m) => write!(f, "功能不可用: {m}"),
            Error::Network(m) => write!(f, "网络失败: {m}"),
            Error::Audio(m) => write!(f, "音频失败: {m}"),
            Error::Io(e) => write!(f, "IO 失败: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<png::EncodingError> for Error {
    fn from(e: png::EncodingError) -> Self {
        Error::Image(e.to_string())
    }
}

impl From<png::DecodingError> for Error {
    fn from(e: png::DecodingError) -> Self {
        Error::Image(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
