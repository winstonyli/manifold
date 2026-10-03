"""Probe server for jar-probe.sh. /login sets an HttpOnly session cookie and a SameSite=Strict one.
/whoami answers AUTH or ANON by whether the session cookie arrived. Each request is logged as
"<seconds> <engine> <path> auth=<bool>" (never a cookie value).
Usage: python -P cookie_server.py <port-file> <log-file> <seconds> < /dev/null"""
import http.server, sys, threading, time

port_file, log_file, lifetime = sys.argv[1], sys.argv[2], float(sys.argv[3])
t0 = time.time()
SID = "sid=PROBE-SESSION"


class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        ua = self.headers.get("User-Agent", "")
        engine = "cef" if "Chrome" in ua and "Edg/" not in ua else "webview2"
        cookie = self.headers.get("Cookie") or ""
        authed = SID in cookie
        with open(log_file, "a") as f:
            f.write(f"{time.time() - t0:7.2f} {engine} {self.path} auth={authed}\n")
        body, headers = b"", []
        if self.path.startswith("/login"):
            headers = [
                ("Set-Cookie", f"{SID}; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600"),
                ("Set-Cookie", "pref=dark; Path=/; SameSite=Strict; Max-Age=3600"),
            ]
            body = b"logged in"
        elif self.path.startswith("/whoami"):
            body = b"AUTH" if authed else b"ANON"
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)


srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
open(port_file, "w").write(str(srv.server_address[1]))
threading.Timer(lifetime, lambda: srv.shutdown()).start()
srv.serve_forever()
