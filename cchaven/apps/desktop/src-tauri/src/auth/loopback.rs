//! Loopback HTTP listener that receives the OAuth authorization callback.
//!
//! The control plane whitelists `http://127.0.0.1:*/callback`, so the app binds
//! an ephemeral port first and only then builds the authorize URL. Binding to
//! `127.0.0.1` (never `0.0.0.0`) keeps the callback unreachable from the network.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Longest single request line we are willing to read, guarding against a local
/// process trying to exhaust memory through the callback port.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// What a request to the loopback port turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackOutcome {
    /// A well-formed `/callback?code=…&state=…`.
    Code { code: String, state: String },
    /// The authorization server reported a failure.
    Denied { error: String, description: String },
    /// Anything else (favicon probes, port scanners, malformed lines).
    Ignored,
}

/// Why waiting for the callback ended without an authorization code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackError {
    /// Nothing arrived before the deadline (login page 「等待授权超时」).
    TimedOut,
    /// The `state` did not match the one we generated — possible CSRF.
    StateMismatch,
    /// The authorization server refused.
    Denied { error: String, description: String },
    /// Local socket failure.
    Io(String),
}

impl CallbackError {
    /// User-facing zh-CN message. Kept in Rust because the flow can fail before
    /// any frontend view exists to render it.
    pub fn message(&self) -> String {
        match self {
            Self::TimedOut => {
                "等待授权超时。浏览器可能没有打开，或你尚未在浏览器中完成登录。".into()
            }
            Self::StateMismatch => "授权回调校验失败，请重新发起登录。".into(),
            Self::Denied { description, .. } if !description.is_empty() => description.clone(),
            Self::Denied { .. } => "你在浏览器中取消了授权。".into(),
            Self::Io(_) => "无法在本机接收授权回调，请重试。".into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::TimedOut => "timeout",
            Self::StateMismatch => "state_mismatch",
            Self::Denied { .. } => "denied",
            Self::Io(_) => "io",
        }
    }
}

/// A bound loopback listener waiting for exactly one authorization callback.
pub struct LoopbackServer {
    listener: TcpListener,
    port: u16,
}

impl LoopbackServer {
    /// Bind an ephemeral port on the loopback interface.
    pub async fn bind() -> Result<Self, CallbackError> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .map_err(|e| CallbackError::Io(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| CallbackError::Io(e.to_string()))?
            .port();
        Ok(Self { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The exact `redirect_uri` to send to `/authorize`; the control plane
    /// requires the token exchange to repeat it verbatim.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.port)
    }

    /// Serve requests until the authorization code arrives or the deadline passes.
    ///
    /// Unrelated requests (a browser asking for `/favicon.ico`, say) are answered
    /// with 404 and do not consume the attempt.
    pub async fn wait_for_code(
        self,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<String, CallbackError> {
        tokio::time::timeout(timeout, self.serve(expected_state))
            .await
            .unwrap_or(Err(CallbackError::TimedOut))
    }

    async fn serve(self, expected_state: &str) -> Result<String, CallbackError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|e| CallbackError::Io(e.to_string()))?;
            match handle_connection(stream, expected_state).await {
                Ok(Some(code)) => return Ok(code),
                Ok(None) => continue,
                Err(err) => return Err(err),
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    expected_state: &str,
) -> Result<Option<String>, CallbackError> {
    let request_line = match read_request_line(&mut stream).await {
        Ok(line) => line,
        Err(err) => {
            respond(&mut stream, 400, PAGE_BAD_REQUEST).await;
            return Err(err);
        }
    };

    match parse_request_line(&request_line) {
        CallbackOutcome::Code { code, state } => {
            if state != expected_state {
                respond(&mut stream, 400, PAGE_BAD_REQUEST).await;
                return Err(CallbackError::StateMismatch);
            }
            respond(&mut stream, 200, PAGE_SUCCESS).await;
            Ok(Some(code))
        }
        CallbackOutcome::Denied { error, description } => {
            respond(&mut stream, 200, PAGE_DENIED).await;
            Err(CallbackError::Denied { error, description })
        }
        CallbackOutcome::Ignored => {
            respond(&mut stream, 404, PAGE_NOT_FOUND).await;
            Ok(None)
        }
    }
}

async fn read_request_line(stream: &mut TcpStream) -> Result<String, CallbackError> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|e| CallbackError::Io(e.to_string()))?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(end) = find_crlf(&buf) {
            buf.truncate(end);
            break;
        }
        if buf.len() > MAX_REQUEST_BYTES {
            return Err(CallbackError::Io("请求过长".into()));
        }
    }
    String::from_utf8(buf).map_err(|_| CallbackError::Io("请求不是合法 UTF-8".into()))
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
}

/// Parse an HTTP request line such as `GET /callback?code=abc&state=xyz HTTP/1.1`.
pub fn parse_request_line(line: &str) -> CallbackOutcome {
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return CallbackOutcome::Ignored;
    };
    if !method.eq_ignore_ascii_case("GET") {
        return CallbackOutcome::Ignored;
    }
    parse_target(target)
}

/// Parse the request target (path + query) of a callback request.
pub fn parse_target(target: &str) -> CallbackOutcome {
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return CallbackOutcome::Ignored;
    };
    if url.path() != "/callback" {
        return CallbackOutcome::Ignored;
    }

    let mut code = None;
    let mut state = String::new();
    let mut error = None;
    let mut description = String::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = value.into_owned(),
            "error" => error = Some(value.into_owned()),
            "error_description" => description = value.into_owned(),
            _ => {}
        }
    }

    if let Some(error) = error {
        return CallbackOutcome::Denied { error, description };
    }
    match code {
        Some(code) if !code.is_empty() => CallbackOutcome::Code { code, state },
        _ => CallbackOutcome::Ignored,
    }
}

async fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    );
    // A browser that already navigated away makes these writes fail; the
    // authorization result is unaffected either way.
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

/// Shared chrome for the four browser-facing result pages.
macro_rules! result_page {
    ($title:literal, $detail:literal) => {
        concat!(
            "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">",
            "<title>CC避风港</title><style>body{font-family:-apple-system,BlinkMacSystemFont,",
            "\"PingFang SC\",sans-serif;background:#f7f8fa;color:#16181d;display:flex;",
            "align-items:center;justify-content:center;height:100vh;margin:0}",
            ".card{background:#fff;border:1px solid #e5e7eb;border-radius:14px;padding:36px 44px;",
            "text-align:center;box-shadow:0 8px 30px rgba(16,24,40,.08)}h1{font-size:20px;margin:0 0 10px}",
            "p{color:#6b7280;font-size:14px;margin:0}</style></head>",
            "<body><div class=\"card\"><h1>",
            $title,
            "</h1><p>",
            $detail,
            "</p></div></body></html>"
        )
    };
}

const PAGE_SUCCESS: &str = result_page!("授权成功", "可以关闭此页面，回到 CC避风港 继续。");
const PAGE_DENIED: &str = result_page!("授权未完成", "你可以关闭此页面，回到 CC避风港 重试。");
const PAGE_NOT_FOUND: &str = result_page!("页面不存在", "这是 CC避风港 的本机回调端口。");
const PAGE_BAD_REQUEST: &str = result_page!("请求无效", "请回到 CC避风港 重新发起登录。");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_successful_callback() {
        assert_eq!(
            parse_request_line("GET /callback?code=abc123&state=xyz HTTP/1.1"),
            CallbackOutcome::Code {
                code: "abc123".into(),
                state: "xyz".into(),
            }
        );
    }

    #[test]
    fn percent_decodes_query_values() {
        assert_eq!(
            parse_target("/callback?code=a%2Bb%2Fc&state=s%20t"),
            CallbackOutcome::Code {
                code: "a+b/c".into(),
                state: "s t".into(),
            }
        );
    }

    #[test]
    fn reports_authorization_server_errors() {
        assert_eq!(
            parse_target(
                "/callback?error=access_denied&error_description=%E5%B7%B2%E5%8F%96%E6%B6%88"
            ),
            CallbackOutcome::Denied {
                error: "access_denied".into(),
                description: "已取消".into(),
            }
        );
    }

    #[test]
    fn ignores_unrelated_requests() {
        for target in [
            "GET /favicon.ico HTTP/1.1",
            "GET /callback HTTP/1.1",
            "GET /callback?code= HTTP/1.1",
            "POST /callback?code=abc HTTP/1.1",
            "garbage",
            "",
        ] {
            assert_eq!(
                parse_request_line(target),
                CallbackOutcome::Ignored,
                "expected {target:?} to be ignored"
            );
        }
    }

    #[tokio::test]
    async fn receives_the_code_over_a_real_socket() {
        let server = LoopbackServer::bind().await.expect("bind");
        let port = server.port();
        assert_eq!(
            server.redirect_uri(),
            format!("http://127.0.0.1:{port}/callback")
        );

        let waiter =
            tokio::spawn(
                async move { server.wait_for_code("st4te", Duration::from_secs(5)).await },
            );

        // A stray probe must not consume the pending login attempt.
        request(port, "GET /favicon.ico HTTP/1.1").await;
        request(port, "GET /callback?code=the-code&state=st4te HTTP/1.1").await;

        assert_eq!(waiter.await.expect("join"), Ok("the-code".to_string()));
    }

    #[tokio::test]
    async fn rejects_a_mismatched_state() {
        let server = LoopbackServer::bind().await.expect("bind");
        let port = server.port();
        let waiter = tokio::spawn(async move {
            server
                .wait_for_code("expected", Duration::from_secs(5))
                .await
        });
        request(port, "GET /callback?code=c&state=forged HTTP/1.1").await;
        assert_eq!(
            waiter.await.expect("join"),
            Err(CallbackError::StateMismatch)
        );
    }

    #[tokio::test]
    async fn times_out_when_nothing_arrives() {
        let server = LoopbackServer::bind().await.expect("bind");
        let result = server
            .wait_for_code("state", Duration::from_millis(60))
            .await;
        assert_eq!(result, Err(CallbackError::TimedOut));
        assert_eq!(
            CallbackError::TimedOut.message(),
            "等待授权超时。浏览器可能没有打开，或你尚未在浏览器中完成登录。"
        );
    }

    async fn request(port: u16, line: &str) {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect");
        stream
            .write_all(format!("{line}\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
            .await
            .expect("write");
        let mut sink = Vec::new();
        let _ = stream.read_to_end(&mut sink).await;
    }
}
