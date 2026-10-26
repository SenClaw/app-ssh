#!/usr/bin/env python3
"""The two SenClaw routes the command guard calls, answered by a script.

`POST /api/space/apps/<id>/bridge` (action llm.request) and
`POST /api/decision/ask`. A command containing `--status` is low risk and
"view"; `--apply` is high risk and "change"; anything else is medium.
`GET /calls` reports how many times each was called.

    python3 -I scripts/fake-senclaw-bridge.py <port>
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CALLS = {"llm": 0, "decision": 0}


def verdict(text):
    if "--status" in text:
        return "low", "view"
    if "--apply" in text:
        return "high", "change"
    return "medium", "view"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, body, status=200):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/calls":
            return self.reply(CALLS)
        self.reply({"error": "not found"}, 404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if self.path.startswith("/api/space/apps/") and self.path.endswith("/bridge"):
            if body.get("action") != "llm.request":
                return self.reply({"status": "error", "message": "unknown action"})
            CALLS["llm"] += 1
            risk, _ = verdict(body["payload"]["prompt"])
            text = "```json\n" + json.dumps({"risk": risk, "reason": f"scripted {risk}"}) + "\n```"
            return self.reply({"status": "ok", "text": text, "model": "fake-model", "finish": "stop"})
        if self.path == "/api/decision/ask":
            CALLS["decision"] += 1
            _, choice = verdict(body.get("state", ""))
            return self.reply({"answers": {"answer": {"choice": choice, "answer_confidence": 0.9}}})
        self.reply({"error": "not found"}, 404)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 28788
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
