"""A fake HTTP proxy for scripts/chrome-shots.sh: gives the engine installer a download that fails or stalls.

Usage: python -P scripts/fake_proxy.py <port-file> <mode-file> <seconds> < /dev/null

Writes its port to <port-file>, serves for <seconds>, and answers each CONNECT by <mode-file>'s current
content: `refuse` replies 502 (the install fails at once), anything else opens the tunnel and then says
nothing (the install sits in Installing with no bytes, until the client gives up). The mode is read per
connection, so a script can switch it between installs.
"""

import os
import pathlib
import socket
import sys
import threading
import time

port_file, mode_file, lifetime = sys.argv[1], sys.argv[2], float(sys.argv[3])
server = socket.socket()
server.bind(("127.0.0.1", 0))
server.listen(8)
pathlib.Path(port_file).write_text(str(server.getsockname()[1]))


def answer(conn):
    try:
        conn.recv(65536)
        if pathlib.Path(mode_file).read_text().strip() == "refuse":
            conn.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
        else:
            conn.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
            time.sleep(lifetime)
    except OSError:
        pass
    conn.close()


threading.Timer(lifetime, lambda: os._exit(0)).start()
while True:
    client, _ = server.accept()
    threading.Thread(target=answer, args=(client,), daemon=True).start()
