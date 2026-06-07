"""
Proxy for codex → anonkey.st that:
1. Strips hosted tool types (web_search, tool_search, custom)
2. Converts non-streaming responses into SSE stream format codex expects
"""
import http.server
import json
import os
import ssl
import sys
import urllib.request

BASE_URL = os.environ.get("ANONKEY_BASE_URL", "https://anonkey.st/v1")
API_KEY = os.environ["ANONKEY_API_KEY"]
PORT = int(os.environ.get("ANONKEY_PROXY_PORT", "18888"))

HOSTED_TOOLS = {"web_search", "web_search_preview", "code_interpreter", "file_search", "tool_search", "custom"}


def forward_get(path, auth):
    req = urllib.request.Request(
        f"{BASE_URL}{path}",
        headers={"Authorization": auth},
    )
    ctx = ssl.create_default_context()
    return urllib.request.urlopen(req, timeout=30, context=ctx).read()


def forward_post(path, body, auth):
    data = json.dumps(body).encode()
    req = urllib.request.Request(
        f"{BASE_URL}{path}",
        data=data,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {API_KEY}",
        },
        method="POST",
    )
    ctx = ssl.create_default_context()
    return urllib.request.urlopen(req, timeout=120, context=ctx).read()


def response_to_sse(resp_json):
    """Convert a completed Response object into the SSE event stream codex expects."""
    resp = json.loads(resp_json)
    events = []

    events.append(("response.created", {"type": "response.created", "sequence_number": 0, "response": {**resp, "status": "in_progress", "output": []}}))
    events.append(("response.in_progress", {"type": "response.in_progress", "sequence_number": 1, "response": {**resp, "status": "in_progress", "output": []}}))

    seq = 2
    for oi, item in enumerate(resp.get("output", [])):
        events.append(("response.output_item.added", {"type": "response.output_item.added", "sequence_number": seq, "output_index": oi, "item": {**item, "status": "in_progress", "content": []}}))
        seq += 1

        for ci, part in enumerate(item.get("content", [])):
            events.append(("response.content_part.added", {"type": "response.content_part.added", "sequence_number": seq, "output_index": oi, "item_id": item.get("id", ""), "content_index": ci, "part": {**part, "text": ""} if part.get("type") == "output_text" else part}))
            seq += 1

            if part.get("type") == "output_text" and part.get("text"):
                events.append(("response.output_text.delta", {"type": "response.output_text.delta", "sequence_number": seq, "output_index": oi, "item_id": item.get("id", ""), "content_index": ci, "delta": part["text"]}))
                seq += 1
                events.append(("response.output_text.done", {"type": "response.output_text.done", "sequence_number": seq, "output_index": oi, "item_id": item.get("id", ""), "content_index": ci, "text": part["text"]}))
                seq += 1

            events.append(("response.content_part.done", {"type": "response.content_part.done", "sequence_number": seq, "output_index": oi, "item_id": item.get("id", ""), "content_index": ci, "part": part}))
            seq += 1

        events.append(("response.output_item.done", {"type": "response.output_item.done", "sequence_number": seq, "output_index": oi, "item": item}))
        seq += 1

    events.append(("response.completed", {"type": "response.completed", "sequence_number": seq, "response": resp}))

    lines = []
    for event_type, data in events:
        lines.append(f"event: {event_type}")
        lines.append(f"data: {json.dumps(data)}")
        lines.append("")
    return "\n".join(lines) + "\n"


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length))

        wants_stream = body.get("stream", False)

        # Strip hosted tools
        if "tools" in body:
            body["tools"] = [t for t in body["tools"] if t.get("type") not in HOSTED_TOOLS]

        # Always request non-streaming from backend
        body["stream"] = False

        try:
            result = forward_post(self.path, body, self.headers.get("Authorization", ""))
        except urllib.error.HTTPError as e:
            err = e.read()
            self.send_response(e.code)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(err)
            return

        if wants_stream:
            sse = response_to_sse(result)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.wfile.write(sse.encode())
        else:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(result)

    def do_GET(self):
        try:
            result = forward_get(self.path, f"Bearer {API_KEY}")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(result)
        except urllib.error.HTTPError as e:
            self.send_response(e.code)
            self.end_headers()
            self.wfile.write(e.read())

    def log_message(self, format, *args):
        pass


if __name__ == "__main__":
    server = http.server.HTTPServer(("127.0.0.1", PORT), Handler)
    print(f"anonkey proxy ready on port {PORT}", file=sys.stderr, flush=True)
    server.serve_forever()
