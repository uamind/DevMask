#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2025-2026 Seyud
"""t9/t15 验收用的临时出口代理 fixture。

与 t9 版本的差异（t15 复验需要「按监听端口归因」）：
  * HTTP 监听可以多个端口（默认 18080 设备级 + 18082 应用级），每条日志都带 listener=<端口>，
    这样「应用级代理是否真的被用到」由代理侧日志判定，而不是只看模块自我上报；
  * 新增虚拟主机 app.fixture -> 198.51.100.8（与设备级探测用的 probe.fixture -> 198.51.100.7 区分），
    探针应用只请求 app.fixture，代理日志里出现 app.fixture 就说明请求来自该应用；
  * SOCKS5 同时接受 NOAUTH 与 USERPASS（记录实际用了哪种），便于用裸 Socket 验证 socks 链路；
  * 目标 198.51.100.9:80 走 SOCKS/HTTP 时直接回一段标记文本，用于「裸 socket 经 SOCKS」的证据。

日志只记方法/目标/监听端口/认证方式，永不记录凭据值。
"""

import base64
import hashlib
import ipaddress
import os
import socket
import socketserver
import sys
import threading
import time

HTTP_PORTS = [int(p) for p in os.environ.get("FIXTURE_HTTP_PORTS", "18080,18082").split(",") if p.strip()]
SOCKS_PORT = int(os.environ.get("FIXTURE_SOCKS_PORT", "18081"))
LOG_PATH = os.environ.get("FIXTURE_LOG", "/tmp/egress-fixture-proxy.log")
VIRTUAL_IP = os.environ.get("FIXTURE_VIRTUAL_IP", "198.51.100.7")
APP_IP = os.environ.get("FIXTURE_APP_IP", "198.51.100.8")
RAW_IP = os.environ.get("FIXTURE_RAW_IP", "198.51.100.9")

VIRTUAL_HOSTS = {
    "probe.fixture": VIRTUAL_IP + "\n",
    "ip.fixture": VIRTUAL_IP + "\n",
    "checkip.fixture": VIRTUAL_IP + "\n",
    "app.fixture": APP_IP + "\n",
    VIRTUAL_IP: VIRTUAL_IP + "\n",
    APP_IP: APP_IP + "\n",
    RAW_IP: "raw-socket-via-proxy-ok\n",
}

SOCKS_USER = os.environ.get("FIXTURE_SOCKS_USER", "")
SOCKS_PASS = os.environ.get("FIXTURE_SOCKS_PASS", "")
# prefer-noauth（默认）：客户端支持 NOAUTH 就用 NOAUTH，用于验证「链路通」；
# require-userpass：只接受用户名密码，用于观察 libcore 到底有没有消费 java.net.socks.* 属性。
SOCKS_MODE = os.environ.get("FIXTURE_SOCKS_MODE", "prefer-noauth")

# ── t9 增量：上游认证（HTTP Basic / SOCKS5 RFC1929）──────────────────────────
# 默认 off，t15/t18 的行为保持不变；require 时缺凭据或凭据错一律 407，
# 日志只写摘要（sha256 前 12 位），永不写明文。
#   FIXTURE_HTTP_AUTH=require  FIXTURE_HTTP_USER=…  FIXTURE_HTTP_PASS=…
#   FIXTURE_SOCKS_MODE=require-userpass  FIXTURE_SOCKS_USER=…  FIXTURE_SOCKS_PASS=…
HTTP_AUTH_MODE = os.environ.get("FIXTURE_HTTP_AUTH", "off").strip().lower()
HTTP_USER = os.environ.get("FIXTURE_HTTP_USER", "")
HTTP_PASS = os.environ.get("FIXTURE_HTTP_PASS", "")
SOCKS_REQUIRE_USERPASS = SOCKS_MODE == "require-userpass"


def credential_digest(user, password):
    """凭据摘要：日志里只出现它，用来证明「内容正确」而不暴露内容。"""
    return hashlib.sha256(("%s:%s" % (user, password)).encode("utf-8")).hexdigest()[:12]


HTTP_DIGEST = credential_digest(HTTP_USER, HTTP_PASS)
SOCKS_DIGEST = credential_digest(SOCKS_USER, SOCKS_PASS)

_log_lock = threading.Lock()


def log(kind, message):
    line = "%s proto=%s %s\n" % (time.strftime("%Y-%m-%dT%H:%M:%S%z"), kind, message)
    with _log_lock:
        with open(LOG_PATH, "a", encoding="utf-8") as handle:
            handle.write(line)
    sys.stderr.write(line)
    sys.stderr.flush()


def http_response(body, status="200 OK"):
    payload = body.encode("utf-8")
    head = (
        "HTTP/1.1 %s\r\n"
        "Content-Type: text/plain; charset=utf-8\r\n"
        "Content-Length: %d\r\n"
        "Connection: close\r\n"
        "\r\n" % (status, len(payload))
    )
    return head.encode("utf-8") + payload


def http_407():
    payload = b"proxy authentication required\n"
    head = (
        "HTTP/1.1 407 Proxy Authentication Required\r\n"
        "Proxy-Authenticate: Basic realm=\"device-faker-t9\"\r\n"
        "Content-Type: text/plain; charset=utf-8\r\n"
        "Content-Length: %d\r\n"
        "Connection: close\r\n"
        "\r\n" % len(payload)
    )
    return head.encode("utf-8") + payload


def http_auth_state(headers):
    """上游视角的认证判定：off / ok / missing / bad（+ 仅摘要）。"""
    if HTTP_AUTH_MODE != "require":
        return "off", "-"
    raw = headers.get("proxy-authorization", "")
    if not raw:
        return "missing", "-"
    scheme, _, value = raw.partition(" ")
    if scheme.strip().lower() != "basic" or not value.strip():
        return "bad", "-"
    try:
        decoded = base64.b64decode(value.strip(), validate=True).decode("utf-8", "replace")
    except Exception:
        return "bad", "-"
    user, _, password = decoded.partition(":")
    digest = credential_digest(user, password)
    return ("ok" if (user == HTTP_USER and password == HTTP_PASS) else "bad"), digest


def tunnel_scan(raw):
    """隧道内字节里是否出现 Proxy-Authorization 或 base64 凭据（都不该出现）。"""
    lowered = raw.lower()
    header_hit = 1 if b"proxy-authorization" in lowered else 0
    cred_hit = 0
    for user, password in ((HTTP_USER, HTTP_PASS), (SOCKS_USER, SOCKS_PASS)):
        if user or password:
            token = base64.b64encode(("%s:%s" % (user, password)).encode("utf-8")).lower()
            if token and token in lowered:
                cred_hit = 1
    return header_hit, cred_hit


def read_until_double_crlf(sock, limit=64 * 1024):
    data = b""
    while b"\r\n\r\n" not in data and len(data) < limit:
        chunk = sock.recv(4096)
        if not chunk:
            break
        data += chunk
    return data


def parse_headers(raw):
    text = raw.split(b"\r\n\r\n", 1)[0].decode("iso-8859-1", "replace")
    lines = text.split("\r\n")
    request_line = lines[0] if lines else ""
    headers = {}
    for line in lines[1:]:
        if ":" in line:
            key, _, value = line.partition(":")
            headers[key.strip().lower()] = value.strip()
    return request_line, headers


def splice(a, b):
    def pump(src, dst):
        try:
            while True:
                chunk = src.recv(16384)
                if not chunk:
                    break
                dst.sendall(chunk)
        except OSError:
            pass
        finally:
            try:
                dst.shutdown(socket.SHUT_WR)
            except OSError:
                pass

    t = threading.Thread(target=pump, args=(b, a), daemon=True)
    t.start()
    pump(a, b)
    t.join(timeout=5)


def handle_http(conn, listener_port):
    try:
        raw = read_until_double_crlf(conn)
        if not raw:
            return
        request_line, headers = parse_headers(raw)
        parts = request_line.split()
        if len(parts) < 3:
            conn.sendall(http_response("bad request\n", "400 Bad Request"))
            return
        method, target = parts[0], parts[1]
        auth_state, auth_digest = http_auth_state(headers)
        if HTTP_AUTH_MODE == "require" and auth_state != "ok":
            log("http", "listener=%d %s %s auth=%s digest=%s rejected=407" % (
                listener_port, method, target, auth_state, auth_digest))
            conn.sendall(http_407())
            return

        if method.upper() == "CONNECT":
            host, _, port_text = target.partition(":")
            port = int(port_text or "443")
            log("http", "listener=%d CONNECT %s:%d auth=%s digest=%s" % (
                listener_port, host, port, auth_state, auth_digest))
            if host in VIRTUAL_HOSTS:
                conn.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                inner = read_until_double_crlf(conn)
                header_hit, cred_hit = tunnel_scan(inner)
                log("http", "listener=%d tunnel-request %s %d bytes auth_header=%d cred_hit=%d" % (
                    listener_port, host, len(inner), header_hit, cred_hit))
                conn.sendall(http_response(VIRTUAL_HOSTS[host]))
                return
            upstream = socket.create_connection((host, port), timeout=10)
            conn.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            splice(conn, upstream)
            upstream.close()
            return

        if "://" not in target:
            conn.sendall(http_response("absolute-uri required\n", "400 Bad Request"))
            return
        scheme, _, rest = target.partition("://")
        hostport = rest.split("/", 1)[0]
        path = "/" + rest.split("/", 1)[1] if "/" in rest else "/"
        host, _, port_text = hostport.partition(":")
        port = int(port_text or ("443" if scheme == "https" else "80"))
        log("http", "listener=%d %s %s host=%s auth=%s digest=%s" % (
            listener_port, method, target, host, auth_state, auth_digest))
        if host in VIRTUAL_HOSTS:
            conn.sendall(http_response(VIRTUAL_HOSTS[host]))
            return
        header_lines = [request_line.replace(target, path, 1)]
        for key, value in headers.items():
            if key in ("proxy-authorization", "proxy-connection"):
                continue
            header_lines.append("%s: %s" % (key, value))
        if "host" not in headers:
            header_lines.append("Host: %s" % hostport)
        header_lines.append("Connection: close")
        forwarded = ("\r\n".join(header_lines) + "\r\n\r\n").encode("iso-8859-1")
        upstream = socket.create_connection((host, port), timeout=10)
        upstream.sendall(forwarded)
        while True:
            chunk = upstream.recv(16384)
            if not chunk:
                break
            conn.sendall(chunk)
        upstream.close()
    except OSError as exc:
        log("http", "listener=%d error %s" % (listener_port, exc))
    finally:
        try:
            conn.close()
        except OSError:
            pass


listener_port_hint = [SOCKS_PORT]


def socks5_handshake(conn):
    head = conn.recv(2)
    if len(head) < 2 or head[0] != 0x05:
        return False, None, "bad-version"
    nmethods = head[1]
    methods = conn.recv(nmethods) if nmethods else b""
    offered = "".join("%02x" % b for b in methods)
    method = "none"
    use_userpass = False
    if SOCKS_MODE == "require-userpass":
        use_userpass = True
    elif SOCKS_MODE == "prefer-userpass":
        use_userpass = 0x02 in methods
    else:
        use_userpass = (0x00 not in methods) and (0x02 in methods)
    if SOCKS_REQUIRE_USERPASS:
        # fail-closed：要求认证时，fixture 没配凭据 / 客户端没提供 0x02 → 一律 0xff
        if not (SOCKS_USER and SOCKS_PASS):
            conn.sendall(b"\x05\xff")
            log("socks5", "listener=%d methods=%s rejected=require-userpass-but-unconfigured" % (
                listener_port_hint[0], offered))
            return False, None, "fixture-unconfigured"
        if 0x02 not in methods:
            conn.sendall(b"\x05\xff")
            log("socks5", "listener=%d methods=%s rejected=require-userpass-but-no-userpass-offered" % (
                listener_port_hint[0], offered))
            return False, None, "no-userpass-offered"
    if SOCKS_REQUIRE_USERPASS or (use_userpass and SOCKS_USER and 0x02 in methods):
        conn.sendall(b"\x05\x02")
        ver_auth = conn.recv(2)
        if len(ver_auth) < 2 or ver_auth[0] != 0x01:
            return False, None, "bad-auth-version"
        ulen = ver_auth[1]
        user = conn.recv(ulen).decode("utf-8", "replace") if ulen else ""
        plen_raw = conn.recv(1)
        plen = plen_raw[0] if plen_raw else 0
        password = conn.recv(plen).decode("utf-8", "replace") if plen else ""
        user_ok = user == SOCKS_USER
        pass_ok = password == SOCKS_PASS
        ok = user_ok and pass_ok
        log("socks5", "listener=%d methods=%s auth user_ok=%d pass_ok=%d digest=%s (values never logged)" % (
            listener_port_hint[0], offered, int(user_ok), int(pass_ok), credential_digest(user, password)))
        conn.sendall(b"\x01\x00" if ok else b"\x01\x01")
        if not ok:
            return False, None, "auth-failed"
        method = "userpass(ok)"
    elif 0x00 in methods:
        conn.sendall(b"\x05\x00")
        method = "noauth"
    else:
        conn.sendall(b"\x05\xff")
        return False, None, "no-acceptable-method"

    request = conn.recv(4)
    if len(request) < 4 or request[1] != 0x01:
        conn.sendall(b"\x05\x07\x00\x01\x00\x00\x00\x00\x00\x00")
        return False, None, "unsupported-command"
    atyp = request[3]
    if atyp == 0x01:
        host = socket.inet_ntoa(conn.recv(4))
    elif atyp == 0x03:
        length = conn.recv(1)[0]
        host = conn.recv(length).decode("utf-8", "replace")
    elif atyp == 0x04:
        host = str(ipaddress.IPv6Address(conn.recv(16)))
    else:
        return False, None, "bad-atyp"
    port = int.from_bytes(conn.recv(2), "big")
    return True, (host, port), method


def handle_socks(conn, listener_port):
    try:
        ok, target, method = socks5_handshake(conn)
        if not ok or not target:
            log("socks5", "listener=%d method=%s rejected" % (listener_port, method))
            return
        host, port = target
        log("socks5", "listener=%d method=%s methods=%s CONNECT %s:%d" % (
            listener_port, method, locals().get("offered", "?"), host, port))
        if host in VIRTUAL_HOSTS:
            conn.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
            inner = read_until_double_crlf(conn)
            header_hit, cred_hit = tunnel_scan(inner)
            log("socks5", "listener=%d tunnel-request %s %d bytes auth_header=%d cred_hit=%d" % (
                listener_port, host, len(inner), header_hit, cred_hit))
            conn.sendall(http_response(VIRTUAL_HOSTS[host]))
            return
        upstream = socket.create_connection((host, port), timeout=10)
        conn.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
        splice(conn, upstream)
        upstream.close()
    except OSError as exc:
        log("socks5", "listener=%d error %s" % (listener_port, exc))
    finally:
        try:
            conn.close()
        except OSError:
            pass


class ThreadedServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def make_http_handler(listener_port):
    class Handler(socketserver.BaseRequestHandler):
        def handle(self):
            handle_http(self.request, listener_port)

    return Handler


class SocksHandler(socketserver.BaseRequestHandler):
    def handle(self):
        listener_port_hint[0] = SOCKS_PORT
        handle_socks(self.request, SOCKS_PORT)


def main():
    os.makedirs(os.path.dirname(LOG_PATH) or ".", exist_ok=True)
    servers = []
    for port in HTTP_PORTS:
        server = ThreadedServer(("0.0.0.0", port), make_http_handler(port))
        servers.append(server)
        threading.Thread(target=server.serve_forever, daemon=True).start()
    socks_server = ThreadedServer(("0.0.0.0", SOCKS_PORT), SocksHandler)
    threading.Thread(target=socks_server.serve_forever, daemon=True).start()
    log("start", "http_ports=%s socks=%d virtual=%s app=%s raw=%s socks_auth=%d" % (
        ",".join(str(p) for p in HTTP_PORTS), SOCKS_PORT, VIRTUAL_IP, APP_IP, RAW_IP, int(bool(SOCKS_USER))))
    servers[0].serve_forever()


if __name__ == "__main__":
    main()
