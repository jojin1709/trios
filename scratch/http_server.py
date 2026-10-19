import http.server
import socketserver
import sys
import os

PORT = 8080
LOG_FILE = os.path.join(os.path.dirname(__file__), "http_server.log")

def log(msg):
    with open(LOG_FILE, "a") as f:
        f.write(msg + "\n")
        f.flush()
    sys.stderr.write(msg + "\n")
    sys.stderr.flush()

class TestHandler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        log(f"[http_server] received GET request from {self.client_address}: path={self.path}")
        body = b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 54\r\nConnection: close\r\n\r\nHello from Host! TriOS Phase 26 Networking v2 Verified!\n"
        self.wfile.write(body)
        log("[http_server] responded with HTTP 200 OK")

    def log_message(self, format, *args):
        log(f"[http_server] {self.address_string()} - {format%args}")

# Clear log
with open(LOG_FILE, "w") as f:
    f.write(f"Starting server on 0.0.0.0:{PORT}...\n")

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("0.0.0.0", PORT), TestHandler) as httpd:
    log(f"Serving HTTP on 0.0.0.0 port {PORT}...")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
