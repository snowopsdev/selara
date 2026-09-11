"""Controlled subprocess for testing Selara's protocol client, never real auth."""
import json
import os
import sys
import threading
import time

mode = sys.argv[1]
assert os.path.isdir(os.environ["CODEX_HOME"])
writer = threading.Lock()


def send(value):
    with writer:
        print(json.dumps(value), flush=True)


def reply(request, value):
    send({"id": request["id"], "result": value})


def event(method, params):
    send({"method": method, "params": params})


def handle(request):
    method = request["method"]
    params = request.get("params", {})
    if method == "initialize":
        assert params.get("selaraWritingMode") == 1
        reply(request, {"selaraWritingMode": 0 if mode == "wrong_capability" else 1})
    elif method == "initialized":
        pass
    elif method == "test/echo":
        time.sleep(params.get("delay", 0))
        event("account/updated", {})
        reply(request, params["value"])
    elif method == "test/hang":
        pass
    elif method == "test/exit":
        os._exit(0)
    elif method == "account/login/start":
        if mode == "slow_login":
            time.sleep(0.1)
        reply(request, {"loginId": "login", "authUrl": "https://auth.openai.com/authorize" if mode != "bad_url" else "https://example.com/steal"})
        if mode == "login_complete":
            event("account/login/completed", {"loginId": "login", "success": True})
    elif method == "account/login/cancel":
        event("fixture/cancelled", params)
        reply(request, {})
    elif method == "model/list":
        if mode == "broken_models":
            reply(request, {"data": [{"id": "one"}], "nextCursor": "repeat"})
        elif params.get("cursor"):
            reply(request, {"data": [{"id": "one"}, {"id": "two"}], "nextCursor": None})
        else:
            reply(request, {"data": [{"id": "one"}, {"id": "hidden", "hidden": True}], "nextCursor": "next"})
    elif method == "thread/start":
        assert params == {"ephemeral": True, "model": "model", "baseInstructions": "system"}
        reply(request, {"thread": {"id": "thread"}})
    elif method == "turn/start":
        assert params["input"] == [{"type": "text", "text": "selection"}]
        if mode == "start_failure":
            send({"id": request["id"], "error": {"message": "injected turn failure"}})
            return
        # Notifications may arrive before the RPC result. No deltas, other
        # threads, or server diagnostic text may become replacement text.
        event("item/completed", {"threadId": "other", "turnId": "turn", "item": {"type": "agentMessage", "id": "other", "text": "wrong"}})
        event("item/agentMessage/delta", {"threadId": "thread", "turnId": "turn", "delta": "partial"})
        if mode != "empty":
            event("item/completed", {"threadId": "thread", "turnId": "turn", "item": {"id": "text", "type": "toolCall" if mode == "tool" else "agentMessage", "text": "Complete rewrite"}})
        reply(request, {"turn": {"id": "turn"}})
        if mode == "disconnect":
            os._exit(0)
        event("thread/tokenUsage/updated", {"threadId": "thread", "tokenUsage": {"last": {"inputTokens": 12, "outputTokens": 4}}})
        event("turn/completed", {"threadId": "thread", "turn": {"id": "turn", "status": mode if mode in ["failed", "interrupted", "incomplete"] else "completed"}})
    elif method in ["thread/unsubscribe", "turn/interrupt"]:
        event("fixture/cleanup", {"method": method, **params})
        reply(request, {})
    else:
        raise AssertionError(method)


for line in sys.stdin:
    request = json.loads(line)
    if request["method"] in ["test/echo", "account/login/start"]:
        threading.Thread(target=handle, args=(request,), daemon=True).start()
    else:
        handle(request)
