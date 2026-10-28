#!/usr/bin/env python3
"""本地 mock Portal，用于开发机上验证 GUI/认证全链路。

用法：
    python3 scripts/mock_portal.py [--portal-port 18888] [--release-port 18889]

配合环境变量：
    CMCC_PROBE_TARGET=127.0.0.1:18888
    CMCC_RELEASE_TARGET=127.0.0.1:18889
    CMCC_NO_SANDBOX=1            # 若 Chromium 沙箱不可用
"""

from __future__ import annotations

import argparse
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

CAPTCHA = (Path(__file__).resolve().parent.parent / "assets/test/captcha_6971.png").read_bytes()
JSESSIONID = "51B9DC8F82AB5CFC08241F5DEEA59A70"

FORM_HTML = f"""<html><head><title>登录</title></head><body>
<form id="inputForm" action="/portalserver/user/unionautologin.do;jsessionid={JSESSIONID}?brasip=1" method="post">
<input type="text" id="username" name="name">
<input type="text" id="passwordIn_1" name="password" value="请输入上网密码">
<input type="password" id="pwd" name="pass">
<input type="text" id="ps" name="psNum">
</form></body></html>""".encode()

SUCCESS_HTML = (
    '<html><head><title>登录成功</title></head>'
    '<body><div id="admin">认证成功</div></body></html>'
).encode()


def make_portal_handler(portal_port: int):
    class Handler(BaseHTTPRequestHandler):
        server_version = "MockPortal/1.0"

        def log_message(self, fmt: str, *args) -> None:  # noqa: A003
            print(f"[portal] {self.address_string()} {fmt % args}")

        def _send(self, status: int, body: bytes, content_type: str, headers: dict | None = None):
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Connection", "close")
            for key, value in (headers or {}).items():
                self.send_header(key, value)
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self) -> None:  # noqa: N802
            path = urlparse(self.path).path
            if path == "/":
                location = (
                    f"http://127.0.0.1:{portal_port}/portalserver/login.jsp"
                    "?wlanuserip=10.9.8.7&wlanacname="
                )
                self._send(302, b"", "text/plain", {"Location": location})
            elif path.startswith("/portalserver/login.jsp"):
                html = (
                    '<html><head><title>portal</title></head><frameset rows="*">'
                    '<frame src="/portalserver/user/unionautologin.do?brasip=1&area=union'
                    '&wlanuserip=10.9.8.7&domain=@cmccgxsd" name="main" id="topFrame1">'
                    "</frameset></html>"
                ).encode()
                self._send(200, html, "text/html; charset=UTF-8")
            elif path.startswith("/portalserver/user/unionautologin.do"):
                self._send(
                    200,
                    FORM_HTML,
                    "text/html; charset=UTF-8",
                    {"Set-Cookie": f"JSESSIONID={JSESSIONID}; Path=/portalserver"},
                )
            elif path.startswith("/portalserver/user/randomimage"):
                self._send(200, CAPTCHA, "image/png")
            else:
                self._send(404, b"not found", "text/plain")

        def do_POST(self) -> None:  # noqa: N802
            path = urlparse(self.path).path
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length)
            if path.startswith("/portalserver/user/unionautologin.do"):
                print(f"[portal] POST body: {body.decode(errors='replace')}")
                self._send(200, SUCCESS_HTML, "text/html; charset=UTF-8")
            else:
                self._send(404, b"not found", "text/plain")

    return Handler


class ReleaseHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args) -> None:  # noqa: A003
        print(f"[release] {fmt % args}")

    def do_GET(self) -> None:  # noqa: N802
        body = b"ok"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)


def serve(server: ThreadingHTTPServer) -> None:
    server.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser(description="本地 mock Portal")
    parser.add_argument("--portal-port", type=int, default=18888)
    parser.add_argument("--release-port", type=int, default=18889)
    args = parser.parse_args()

    portal = ThreadingHTTPServer(
        ("127.0.0.1", args.portal_port), make_portal_handler(args.portal_port)
    )
    release = ThreadingHTTPServer(("127.0.0.1", args.release_port), ReleaseHandler)

    threading.Thread(target=serve, args=(portal,), daemon=True).start()
    threading.Thread(target=serve, args=(release,), daemon=True).start()
    print(f"mock portal:  http://127.0.0.1:{args.portal_port}/")
    print(f"release host: http://127.0.0.1:{args.release_port}/")
    print("Ctrl+C 退出")
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        portal.shutdown()
        release.shutdown()


if __name__ == "__main__":
    main()
