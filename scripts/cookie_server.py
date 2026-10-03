"""Probe server for jar-probe.sh. /login sets an HttpOnly session cookie, a SameSite=Strict one and a
__Host- one (Secure; browsers accept it over http only on localhost names, if at all). /login?u=X signs in
probe user X instead (sid=PROBE-SESSION-X); plain /login keeps sid=PROBE-SESSION (user "default").
/whoami answers "AUTH <user>" or "ANON" by whether a session cookie arrived. Each request is logged as
"<seconds> <engine> <path> auth=<bool> user=<user or -> names=<cookie names sent>" (never a cookie value;
the user is a probe username taken from the sid, not a secret).
Usage: python -P cookie_server.py <port-file> <log-file> <seconds> < /dev/null"""
import http.server, sys, threading, time, urllib.parse

port_file, log_file, lifetime = sys.argv[1], sys.argv[2], float(sys.argv[3])
t0 = time.time()
SID = "sid=PROBE-SESSION"


def user_of(cookie):
    for part in cookie.split(";"):
        name, _, value = part.strip().partition("=")
        if name == "sid" and value.startswith("PROBE-SESSION"):
            return value[len("PROBE-SESSION-"):] or "default"
    return None


class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        ua = self.headers.get("User-Agent", "")
        engine = "cef" if "Chrome" in ua and "Edg/" not in ua else "webview2"
        cookie = self.headers.get("Cookie") or ""
        user = user_of(cookie)
        authed = user is not None
        names = ",".join(sorted(p.split("=", 1)[0].strip() for p in cookie.split(";") if "=" in p)) or "-"
        with open(log_file, "a") as f:
            f.write(f"{time.time() - t0:7.2f} {engine} {self.path} auth={authed} user={user or '-'} names={names}\n")
        body, headers = b"", []
        if self.path.startswith("/login"):
            u = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query).get("u", [""])[0]
            sid = f"{SID}-{u}" if u else SID
            headers = [
                ("Set-Cookie", f"{sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600"),
                ("Set-Cookie", "pref=dark; Path=/; SameSite=Strict; Max-Age=3600"),
                ("Set-Cookie", "__Host-hp=1; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=3600"),
            ]
            body = b"logged in"
        elif self.path.startswith("/whoami"):
            body = f"AUTH {user}".encode() if authed else b"ANON"
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
