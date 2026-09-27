"""A site that redirects to a fresh url on every load, for two-instance-test.sh.

Usage: python -P scripts/nonce_server.py <hits-file> <seconds> < /dev/null

Serves http://127.0.0.1:<port>/r. Each `?n=` it issued is served once;
any other request (no `n`, or an `n` already served) redirects to a new
`n`, like a login nonce that is consumed on use. Also serves `/link`, a
page that is one window-sized link to `/r`, `/js`, a page whose script
navigates to a fresh `/dest?n=` without a gesture, and `/spa`, a
single-page app: a click pushes `/spa/c?n=` and a timer pushes `/spa/t?n=`
a second later, neither of which asks the server for anything.

Prints the port, writes the running count of /r requests to <hits-file>
and "<r-repeats> <dest-repeats> <dests> <spa-clicks> <spa-timers>" to
<hits-file>.repeats: how often an already-served `/r?n=` or `/dest?n=` was
asked for again, which is another instance loading a url one instance
shared, how many `/dest` urls were asked for at all, and how often `/spa/c`
and `/spa/t` were (only a load of a shared url asks). Exits after
<seconds> on its own, so nothing has to kill it.
"""

import http.server
import sys
import threading
import time
import urllib.parse

hits_path, lifetime = sys.argv[1], float(sys.argv[2])
lock = threading.Lock()
state = {"next": 0, "issued": set(), "served": set(), "hits": 0, "dests": set(), "repeats": [0, 0], "spa": [0, 0]}

LINK = b"""<!doctype html><title>link</title>
<a href="/r" style="position:fixed;inset:0;display:block">go</a>"""
JS = b"""<!doctype html><title>js</title>
<script>setTimeout(function () { location.href = "/dest?n=" + Math.random(); }, 500);</script>"""
SPA = b"""<!doctype html><title>spa</title>
<div style="position:fixed;inset:0">spa</div>
<script>onclick = function () {
  history.pushState({}, "", "/spa/c?n=" + Math.random());
  setTimeout(function () { history.pushState({}, "", "/spa/t?n=" + Math.random()); }, 1000);
};</script>"""


def write(path, text):
    try:
        with open(path, "w") as f:
            f.write(text)
    except OSError:
        pass  # the test's output dir is gone: it has finished


def write_counts():
    write(hits_path + ".repeats", "%d %d %d %d %d" % (*state["repeats"], len(state["dests"]), *state["spa"]))


class Handler(http.server.BaseHTTPRequestHandler):
    def page(self, body):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        with lock:
            try:
                with open(hits_path + ".log", "a") as f:
                    f.write("%.2f %s\n" % (time.monotonic(), self.path))
            except OSError:
                pass
        n = urllib.parse.parse_qs(url.query).get("n", [None])[0]
        if url.path == "/link":
            return self.page(LINK)
        if url.path == "/js":
            return self.page(JS)
        if url.path in ("/spa", "/spa/c", "/spa/t"):
            if url.path != "/spa":
                with lock:
                    state["spa"][url.path == "/spa/t"] += 1
                    write_counts()
            return self.page(SPA)
        if url.path == "/dest":
            with lock:
                if n in state["dests"]:
                    state["repeats"][1] += 1
                state["dests"].add(n)
                write_counts()
            return self.page(b"<!doctype html><title>dest</title>")
        if url.path != "/r":
            self.send_response(404)
            self.end_headers()
            return
        with lock:
            state["hits"] += 1
            write(hits_path, str(state["hits"]))
            if n in state["served"]:
                state["repeats"][0] += 1
                write_counts()
            if n in state["issued"] and n not in state["served"]:
                state["served"].add(n)
                redirect = None
            else:
                state["next"] += 1
                redirect = f"/r?n={state['next']}"
                state["issued"].add(str(state["next"]))
        if redirect:
            self.send_response(302)
            self.send_header("Location", redirect)
            self.end_headers()
        else:
            self.page(b"<!doctype html><title>nonce</title><p>served once")

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
threading.Thread(target=server.serve_forever, daemon=True).start()
time.sleep(lifetime)
server.shutdown()
