"""
Proxy for Claude Code → anonkey.st that translates
Anthropic Messages API → OpenAI Chat Completions API.

Strips unsupported fields (user, metadata, etc.) and handles
tool use conversion between the two formats.
"""
import http.server
import json
import os
import ssl
import sys
import urllib.request

BASE_URL = os.environ.get("ANONKEY_BASE_URL", "https://anonkey.st/v1")
API_KEY = os.environ["ANONKEY_API_KEY"]
MODEL = os.environ.get("ANONKEY_MODEL", "gpt-5.5")
PORT = int(os.environ.get("ANONKEY_PROXY_PORT", "18890"))


def anthropic_to_openai_messages(messages, system=None):
    """Convert Anthropic message format to OpenAI format."""
    oai_messages = []

    if system:
        if isinstance(system, str):
            oai_messages.append({"role": "system", "content": system})
        elif isinstance(system, list):
            text_parts = []
            for block in system:
                if isinstance(block, dict) and block.get("type") == "text":
                    text_parts.append(block.get("text", ""))
                elif isinstance(block, str):
                    text_parts.append(block)
            if text_parts:
                oai_messages.append({"role": "system", "content": "\n".join(text_parts)})

    for msg in messages:
        role = msg.get("role", "user")
        content = msg.get("content")

        if isinstance(content, str):
            oai_messages.append({"role": role, "content": content})
        elif isinstance(content, list):
            # Handle content blocks
            text_parts = []
            tool_calls = []
            tool_results = []

            for block in content:
                if isinstance(block, str):
                    text_parts.append(block)
                    continue
                block_type = block.get("type", "")

                if block_type == "text":
                    text_parts.append(block.get("text", ""))
                elif block_type == "tool_use":
                    tool_calls.append({
                        "id": block.get("id", ""),
                        "type": "function",
                        "function": {
                            "name": block.get("name", ""),
                            "arguments": json.dumps(block.get("input", {})),
                        },
                    })
                elif block_type == "tool_result":
                    result_content = block.get("content", "")
                    if isinstance(result_content, list):
                        parts = []
                        for rc in result_content:
                            if isinstance(rc, dict) and rc.get("type") == "text":
                                parts.append(rc.get("text", ""))
                            elif isinstance(rc, str):
                                parts.append(rc)
                        result_content = "\n".join(parts)
                    tool_results.append({
                        "role": "tool",
                        "tool_call_id": block.get("tool_use_id", ""),
                        "content": str(result_content),
                    })
                elif block_type == "image":
                    # Skip images - can't translate easily
                    text_parts.append("[image]")

            if role == "assistant":
                msg_obj = {"role": "assistant"}
                if text_parts:
                    msg_obj["content"] = "\n".join(text_parts)
                else:
                    msg_obj["content"] = None
                if tool_calls:
                    msg_obj["tool_calls"] = tool_calls
                oai_messages.append(msg_obj)
            elif role == "user":
                # Tool results from user role
                if tool_results:
                    for tr in tool_results:
                        oai_messages.append(tr)
                if text_parts:
                    oai_messages.append({"role": "user", "content": "\n".join(text_parts)})
            else:
                if text_parts:
                    oai_messages.append({"role": role, "content": "\n".join(text_parts)})
        else:
            oai_messages.append({"role": role, "content": str(content) if content else ""})

    return oai_messages


def anthropic_to_openai_tools(tools):
    """Convert Anthropic tool definitions to OpenAI function format."""
    oai_tools = []
    for tool in tools:
        if tool.get("type") == "custom" or "name" not in tool:
            continue
        oai_tools.append({
            "type": "function",
            "function": {
                "name": tool["name"],
                "description": tool.get("description", ""),
                "parameters": tool.get("input_schema", {"type": "object", "properties": {}}),
            },
        })
    return oai_tools


def openai_to_anthropic_response(oai_resp, model_name):
    """Convert OpenAI chat completion response to Anthropic Messages response."""
    choice = oai_resp.get("choices", [{}])[0]
    message = choice.get("message", {})
    usage = oai_resp.get("usage", {})

    content = []
    if message.get("content"):
        content.append({"type": "text", "text": message["content"]})

    for tc in message.get("tool_calls", []):
        func = tc.get("function", {})
        try:
            input_data = json.loads(func.get("arguments", "{}"))
        except json.JSONDecodeError:
            input_data = {}
        content.append({
            "type": "tool_use",
            "id": tc.get("id", ""),
            "name": func.get("name", ""),
            "input": input_data,
        })

    stop_reason = "end_turn"
    finish = choice.get("finish_reason", "")
    if finish == "tool_calls":
        stop_reason = "tool_use"
    elif finish == "length":
        stop_reason = "max_tokens"

    return {
        "id": "msg_" + oai_resp.get("id", "unknown"),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model_name,
        "stop_reason": stop_reason,
        "stop_sequence": None,
        "usage": {
            "input_tokens": usage.get("prompt_tokens", 0),
            "output_tokens": usage.get("completion_tokens", 0),
        },
    }


def forward_to_openai(oai_body):
    """Send request to anonkey.st OpenAI endpoint."""
    data = json.dumps(oai_body).encode()
    req = urllib.request.Request(
        f"{BASE_URL}/chat/completions",
        data=data,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {API_KEY}",
        },
        method="POST",
    )
    ctx = ssl.create_default_context()
    resp = urllib.request.urlopen(req, timeout=120, context=ctx)
    return json.loads(resp.read())


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(length))

        if "/messages" in self.path:
            self.handle_messages(body)
        else:
            # Unknown endpoint
            self.send_response(404)
            self.end_headers()
            self.wfile.write(b'{"error": "not found"}')

    def handle_messages(self, body):
        messages = body.get("messages", [])
        system = body.get("system")
        tools = body.get("tools", [])
        max_tokens = body.get("max_tokens", 4096)
        temperature = body.get("temperature")
        model_requested = body.get("model", MODEL)
        stream = body.get("stream", False)

        oai_messages = anthropic_to_openai_messages(messages, system)
        oai_body = {
            "model": MODEL,
            "messages": oai_messages,
            "max_tokens": max_tokens,
        }
        if temperature is not None:
            oai_body["temperature"] = temperature
        if tools:
            oai_tools = anthropic_to_openai_tools(tools)
            if oai_tools:
                oai_body["tools"] = oai_tools

        try:
            oai_resp = forward_to_openai(oai_body)
        except urllib.error.HTTPError as e:
            err_body = e.read().decode()
            self.send_response(e.code)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            error_resp = {"type": "error", "error": {"type": "api_error", "message": err_body}}
            self.wfile.write(json.dumps(error_resp).encode())
            return
        except Exception as e:
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            error_resp = {"type": "error", "error": {"type": "api_error", "message": str(e)}}
            self.wfile.write(json.dumps(error_resp).encode())
            return

        anthropic_resp = openai_to_anthropic_response(oai_resp, model_requested)

        if stream:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.write_sse(anthropic_resp)
        else:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(anthropic_resp).encode())

    def write_sse(self, resp):
        """Write Anthropic SSE streaming events."""
        events = []
        events.append({"type": "message_start", "message": {**resp, "content": []}})

        for idx, block in enumerate(resp.get("content", [])):
            events.append({"type": "content_block_start", "index": idx, "content_block": block if block["type"] == "tool_use" else {"type": "text", "text": ""}})
            if block["type"] == "text":
                events.append({"type": "content_block_delta", "index": idx, "delta": {"type": "text_delta", "text": block["text"]}})
            elif block["type"] == "tool_use":
                events.append({"type": "content_block_delta", "index": idx, "delta": {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}})
            events.append({"type": "content_block_stop", "index": idx})

        events.append({"type": "message_delta", "delta": {"stop_reason": resp.get("stop_reason", "end_turn"), "stop_sequence": None}, "usage": resp.get("usage", {})})
        events.append({"type": "message_stop"})

        for event in events:
            line = f"event: {event['type']}\ndata: {json.dumps(event)}\n\n"
            self.wfile.write(line.encode())
            self.wfile.flush()

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"status": "ok"}')

    def log_message(self, format, *args):
        pass


if __name__ == "__main__":
    server = http.server.HTTPServer(("127.0.0.1", PORT), Handler)
    print(f"claude proxy ready on port {PORT}", file=sys.stderr, flush=True)
    server.serve_forever()
