//! 在线/断线探测。
//!
//! 判断方式完全沿用原 Python 版：向黑洞地址（TEST-NET-1，RFC 5737）发 :80 请求。
//! - 未认证：BRAS 对任意目的 :80 无差别 302 到 Portal，Location 带 `wlanuserip=`；
//! - 已在线：黑洞目标永不应答 → 超时/空连接 → 视为在线；
//! - 其余任何结局都按“在线”处理（网络全断时也没有可认证的对象，静默等待）。
//!
//! 认证成功后另用真实主机 1.1.1.1 做毫秒级放行确认（每次登录只此一次请求）。

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub const BLACKHOLE_IP: &str = "192.0.2.1";
pub const RELEASE_IP: &str = "1.1.1.1";
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(3);

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct HttpResponseHead {
    pub status: u16,
    pub location: Option<String>,
}

#[derive(Debug)]
pub enum PortalProbe {
    /// 无 Portal 重定向：在线
    Online,
    /// 被网关 302 到 Portal：断线，值为完整认证页 URL
    Portal(String),
    /// 探测本身失败（网络不可达等）
    Error(io::Error),
}

/// 读取环境变量测试覆盖；默认值与原版一致。
fn probe_target() -> String {
    std::env::var("CMCC_PROBE_TARGET").unwrap_or_else(|_| format!("{BLACKHOLE_IP}:80"))
}

fn release_target() -> String {
    std::env::var("CMCC_RELEASE_TARGET").unwrap_or_else(|_| format!("{RELEASE_IP}:80"))
}

/// 发一个最小 GET 请求，只解析状态行与 Location 响应头。
pub fn http_get_head(target: &str, timeout: Duration) -> io::Result<HttpResponseHead> {
    let addr = target
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "无法解析探测地址"))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    // 一次性写出完整请求：分片写出时部分服务器读到首段就应答并关闭，
    // 未读数据会让 Windows 内核回 RST，客户端会丢掉响应。
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {target}\r\nUser-Agent: {USER_AGENT}\r\n\
         Accept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            // 连接被空关闭：让调用方按网络错误处理
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "连接在响应头之前关闭",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "响应头过大"));
        }
    };

    parse_head(&buf[..header_end])
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn parse_head(head: &[u8]) -> io::Result<HttpResponseHead> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split_whitespace();
    let _version = parts.next();
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "无法解析 HTTP 状态行"))?;

    let mut location = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("location")
        {
            location = Some(value.trim().to_string());
            break;
        }
    }

    Ok(HttpResponseHead { status, location })
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn probe_portal_at(target: &str, timeout: Duration) -> PortalProbe {
    match http_get_head(target, timeout) {
        Ok(head) => {
            if is_redirect(head.status)
                && let Some(location) = head.location
                && location.contains("wlanuserip=")
            {
                return PortalProbe::Portal(location);
            }
            PortalProbe::Online
        }
        Err(err) => PortalProbe::Error(err),
    }
}

/// 探测是否被网关重定向到 Portal。
pub fn probe_portal() -> PortalProbe {
    probe_portal_at(&probe_target(), PROBE_TIMEOUT)
}

/// 监控轮询用：任何异常都按“在线”处理（与原版 is_offline_sync 一致）。
pub fn is_offline_sync() -> bool {
    matches!(probe_portal(), PortalProbe::Portal(_))
}

/// 认证后的 BRAS 放行确认：
/// - `Some(true)`  已放行（正常应答）
/// - `Some(false)` 仍被网关 302 拦截（未放行）
/// - `None`        探测异常，状态未知（不等于未放行）
pub fn is_released() -> Option<bool> {
    is_released_at(&release_target(), RELEASE_TIMEOUT)
}

fn is_released_at(target: &str, timeout: Duration) -> Option<bool> {
    let head = http_get_head(target, timeout).ok()?;
    if is_redirect(head.status)
        && let Some(location) = head.location
        && location.contains("wlanuserip=")
    {
        return Some(false);
    }
    Some(true)
}

/// 超时/连接被空关闭等“在线时黑洞探测的正常结局”，不应视为网络故障。
pub fn is_quiet_online_error(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    )
}

/// 从 Portal URL 中提取 WAN IP（`wlanuserip=100.96.1.2`）。
pub fn extract_wan_ip(url: &str) -> Option<String> {
    let rest = url.split_once("wlanuserip=")?.1;
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    let ip = &rest[..end];
    let octets: Vec<&str> = ip.split('.').collect();
    if octets.len() == 4 && octets.iter().all(|o| o.parse::<u8>().is_ok()) {
        Some(ip.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn one_shot_server(response: &'static str) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // 读到请求头结束再应答；残留未读请求数据时直接关闭，
                // Windows 会回 RST，客户端读不到响应。
                let mut buf = Vec::new();
                let mut chunk = [0u8; 512];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        });
        (format!("127.0.0.1:{}", addr.port()), handle)
    }

    #[test]
    fn parses_portal_redirect() {
        let (addr, handle) = one_shot_server(
            "HTTP/1.1 302 Moved Temporarily\r\n\
             Location: http://218.200.239.185:8888/portalserver/login.jsp?wlanuserip=100.96.1.2&wlanacname=\r\n\
             Content-Length: 0\r\n\r\n",
        );
        match probe_portal_at(&addr, Duration::from_secs(5)) {
            PortalProbe::Portal(url) => {
                assert!(url.contains("wlanuserip=100.96.1.2"));
            }
            other => panic!("期望 Portal，得到 {other:?}"),
        }
        handle.join().unwrap();
    }

    #[test]
    fn non_portal_redirect_is_online() {
        let (addr, handle) = one_shot_server(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: https://example.com/\r\n\r\n",
        );
        assert!(matches!(
            probe_portal_at(&addr, Duration::from_secs(5)),
            PortalProbe::Online
        ));
        handle.join().unwrap();
    }

    #[test]
    fn plain_200_is_online() {
        let (addr, handle) = one_shot_server("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        assert!(matches!(
            probe_portal_at(&addr, Duration::from_secs(5)),
            PortalProbe::Online
        ));
        handle.join().unwrap();
    }

    #[test]
    fn release_detects_portal_interception() {
        let (addr, handle) = one_shot_server(
            "HTTP/1.1 302 Found\r\nLocation: http://portal/?wlanuserip=10.0.0.1\r\n\r\n",
        );
        assert_eq!(is_released_at(&addr, Duration::from_secs(5)), Some(false));
        handle.join().unwrap();
    }

    #[test]
    fn release_true_on_normal_response() {
        let (addr, handle) =
            one_shot_server("HTTP/1.1 301 Moved\r\nLocation: https://one.one.one.one/\r\n\r\n");
        assert_eq!(is_released_at(&addr, Duration::from_secs(5)), Some(true));
        handle.join().unwrap();
    }

    #[test]
    fn extract_ip() {
        assert_eq!(
            extract_wan_ip("http://x/login.jsp?wlanuserip=100.96.171.247&a=b"),
            Some("100.96.171.247".into())
        );
        assert_eq!(
            extract_wan_ip("http://x/login.jsp?wlanuserip=100.96.171.247"),
            Some("100.96.171.247".into())
        );
        assert_eq!(extract_wan_ip("http://x/?wlanacname="), None);
        assert_eq!(extract_wan_ip("http://x/?wlanuserip=999.1.1.1"), None);
    }

    #[test]
    fn header_end_detection() {
        assert_eq!(
            find_header_end(b"HTTP/1.1 302\r\nX: y\r\n\r\nrest"),
            Some(22)
        );
        assert_eq!(find_header_end(b"HTTP/1.1 302\r\nX: y\r\n"), None);
    }
}
