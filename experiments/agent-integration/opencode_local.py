"""Real OpenCode ACP against a local SSE model; no account, project edits or quota.

Usage: python3 experiments/agent-integration/opencode_local.py /absolute/path/to/opencode
Runs the Rust adapter acceptance with an isolated native database/configuration.
"""
import http.server
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import threading
import time


class Model(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", "0")))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        try:
            for delta in ["CODE", "SHEEP", "DOG", "_OK", ""]:
                event = {"id": "local-acceptance", "object": "chat.completion.chunk",
                         "created": 0, "model": "local-model", "choices": [{"index": 0,
                         "delta": {"role": "assistant", "content": delta},
                         "finish_reason": None if delta else "stop"}]}
                self.wfile.write(("data: " + json.dumps(event) + "\n\n").encode())
                self.wfile.flush()
                time.sleep(0.05)
            self.wfile.write(b"data: [DONE]\n\n")
        except (BrokenPipeError, ConnectionResetError):
            pass


def main():
    binary = Path(sys.argv[1]).resolve(strict=True)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="dogcodes-opencode-local-") as temp:
            root = Path(temp)
            for name in ["config", "home", "data", "xdg-config", "state"]:
                (root / name).mkdir()
            config = {"enabled_providers": ["local-test"], "model": "local-test/local-model",
                      "small_model": "local-test/local-model", "provider": {"local-test": {
                          "npm": "@ai-sdk/openai-compatible", "name": "Local acceptance",
                          "options": {"apiKey": "test-only", "baseURL": f"http://127.0.0.1:{server.server_port}/v1"},
                          "models": {"local-model": {"name": "Local model", "tool_call": True,
                                                      "limit": {"context": 32000, "output": 4000}}}}}}
            config_path = root / "config.json"
            config_path.write_text(json.dumps(config))
            settings = {"OPENCODE_CONFIG": str(config_path), "OPENCODE_CONFIG_DIR": str(root / "config"),
                        "OPENCODE_DISABLE_PROJECT_CONFIG": "true", "OPENCODE_TEST_HOME": str(root / "home"),
                        "XDG_DATA_HOME": str(root / "data"), "XDG_CONFIG_HOME": str(root / "xdg-config"),
                        "XDG_STATE_HOME": str(root / "state")}
            wrapper = root / "opencode"
            wrapper.write_text("#!/bin/sh\n" + "\n".join(f"export {key}={shlex.quote(value)}" for key, value in settings.items())
                               + f"\nexec {shlex.quote(str(binary))} \"$@\" 2>{shlex.quote(str(root / 'stderr.txt'))}\n")
            wrapper.chmod(0o700)
            env = dict(os.environ, DOGCODES_ACP_PROVIDER="opencode", DOGCODES_ACP_BINARY=str(wrapper))
            result = subprocess.run(["cargo", "test", "-p", "acp-driver", "real_new_agent_turn",
                                     "--", "--ignored", "--nocapture"], env=env, timeout=150)
            if result.returncode:
                # This child uses only the isolated test configuration and dummy key.
                print((root / 'stderr.txt').read_text(errors='replace')[-2000:])
            return result.returncode
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    sys.exit(main())
