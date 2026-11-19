import json
import os
import selectors
import subprocess
import sys
import time

program, session, marker = sys.argv[1:]


def verify(process):
    pending = bytearray()
    with selectors.DefaultSelector() as ready:
        ready.register(process.stdout, selectors.EVENT_READ)

        def send(message):
            process.stdin.write(json.dumps(message).encode() + b"\n")
            process.stdin.flush()

        def request(identifier, method, params):
            send({"id": identifier, "method": method, "params": params})
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                while b"\n" in pending:
                    line, _, remainder = pending.partition(b"\n")
                    pending[:] = remainder
                    event = json.loads(line)
                    if event.get("id") == identifier:
                        return event
                if not ready.select(max(0, deadline - time.monotonic())):
                    break
                chunk = os.read(process.stdout.fileno(), 65536)
                if not chunk:
                    return {"failure": "closed"}
                pending.extend(chunk)
                if len(pending) > 4 * 1024 * 1024:
                    return {"failure": "oversized"}
            return {"failure": "timeout"}

        initialized = request(1, "initialize", {"clientInfo": {"name": "tugboat-proof", "version": "1.0.0"}})
        if "result" not in initialized:
            return "initialize-" + initialized.get("failure", "refused")
        send({"method": "initialized"})
        result = request(2, "thread/resume", {"threadId": session, "excludeTurns": False})
        if "result" not in result:
            code = result.get("error", {}).get("code")
            return f"resume-rpc-{code}" if isinstance(code, int) else "resume-" + result.get("failure", "refused")
        thread = result["result"]["thread"]
        if thread["id"] != session or thread["cwd"] != "/work/sample project":
            return "wrong-thread"
        replies = [item["text"] for turn in thread["turns"] for item in turn["items"]
                   if item.get("type") == "agentMessage" and isinstance(item.get("text"), str)]
        if not replies or marker not in replies[-1]:
            return "missing-historical-assistant-reply"
        return "exact-native-resume"


try:
    with subprocess.Popen(
        [program, "-c", 'cli_auth_credentials_store="file"', "app-server", "--listen", "stdio://"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    ) as process:
        try:
            print(verify(process), flush=True)
        finally:
            if process.poll() is None:
                process.kill()
except (OSError, ValueError, KeyError, TypeError):
    print("unsupported-local-protocol", flush=True)
