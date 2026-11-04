//! AI 接入：DeepSeek / MiniMax。
//!
//! 两家都提供 **OpenAI 兼容**的 `POST {base}/chat/completions`：
//! 请求体形状一致（`model` + `messages[].content` 数组 + `image_url`），
//! 响应体也一致（`choices[0].message.content`）。
//! 所以这里只用**一套**客户端，按 Provider 切 endpoint + model。
//!
//! 端点与模型（据两家官方文档）：
//! | Provider | endpoint | 默认模型 | key 环境变量 |
//! |---|---|---|---|
//! | `minimax` | `https://api.minimax.cn/v1/chat/completions` | `MiniMax-M3` | `MINIMAX_API_KEY` |
//! | `deepseek` | `https://api.deepseek.com/chat/completions` | `deepseek-flash` | `DEEPSEEK_API_KEY` |
//!
//! **不硬编码任何 key**：来源顺序 = 环境变量 → 配置文件。
//!
//! 依赖：只加 `serde_json`（请求体构造/响应解析）与 `base64`（图片内联），
//! HTTP 传输走 Windows 自带 WinHTTP（见 `http.rs`），不引第三方 TLS 栈。

use base64::Engine as _;
use serde_json::{json, Value};
use screenloom_core::{Error, Result};

use crate::http::post_json;

/// 支持的模型服务方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// DeepSeek（`deepseek-flash` 支持图文）
    DeepSeek,
    /// MiniMax（`MiniMax-M3` 支持图文）
    MiniMax,
}

impl Provider {
    /// 全部可选值。
    pub const ALL: [Provider; 2] = [Provider::DeepSeek, Provider::MiniMax];

    /// 从字符串解析（大小写不敏感，容忍 `ds` / `mm` 简写）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "deepseek" | "ds" => Some(Provider::DeepSeek),
            "minimax" | "mm" => Some(Provider::MiniMax),
            _ => None,
        }
    }

    /// 稳定标识（写进配置文件的值）。
    pub fn name(&self) -> &'static str {
        match self {
            Provider::DeepSeek => "deepseek",
            Provider::MiniMax => "minimax",
        }
    }

    /// OpenAI 兼容的 chat/completions 端点。
    pub fn default_endpoint(&self) -> &'static str {
        match self {
            Provider::DeepSeek => "https://api.deepseek.com/chat/completions",
            Provider::MiniMax => "https://api.minimax.cn/v1/chat/completions",
        }
    }

    /// 默认模型（默认都选**能看图**的，否则「问 AI」传图会直接 400）。
    pub fn default_model(&self) -> &'static str {
        match self {
            Provider::DeepSeek => "deepseek-flash",
            Provider::MiniMax => "MiniMax-M3",
        }
    }

    /// API key 的环境变量名。
    pub fn env_key_name(&self) -> &'static str {
        match self {
            Provider::DeepSeek => "DEEPSEEK_API_KEY",
            Provider::MiniMax => "MINIMAX_API_KEY",
        }
    }

    /// 该 provider 的请求体是否带 MiniMax 专属的 `thinking` 字段。
    fn supports_thinking(&self, model: &str) -> bool {
        matches!(self, Provider::MiniMax) && model.to_ascii_lowercase().contains("minimax")
    }
}

/// 待发送的图片（PNG 字节）。
#[derive(Clone, Debug)]
pub struct ImagePayload {
    /// MIME 类型，如 `image/png`。
    pub mime: String,
    /// 编码后的图片字节。
    pub data: Vec<u8>,
}

impl ImagePayload {
    /// 用 PNG 字节构造。
    pub fn png(data: Vec<u8>) -> Self {
        ImagePayload {
            mime: "image/png".to_string(),
            data,
        }
    }
}

/// AI 相关配置。
#[derive(Clone, Debug)]
pub struct AiConfig {
    /// 服务方。
    pub provider: Provider,
    /// 模型名。
    pub model: String,
    /// API key（空表示没配）。
    pub api_key: String,
    /// 端点（空表示用 provider 默认）。
    pub endpoint: String,
    /// 超时（秒）。
    pub timeout_secs: u32,
}

impl Default for AiConfig {
    fn default() -> Self {
        AiConfig {
            provider: Provider::MiniMax,
            model: Provider::MiniMax.default_model().to_string(),
            api_key: String::new(),
            endpoint: String::new(),
            timeout_secs: 120,
        }
    }
}

impl AiConfig {
    /// 从环境变量 + 配置文件 + 命令行覆盖解析出最终配置。
    ///
    /// 优先级（后者覆盖前者）：provider 默认 → 配置文件 → 环境变量 → 命令行覆盖。
    pub fn resolve(
        cfg: &screenloom_core::Config,
        provider_override: Option<Provider>,
        model_override: Option<&str>,
        endpoint_override: Option<&str>,
    ) -> AiConfig {
        let provider = provider_override
            .or_else(|| Provider::parse(&cfg.ai_provider))
            .unwrap_or(Provider::MiniMax);

        let model = model_override
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                let m = cfg.ai_model.trim();
                if m.is_empty() {
                    None
                } else {
                    Some(m.to_string())
                }
            })
            .unwrap_or_else(|| provider.default_model().to_string());

        let endpoint = endpoint_override
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                // 配置文件里的端点只在「配置指定的 provider == 实际生效的 provider」时用。
                // 否则命令行切了 provider 却仍拿旧 provider 的端点 → key 与端点不匹配 → 401。
                let cfg_provider = Provider::parse(&cfg.ai_provider);
                if cfg_provider == Some(provider) {
                    let e = cfg.ai_endpoint.trim();
                    if !e.is_empty() {
                        return Some(e.to_string());
                    }
                }
                None
            })
            .unwrap_or_else(|| provider.default_endpoint().to_string());

        let api_key = std::env::var(provider.env_key_name())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let per = match provider {
                    Provider::DeepSeek => cfg.ai_key_deepseek.trim(),
                    Provider::MiniMax => cfg.ai_key_minimax.trim(),
                };
                if per.is_empty() {
                    None
                } else {
                    Some(per.to_string())
                }
            })
            .or_else(|| {
                let g = cfg.ai_api_key.trim();
                if g.is_empty() {
                    None
                } else {
                    Some(g.to_string())
                }
            })
            .unwrap_or_default();

        AiConfig {
            provider,
            model,
            api_key,
            endpoint,
            timeout_secs: if cfg.ai_timeout == 0 {
                120
            } else {
                cfg.ai_timeout
            },
        }
    }

    /// 是否具备调用条件（有 key）。
    pub fn is_ready(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// 缺 key 时给用户的提示。
    pub fn missing_key_hint(&self) -> String {
        format!(
            "未找到 {} 的 API key。请设置环境变量 {} 或在配置文件里写 ai_key_{} = <你的key>。",
            self.provider.name(),
            self.provider.env_key_name(),
            self.provider.name()
        )
    }
}

/// AI 客户端。
#[derive(Clone, Debug)]
pub struct AiClient {
    /// 生效的配置。
    pub cfg: AiConfig,
}

impl AiClient {
    /// 用给定配置构造。
    pub fn new(cfg: AiConfig) -> Self {
        AiClient { cfg }
    }

    /// 发送一次对话请求（可选带图），返回模型回复文本。
    pub fn chat(&self, prompt: &str, image: Option<&ImagePayload>) -> Result<String> {
        if !self.cfg.is_ready() {
            return Err(Error::Network(self.cfg.missing_key_hint()));
        }
        let body = build_request_body(
            self.cfg.provider,
            &self.cfg.model,
            None,
            prompt,
            image.map(|i| (i.mime.as_str(), i.data.as_slice())),
        );
        let resp = post_json(
            &self.cfg.endpoint,
            &[
                ("Content-Type", "application/json"),
                ("Authorization", &format!("Bearer {}", self.cfg.api_key)),
            ],
            body.as_bytes(),
            self.cfg.timeout_secs,
        )?;
        if !resp.is_ok() {
            return Err(Error::Network(describe_error(resp.status, &resp.body)));
        }
        parse_completion(&resp.body)
    }

    /// 问 AI（带图的通用提问）。
    pub fn ask(&self, prompt: &str, image: Option<&ImagePayload>) -> Result<String> {
        self.chat(prompt, image)
    }

    /// 翻译文本（走本地 OCR 得到的原文 → 目标语言）。
    pub fn translate(&self, text: &str, target: &str) -> Result<String> {
        let prompt = format!(
            "请把下面的文本翻译成{target}。只输出译文本身，不要加引号、解释或原文复述。\n\n文本：\n{text}"
        );
        self.chat(&prompt, None)
    }

    /// AI 一键打码：让模型给出需要打码的区域。
    pub fn detect_mosaic(&self, image: &ImagePayload) -> Result<Vec<MosaicRegion>> {
        let raw = self.chat(MOSAIC_PROMPT, Some(image))?;
        parse_regions(&raw)
    }
}

/// AI 一键打码的提示词。
pub const MOSAIC_PROMPT: &str = "请找出这张截图中所有需要打码的敏感信息区域，例如：人脸、身份证/护照/驾照/工牌等证件、车牌、门禁卡、密码与密钥、二维码、条形码、邮箱、手机号、姓名、住址、银行卡号。\
只输出一个 JSON 对象，不要任何解释、不要用 markdown 代码块。格式：\
{\"regions\":[{\"x\":0.12,\"y\":0.34,\"w\":0.18,\"h\":0.22,\"label\":\"人脸\"}]}\
其中 x/y/w/h 是相对整张图的归一化坐标（0 到 1，左上角为原点，w/h 是宽高）。\
如果没有任何需要打码的区域，输出 {\"regions\":[]}。";

/// 端点联通性探针。
///
/// 发一个**最小**的文本请求（不带图），返回 `(HTTP 状态码, 响应体前 400 字)`。
///
/// 用途：在没有真 key 也能验证「DNS 解析 + TLS 握手 + HTTP 请求」整条链路是通的——
/// 此时应该拿到 **401**（鉴权失败），而不是超时或连接错误。
/// 拿到 401 就说明剩下的问题只是 key。
pub fn probe(cfg: &AiConfig) -> Result<(u16, String)> {
    let key = if cfg.api_key.trim().is_empty() {
        "sk-probe-invalid-key".to_string()
    } else {
        cfg.api_key.clone()
    };
    let body = build_request_body(cfg.provider, &cfg.model, None, "ping", None);
    let resp = post_json(
        &cfg.endpoint,
        &[
            ("Content-Type", "application/json"),
            ("Authorization", &format!("Bearer {key}")),
        ],
        body.as_bytes(),
        cfg.timeout_secs.max(5),
    )?;
    Ok((resp.status, truncate(&resp.body, 400)))
}

/// 构造 OpenAI 兼容的请求体（纯函数，便于单测）。
pub fn build_request_body(
    provider: Provider,
    model: &str,
    system: Option<&str>,
    prompt: &str,
    image: Option<(&str, &[u8])>,
) -> String {
    let mut content: Vec<Value> = Vec::new();
    if let Some((mime, bytes)) = image {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        content.push(json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:{mime};base64,{b64}"),
                "detail": "high"
            }
        }));
    }
    content.push(json!({ "type": "text", "text": prompt }));

    let mut messages: Vec<Value> = Vec::new();
    if let Some(s) = system {
        messages.push(json!({ "role": "system", "content": s }));
    }
    messages.push(json!({ "role": "user", "content": content }));

    let mut root = json!({
        "model": model,
        "messages": messages,
        "stream": false,
    });

    // MiniMax 专属：思考模式开关（M3 支持 adaptive / disabled）。
    if provider.supports_thinking(model) {
        root["thinking"] = json!({ "type": "adaptive" });
    }

    root.to_string()
}

/// 从 OpenAI 兼容响应里取出 `choices[0].message.content`。
pub fn parse_completion(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body).map_err(|e| {
        Error::Network(format!("响应不是合法 JSON（{e}）：{}", truncate(body, 300)))
    })?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"));

    match content {
        Some(Value::String(s)) => Ok(s.clone()),
        // 少数模型把 content 拆成数组
        Some(Value::Array(items)) => {
            let mut out = String::new();
            for it in items {
                if it.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = it.get("text").and_then(|t| t.as_str()) {
                        out.push_str(t);
                    }
                }
            }
            if out.is_empty() {
                Err(Error::Network(format!(
                    "响应里没有文本内容：{}",
                    truncate(body, 300)
                )))
            } else {
                Ok(out)
            }
        }
        _ => Err(Error::Network(format!(
            "响应里没有 choices[0].message.content：{}",
            truncate(body, 300)
        ))),
    }
}

/// 把 HTTP 错误响应翻译成人话。
pub fn describe_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            // OpenAI / DeepSeek
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                // MiniMax
                .or_else(|| {
                    v.get("base_resp")
                        .and_then(|b| b.get("status_msg"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
                .or_else(|| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
        })
        .unwrap_or_else(|| truncate(body, 300));

    let hint = match status {
        401 | 403 => "（API key 无效或没有权限）",
        402 => "（余额不足）",
        429 => "（触发限流，稍后重试）",
        400 => "（请求参数不被接受，常见于模型不支持图片或参数写错）",
        _ => "",
    };
    format!("HTTP {status}{hint}: {detail}")
}

/// 一个需要打码的区域（模型给出的原始坐标）。
#[derive(Clone, Debug, PartialEq)]
pub struct MosaicRegion {
    /// 左上角 x。
    pub x: f32,
    /// 左上角 y。
    pub y: f32,
    /// 宽。
    pub w: f32,
    /// 高。
    pub h: f32,
    /// 区域标签（模型给的中文说明）。
    pub label: String,
    /// 坐标是否已归一化到 0..1。
    pub normalized: bool,
}

impl MosaicRegion {
    /// 映射到图像像素坐标，并裁剪进图像范围。
    pub fn to_rect(&self, img_w: u32, img_h: u32) -> screenloom_core::Rect {
        let (fw, fh) = (img_w as f32, img_h as f32);
        let (x, y, w, h) = if self.normalized {
            (self.x * fw, self.y * fh, self.w * fw, self.h * fh)
        } else {
            (self.x, self.y, self.w, self.h)
        };
        let x = x.clamp(0.0, fw.max(0.0));
        let y = y.clamp(0.0, fh.max(0.0));
        let w = w.clamp(0.0, fw - x);
        let h = h.clamp(0.0, fh - y);
        screenloom_core::Rect::new(x as i32, y as i32, w as i32, h as i32).normalized()
    }
}

/// 从模型回复里解析打码区域。
///
/// 容错点：
/// - 模型可能把 JSON 包在 ```json 代码块里；
/// - 可能直接给顶层数组，而不是 `{"regions":[...]}`；
/// - 坐标键名可能是 `x/y/w/h`、`left/top/width/height` 或 `bbox:[x,y,w,h]`；
/// - 坐标可能是归一化 0..1，也可能是像素值——用「四个值都 ≤ 1」来区分。
pub fn parse_regions(text: &str) -> Result<Vec<MosaicRegion>> {
    let json_text = extract_json_block(text)
        .ok_or_else(|| Error::Network(format!("模型没有返回 JSON：{}", truncate(text, 300))))?;
    let v: Value = serde_json::from_str(&json_text).map_err(|e| {
        Error::Network(format!(
            "模型返回的 JSON 无法解析（{e}）：{}",
            truncate(&json_text, 300)
        ))
    })?;

    let arr: &Vec<Value> = match &v {
        Value::Array(a) => a,
        Value::Object(_) => v
            .get("regions")
            .or_else(|| v.get("boxes"))
            .or_else(|| v.get("results"))
            .and_then(|r| r.as_array())
            .ok_or_else(|| {
                Error::Network(format!(
                    "模型返回的 JSON 里没有 regions 数组：{}",
                    truncate(&json_text, 300)
                ))
            })?,
        _ => {
            return Err(Error::Network(format!(
                "模型返回的 JSON 结构无法识别：{}",
                truncate(&json_text, 300)
            )))
        }
    };

    let mut out = Vec::new();
    for item in arr {
        let Some(reg) = parse_one_region(item) else {
            continue;
        };
        out.push(reg);
    }
    Ok(out)
}

fn parse_one_region(item: &Value) -> Option<MosaicRegion> {
    // 形态一：{"bbox":[x,y,w,h], "label":"..."}
    // 形态二：{"x":..,"y":..,"w":..,"h":..}
    // 形态三：{"left":..,"top":..,"width":..,"height":..}
    let (x, y, w, h) = if let Some(b) = item.get("bbox").and_then(|b| b.as_array()) {
        if b.len() < 4 {
            return None;
        }
        (
            as_f32(&b[0])?,
            as_f32(&b[1])?,
            as_f32(&b[2])?,
            as_f32(&b[3])?,
        )
    } else if let (Some(x), Some(y), Some(w), Some(h)) = (
        item.get("x").or_else(|| item.get("left")),
        item.get("y").or_else(|| item.get("top")),
        item.get("w").or_else(|| item.get("width")),
        item.get("h").or_else(|| item.get("height")),
    ) {
        (as_f32(x)?, as_f32(y)?, as_f32(w)?, as_f32(h)?)
    } else {
        return None;
    };

    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
        return None;
    }
    if w <= 0.0 || h <= 0.0 {
        return None;
    }

    // 四个值都 ≤ 1 视为归一化坐标；否则模型给的是像素坐标。
    let normalized = x <= 1.0 && y <= 1.0 && w <= 1.0 && h <= 1.0;

    let label = item
        .get("label")
        .or_else(|| item.get("name"))
        .or_else(|| item.get("category"))
        .or_else(|| item.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(MosaicRegion {
        x,
        y,
        w,
        h,
        label,
        normalized,
    })
}

fn as_f32(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

/// 从模型回复里抠出 JSON 文本。
fn extract_json_block(text: &str) -> Option<String> {
    // 1) ```json ... ``` 代码块
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        // 跳过 ```json / ```JSON 之类的语言标记
        let body = match after.find('\n') {
            Some(nl) if after[..nl].trim().is_ascii() => &after[nl + 1..],
            _ => after,
        };
        if let Some(end) = body.find("```") {
            let inner = body[..end].trim();
            if inner.starts_with('{') || inner.starts_with('[') {
                return Some(inner.to_string());
            }
        }
    }
    // 2) 取「起始位置最靠前」的括号块：
    //    - `{"regions":[...]}` → 取 `{...}`；
    //    - `[{...},{...}]`     → 取 `[...]`（否则会被 `{...}` 抢先）。
    let brace = text.find('{').zip(text.rfind('}')).filter(|(s, e)| s < e);
    let bracket = text.find('[').zip(text.rfind(']')).filter(|(s, e)| s < e);
    let picked = match (brace, bracket) {
        (Some(b), Some(k)) => {
            if k.0 < b.0 {
                k
            } else {
                b
            }
        }
        (Some(b), None) => b,
        (None, Some(k)) => k,
        (None, None) => return None,
    };
    Some(text[picked.0..=picked.1].to_string())
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_names() {
        assert_eq!(Provider::parse("DeepSeek"), Some(Provider::DeepSeek));
        assert_eq!(Provider::parse(" minimax "), Some(Provider::MiniMax));
        assert_eq!(Provider::parse("ds"), Some(Provider::DeepSeek));
        assert_eq!(Provider::parse("openai"), None);
    }

    #[test]
    fn provider_defaults_match_official_endpoints() {
        assert_eq!(
            Provider::MiniMax.default_endpoint(),
            "https://api.minimax.cn/v1/chat/completions"
        );
        assert_eq!(
            Provider::DeepSeek.default_endpoint(),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(Provider::MiniMax.default_model(), "MiniMax-M3");
        assert_eq!(Provider::DeepSeek.default_model(), "deepseek-flash");
        assert_eq!(Provider::MiniMax.env_key_name(), "MINIMAX_API_KEY");
        assert_eq!(Provider::DeepSeek.env_key_name(), "DEEPSEEK_API_KEY");
    }

    #[test]
    fn builds_text_only_request_body() {
        let body = build_request_body(Provider::DeepSeek, "deepseek-flash", None, "你好", None);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "deepseek-flash");
        assert_eq!(v["stream"], false);
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "你好");
        // DeepSeek 不该带 thinking
        assert!(v.get("thinking").is_none());
    }

    #[test]
    fn builds_multimodal_request_body_with_data_url() {
        let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let body = build_request_body(
            Provider::MiniMax,
            "MiniMax-M3",
            Some("你是助手"),
            "这张图是什么",
            Some(("image/png", &png)),
        );
        let v: Value = serde_json::from_str(&body).unwrap();
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        let content = msgs[1]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image_url");
        let url = content[0]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert_eq!(content[0]["image_url"]["detail"], "high");
        assert_eq!(content[1]["type"], "text");
        // MiniMax 专属 thinking
        assert_eq!(v["thinking"]["type"], "adaptive");
    }

    #[test]
    fn parses_openai_compatible_completion() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"你好呀"}}]}"#;
        assert_eq!(parse_completion(body).unwrap(), "你好呀");
    }

    #[test]
    fn parses_completion_with_array_content() {
        let body = r#"{"choices":[{"message":{"content":[{"type":"text","text":"A"},{"type":"text","text":"B"}]}}]}"#;
        assert_eq!(parse_completion(body).unwrap(), "AB");
    }

    #[test]
    fn parse_completion_rejects_garbage() {
        assert!(parse_completion("not json").is_err());
        assert!(parse_completion(r#"{"choices":[]}"#).is_err());
    }

    #[test]
    fn describes_minimax_error_shape() {
        let body = r#"{"base_resp":{"status_code":1004,"status_msg":"api key invalid"}}"#;
        let s = describe_error(401, body);
        assert!(s.contains("401"));
        assert!(s.contains("api key invalid"));
    }

    #[test]
    fn describes_openai_error_shape() {
        let body =
            r#"{"error":{"message":"This model does not support image","type":"invalid_request"}}"#;
        let s = describe_error(400, body);
        assert!(s.contains("400"));
        assert!(s.contains("does not support image"));
    }

    #[test]
    fn parses_normalized_regions() {
        let raw = r#"{"regions":[{"x":0.1,"y":0.2,"w":0.3,"h":0.4,"label":"人脸"}]}"#;
        let regs = parse_regions(raw).unwrap();
        assert_eq!(regs.len(), 1);
        assert!(regs[0].normalized);
        assert_eq!(regs[0].label, "人脸");
        let r = regs[0].to_rect(1000, 800);
        assert_eq!(r.x, 100);
        assert_eq!(r.y, 160);
        assert_eq!(r.w, 300);
        assert_eq!(r.h, 320);
    }

    #[test]
    fn parses_regions_from_fenced_code_block() {
        let raw = "好的，结果如下：\n```json\n{\"regions\":[{\"x\":0.5,\"y\":0.5,\"w\":0.2,\"h\":0.2}]}\n```\n";
        let regs = parse_regions(raw).unwrap();
        assert_eq!(regs.len(), 1);
    }

    #[test]
    fn parses_regions_with_bbox_array_and_pixel_coords() {
        let raw = r#"{"regions":[{"bbox":[10,20,30,40],"label":"车牌"}]}"#;
        let regs = parse_regions(raw).unwrap();
        assert_eq!(regs.len(), 1);
        assert!(!regs[0].normalized);
        let r = regs[0].to_rect(500, 500);
        assert_eq!((r.x, r.y, r.w, r.h), (10, 20, 30, 40));
    }

    #[test]
    fn parses_top_level_region_array() {
        let raw = r#"[{"x":0.1,"y":0.1,"w":0.1,"h":0.1}]"#;
        assert_eq!(parse_regions(raw).unwrap().len(), 1);
    }

    #[test]
    fn ignores_malformed_regions() {
        let raw =
            r#"{"regions":[{"x":"abc","y":0.1,"w":0.1,"h":0.1},{"x":0.1,"y":0.1,"w":0,"h":0.1}]}"#;
        assert_eq!(parse_regions(raw).unwrap().len(), 0);
    }

    #[test]
    fn region_is_clipped_to_image() {
        let reg = MosaicRegion {
            x: 0.9,
            y: 0.9,
            w: 0.5,
            h: 0.5,
            label: String::new(),
            normalized: true,
        };
        let r = reg.to_rect(100, 100);
        assert!(r.x + r.w <= 100);
        assert!(r.y + r.h <= 100);
    }

    #[test]
    fn resolves_config_with_env_and_overrides() {
        let cfg = screenloom_core::Config {
            ai_provider: "deepseek".into(),
            ai_model: "deepseek-chat".into(),
            ai_endpoint: "https://example.com/x".into(),
            ai_timeout: 30,
            ..screenloom_core::Config::default()
        };
        let c = AiConfig::resolve(&cfg, None, None, None);
        assert_eq!(c.provider, Provider::DeepSeek);
        assert_eq!(c.model, "deepseek-chat");
        assert_eq!(c.endpoint, "https://example.com/x");
        assert_eq!(c.timeout_secs, 30);

        let c = AiConfig::resolve(
            &cfg,
            Some(Provider::MiniMax),
            Some("MiniMax-M3.1-Flash"),
            None,
        );
        assert_eq!(c.provider, Provider::MiniMax);
        assert_eq!(c.model, "MiniMax-M3.1-Flash");
        // 覆盖 provider 后 endpoint 也回到该 provider 的默认
        assert_eq!(c.endpoint, "https://api.minimax.cn/v1/chat/completions");
    }

    #[test]
    fn missing_key_gives_hint() {
        let cfg = AiConfig {
            api_key: String::new(),
            ..AiConfig::default()
        };
        assert!(!cfg.is_ready());
        assert!(cfg.missing_key_hint().contains("MINIMAX_API_KEY"));
    }
}
