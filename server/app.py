"""ELESSAR HTTP server.

    python3 -m server.app [--scenario change-cascade] [--port 8777] [--check]

Serves three things from one process:
  /mock/<pack>/<source>   the mock services, each payload in its own wire format
  /api/...                scenarios, state and actions
  /                       the web client
The pipeline fetches the mock services over real HTTP, so the server must be
threaded or it would deadlock requesting from itself.
"""
import argparse
import json
import mimetypes
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from . import pipeline, scenario

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WEB = os.path.join(ROOT, "web")


def load_dotenv(path):
    """Minimal .env reader: KEY=VALUE lines; real environment variables win."""
    if not os.path.isfile(path):
        return
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                key, value = line.split("=", 1)
                os.environ.setdefault(key.strip(), value.strip().strip("\"'"))


load_dotenv(os.path.join(ROOT, ".env"))

from . import agent, jev  # noqa: E402  (they read their model ids from the loaded environment)
from .session import Session  # noqa: E402


class App:
    def __init__(self, scenarios_dir, default):
        self.packs, self.failures = scenario.discover(scenarios_dir)
        if default not in self.packs:
            raise SystemExit(f"scenario '{default}' not found or invalid. Available: {sorted(self.packs)}")
        self.default = default
        self.sessions = {}
        self.base_url = None

    def session(self, pid):
        if pid not in self.packs:
            raise KeyError(pid)
        if pid not in self.sessions:
            self.sessions[pid] = Session(self.packs[pid], self.base_url)
        return self.sessions[pid]


def make_handler(app):
    class Handler(BaseHTTPRequestHandler):
        server_version = "elessar/1"

        def log_message(self, fmt, *args):
            sys.stderr.write(f"  {self.command} {self.path} -> {args[1] if len(args) > 1 else ''}\n")

        def _send(self, code, body, ctype="application/json", extra=None):
            data = body if isinstance(body, bytes) else body.encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(data)

        def _json(self, code, obj):
            self._send(code, json.dumps(obj))

        def do_GET(self):
            url = urlparse(self.path)
            parts = [p for p in url.path.split("/") if p]
            try:
                if parts[:1] == ["mock"] and len(parts) == 3:
                    return self._mock(parts[1], parts[2])
                if url.path == "/api/scenarios":
                    return self._json(200, {
                        "default": app.default,
                        "scenarios": [{
                            "id": p["id"], "title": p["title"], "premise": p["premise"],
                            "sources": len(p["sources"]), "pieces": len(p["layout"]["nodes"]),
                            "warnings": p["warnings"],
                        } for p in app.packs.values()],
                        "invalid": app.failures,
                    })
                if url.path == "/api/state":
                    pid = parse_qs(url.query).get("scenario", [app.default])[0]
                    return self._json(200, app.session(pid).public_state())
                return self._static(url.path)
            except KeyError as e:
                return self._json(404, {"error": f"unknown scenario or source {e}"})

        def do_POST(self):
            path = urlparse(self.path).path
            if path not in ("/api/action", "/api/agent"):
                return self._json(404, {"error": "not found"})
            try:
                length = int(self.headers.get("Content-Length", 0))
                payload = json.loads(self.rfile.read(length) or b"{}")
                session = app.session(payload.get("scenario", app.default))
                if path == "/api/agent":
                    if not agent.available():
                        return self._json(400, {"error": "The agent is off: add OPENROUTER_API_KEY to .env"})
                    message = str(payload.get("message", "")).strip()
                    if not message:
                        return self._json(400, {"error": "empty message"})
                    turn = session.ask_agent(message, payload.get("selected"))
                    return self._json(200, {"turn": turn, "state": session.public_state()})
                result = session.act(payload)
                return self._json(200, result)
            except KeyError as e:
                return self._json(400, {"error": f"missing or unknown field {e}"})
            except ValueError as e:
                return self._json(400, {"error": str(e)})

        def _mock(self, pid, sid):
            src = app.packs[pid]["sources"][sid]
            if src["transport"] != "file":
                return self._json(404, {"error": f"source '{sid}' is not served locally"})
            time.sleep(src["latency_ms"] / 1000)
            with open(src["path"], "rb") as f:
                body = f.read()
            self._send(200, body, pipeline.CONTENT_TYPES[src["format"]] + "; charset=utf-8",
                       {"X-Source-Format": src["format"], "X-Source-Name": src["name"]})

        def _static(self, path):
            rel = "index.html" if path in ("", "/") else path.lstrip("/")
            full = os.path.realpath(os.path.join(WEB, rel))
            if not full.startswith(os.path.realpath(WEB) + os.sep) or not os.path.isfile(full):
                return self._json(404, {"error": "not found"})
            ctype = mimetypes.guess_type(full)[0] or "application/octet-stream"
            if full.endswith(".js"):
                ctype = "text/javascript"
            with open(full, "rb") as f:
                self._send(200, f.read(), ctype)

    return Handler


def check(scenarios_dir):
    packs, failures = scenario.discover(scenarios_dir)
    for pid, p in packs.items():
        print(f"OK    {pid}: {len(p['sources'])} sources, {len(p['layout']['nodes'])} pieces, "
              f"board {p['layout']['width']}x{p['layout']['height']}")
        for w in p["warnings"]:
            print(f"      warning: {w}")
    for name, errs in failures.items():
        print(f"FAIL  {name}")
        for e in errs:
            print(f"      {e}")
    return 1 if failures else 0


def main():
    ap = argparse.ArgumentParser(description="ELESSAR: fuse disparate sources onto a Bayesian checkers board.")
    ap.add_argument("--scenario", default="change-cascade", help="default scenario pack id")
    ap.add_argument("--scenarios-dir", default=os.path.join(ROOT, "scenarios"))
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8777)
    ap.add_argument("--check", action="store_true", help="validate every scenario pack and exit")
    args = ap.parse_args()

    if args.check:
        sys.exit(check(args.scenarios_dir))

    app = App(args.scenarios_dir, args.scenario)
    server = ThreadingHTTPServer((args.host, args.port), make_handler(app))
    app.base_url = f"http://{args.host}:{server.server_address[1]}"
    print(f"ELESSAR on {app.base_url}  (scenario: {args.scenario}; packs: {', '.join(sorted(app.packs))})")
    print(f"  decision model (Jev): {jev.MODEL if jev.available() else 'off (no OPENROUTER_API_KEY)'}")
    print(f"  inference model (agent): {agent.MODEL if agent.available() else 'off (no OPENROUTER_API_KEY)'}")
    for name, errs in app.failures.items():
        print(f"  skipped invalid pack '{name}': {errs[0]}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
