"""Generate differential specs for the Codex adapter (run the reference on each)."""
import base64
import json
import os
import sys

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)


def b64url(obj):
    raw = json.dumps(obj, separators=(",", ":")).encode()
    return base64.urlsafe_b64encode(raw).decode().rstrip("=")


# Payload chosen so its base64 has no '-'/'_' (the reference decodes with atob).
JWT = ".".join([
    b64url({"alg": "none"}),
    b64url({"https://api.openai.com/auth": {"chatgpt_account_id": "acct_123"}}),
    "sig",
])
assert "-" not in JWT.split(".")[1] and "_" not in JWT.split(".")[1], JWT

MODEL = {
    "id": "gpt-5.5",
    "name": "GPT-5.5",
    "api": "openai-codex-responses",
    "provider": "openai-codex",
    "baseUrl": "http://127.0.0.1:1",
    "reasoning": True,
    "input": ["text", "image"],
    "cost": {"input": 5, "output": 30, "cacheRead": 0.5, "cacheWrite": 0},
    "contextWindow": 272000,
    "maxTokens": 128000,
    "thinkingLevelMap": {"xhigh": "xhigh", "minimal": "low"},
    "headers": {"x-model-header": "m"},
    "compat": {},
}

USAGE0 = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
          "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}}


def sse(events, done=True):
    out = ": keepalive\n\n"
    for ev in events:
        out += f"event: {ev['type']}\ndata: {json.dumps(ev, ensure_ascii=False)}\n\n"
    if done:
        out += "data: [DONE]\n\n"
    return out


RICH_EVENTS = [
    {"type": "response.created", "response": {"id": "resp_1", "status": "in_progress"}},
    {"type": "response.output_item.added", "output_index": 0,
     "item": {"type": "reasoning", "id": "rs_9", "summary": []}},
    {"type": "response.reasoning_summary_text.delta", "output_index": 0, "delta": "Thinking about "},
    {"type": "response.reasoning_summary_text.delta", "output_index": 0, "delta": "files"},
    {"type": "response.reasoning_summary_part.done", "output_index": 0},
    {"type": "response.reasoning_summary_text.delta", "output_index": 0, "delta": "Second part"},
    {"type": "response.reasoning_summary_part.done", "output_index": 0},
    {"type": "response.output_item.done", "output_index": 0,
     "item": {"type": "reasoning", "id": "rs_9",
              "summary": [{"type": "summary_text", "text": "Thinking about files"},
                          {"type": "summary_text", "text": "Second part"}],
              "encrypted_content": "ENC9"}},
    {"type": "response.output_item.added", "output_index": 1,
     "item": {"type": "message", "id": "msg_9", "role": "assistant", "content": [], "phase": "commentary"}},
    {"type": "response.output_text.delta", "output_index": 1, "delta": "Let me read héllo \U0001F600 "},
    {"type": "response.output_text.delta", "output_index": 1, "delta": "the file."},
    {"type": "response.output_item.done", "output_index": 1,
     "item": {"type": "message", "id": "msg_9", "role": "assistant", "phase": "commentary",
              "content": [{"type": "output_text", "text": "Let me read héllo \U0001F600 the file.",
                           "annotations": []}]}},
    {"type": "response.output_item.added", "output_index": 2,
     "item": {"type": "function_call", "id": "fc_9", "call_id": "call_9", "name": "read", "arguments": ""}},
    {"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "{\"path\":"},
    {"type": "response.function_call_arguments.delta", "output_index": 2, "delta": "\"b.txt\"}"},
    {"type": "response.function_call_arguments.done", "output_index": 2, "arguments": "{\"path\":\"b.txt\"}"},
    {"type": "response.output_item.done", "output_index": 2,
     "item": {"type": "function_call", "id": "fc_9", "call_id": "call_9", "name": "read",
              "arguments": "{\"path\":\"b.txt\"}"}},
    {"type": "response.completed",
     "response": {"id": "resp_1", "status": "completed", "end_turn": False, "service_tier": "default",
                  "usage": {"input_tokens": 1000, "input_tokens_details": {"cached_tokens": 200},
                            "output_tokens": 300, "output_tokens_details": {"reasoning_tokens": 50},
                            "total_tokens": 1300}}},
]

SAME_MODEL_ASSISTANT = {
    "role": "assistant",
    "content": [
        {"type": "thinking", "thinking": "thought",
         "thinkingSignature": json.dumps({"type": "reasoning", "id": "rs_1", "summary": [],
                                          "encrypted_content": "enc"}, separators=(",", ":"))},
        {"type": "text", "text": "Hi there",
         "textSignature": json.dumps({"v": 1, "id": "msg_1", "phase": "final_answer"}, separators=(",", ":"))},
        {"type": "toolCall", "id": "call_1|fc_1", "name": "read", "arguments": {"path": "a.txt"}},
    ],
    "api": "openai-codex-responses", "provider": "openai-codex", "model": "gpt-5.5",
    "usage": USAGE0, "stopReason": "toolUse", "timestamp": 2,
}

TOOLS = [{"name": "read", "description": "Read a file",
          "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}]

specs = {}

specs["full"] = {
    "model": MODEL,
    "systemPrompt": "You are Pipkin.",
    "messages": [
        {"role": "user", "content": [{"type": "text", "text": "Hello"},
                                     {"type": "image", "data": "AAAA", "mimeType": "image/png"}], "timestamp": 1},
        SAME_MODEL_ASSISTANT,
        {"role": "toolResult", "toolCallId": "call_1|fc_1", "toolName": "read",
         "content": [{"type": "text", "text": "file body"}], "isError": False, "timestamp": 3},
        {"role": "user", "content": [{"type": "text", "text": "Thanks"}], "timestamp": 4},
    ],
    "tools": TOOLS,
    "options": {"apiKey": JWT, "transport": "sse", "sessionId": "sess-1", "reasoningEffort": "high",
                "headers": {"x-extra": "e", "x-model-header": None}},
    "replies": [{"sse": [sse(RICH_EVENTS)]}],
}

specs["plain"] = {
    "model": {**MODEL, "thinkingLevelMap": {"off": "minimal", "xhigh": "xhigh"}},
    "systemPrompt": "",
    "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}], "timestamp": 1}],
    "tools": [],
    "options": {"apiKey": JWT, "transport": "sse", "sessionId": "sess-2", "cacheRetention": "none",
                "textVerbosity": "medium", "serviceTier": "flex", "temperature": 0.5, "toolChoice": "none",
                "reasoningSummary": "detailed"},
    "replies": [{"sse": [sse([
        {"type": "response.output_item.added", "output_index": 0,
         "item": {"type": "message", "id": "msg_2", "role": "assistant", "content": []}},
        {"type": "response.output_text.delta", "output_index": 0, "delta": "Hello!"},
        {"type": "response.output_item.done", "output_index": 0,
         "item": {"type": "message", "id": "msg_2", "role": "assistant",
                  "content": [{"type": "output_text", "text": "Hello!", "annotations": []}]}},
        {"type": "response.done",
         "response": {"id": "resp_2", "status": "completed", "service_tier": "flex",
                      "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}}},
    ], done=False)]}],
}

LONG_SESSION = "s" * 70 + "é" * 10
specs["cross_model"] = {
    "model": {**MODEL, "input": ["text"], "thinkingLevelMap": {"off": None, "minimal": "low"}},
    "systemPrompt": "sys",
    "messages": [
        {"role": "user", "content": [{"type": "text", "text": "look"},
                                     {"type": "image", "data": "AAAA", "mimeType": "image/png"},
                                     {"type": "image", "data": "BBBB", "mimeType": "image/png"}], "timestamp": 1},
        {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "foreign thought", "thinkingSignature": "sig"},
            {"type": "text", "text": "foreign text", "textSignature": "msg_foreign"},
            {"type": "toolCall", "id": "call_x|item.with.dots", "name": "read", "arguments": {"path": "x"}},
        ], "api": "openai-responses", "provider": "openai", "model": "gpt-4o",
            "usage": USAGE0, "stopReason": "toolUse", "timestamp": 2},
        {"role": "toolResult", "toolCallId": "call_x|item.with.dots", "toolName": "read",
         "content": [{"type": "text", "text": "out"}, {"type": "image", "data": "CCCC", "mimeType": "image/png"}],
         "isError": False, "timestamp": 3},
        {"role": "assistant", "content": [{"type": "text", "text": "broken"}],
         "api": "openai-codex-responses", "provider": "openai-codex", "model": "gpt-5.5",
         "usage": USAGE0, "stopReason": "error", "errorMessage": "boom", "timestamp": 4},
        {"role": "assistant", "content": [
            {"type": "text", "text": "older model text", "textSignature": "x" * 80},
            {"type": "toolCall", "id": "call_o|fc_o", "name": "read", "arguments": {"path": "o"}},
        ], "api": "openai-codex-responses", "provider": "openai-codex", "model": "gpt-5.4",
            "usage": USAGE0, "stopReason": "toolUse", "timestamp": 5},
        {"role": "user", "content": [{"type": "text", "text": "next"}], "timestamp": 6},
        {"role": "assistant", "content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}],
         "api": "openai-codex-responses", "provider": "openai-codex", "model": "gpt-5.5",
         "usage": USAGE0, "stopReason": "stop", "timestamp": 7},
        {"role": "user", "content": [], "timestamp": 8},
        {"role": "user", "content": [{"type": "text", "text": "last"}], "timestamp": 9},
    ],
    "tools": TOOLS,
    "options": {"apiKey": JWT, "transport": "sse", "sessionId": LONG_SESSION},
    "replies": [{"sse": [sse([
        {"type": "response.output_item.added", "output_index": 0,
         "item": {"type": "message", "id": "msg_3", "role": "assistant", "content": []}},
        {"type": "response.output_text.delta", "output_index": 0, "delta": "partial"},
        {"type": "response.incomplete",
         "response": {"id": "resp_3", "status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"},
                      "usage": {"input_tokens": 50, "input_tokens_details": {"cached_tokens": 10,
                                                                             "cache_write_tokens": 5},
                                "output_tokens": 7, "total_tokens": 57}}},
    ])]}],
}

specs["content_filter"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"sse": [sse([
        {"type": "response.incomplete",
         "response": {"id": "resp_4", "status": "incomplete", "incomplete_details": {"reason": "content_filter"}}},
    ])]}],
}

specs["failed_event"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"sse": [sse([
        {"type": "response.created", "response": {"id": "resp_5"}},
        {"type": "response.failed",
         "response": {"id": "resp_5", "status": "failed", "error": {"code": "server_error", "message": "The model crashed"}}},
    ])]}],
}

specs["error_event"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"sse": [sse([
        {"type": "error", "error": {"type": "invalid_request_error", "code": "context_length_exceeded",
                                    "message": "Your input exceeds the context window of this model."}},
    ])]}],
}

specs["no_terminal"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"sse": [sse([
        {"type": "response.output_item.added", "output_index": 0,
         "item": {"type": "message", "id": "msg_6", "role": "assistant", "content": []}},
        {"type": "response.output_text.delta", "output_index": 0, "delta": "cut"},
    ], done=False)]}],
}

specs["unfinished_tool"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"sse": [sse([
        {"type": "response.output_item.added", "output_index": 0,
         "item": {"type": "function_call", "id": "fc_7", "call_id": "call_7", "name": "read", "arguments": ""}},
        {"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"pa"},
        {"type": "response.completed", "response": {"id": "resp_7", "status": "completed"}},
    ])]}],
}

specs["invalid_json"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"sse": ["data: {not json}\n\n"]}],
}

specs["http_400"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"status": 400, "body": json.dumps({"error": {"message": "Unsupported parameter: foo",
                                                              "type": "invalid_request_error"}})}],
}

specs["http_500_text"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"status": 500, "body": "upstream connect error or disconnect/reset before headers"}],
}

specs["http_429_rate"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"status": 429, "body": json.dumps({"error": {"type": "rate_limit_exceeded", "message": "slow down"}})}],
}

specs["usage_limit_no_reset"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse"},
    "replies": [{"status": 429, "body": json.dumps({"error": {"code": "usage_limit_reached", "plan_type": "PLUS",
                                                              "message": "The usage limit has been reached"}})}],
}

PLAIN_SSE = specs["plain"]["replies"][0]

specs["retry_503"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse", "maxRetries": 2},
    "replies": [{"status": 503, "body": "overloaded", "headers": {"retry-after-ms": "10"}}, PLAIN_SSE],
}

specs["retry_delay_cap"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse", "maxRetries": 1},
    "replies": [{"status": 503, "body": "busy", "headers": {"retry-after": "120"}}, PLAIN_SSE],
}

specs["retry_400_catch"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse", "maxRetries": 1},
    "replies": [{"status": 400, "body": json.dumps({"error": {"message": "bad input"}})}],
}

specs["retry_usage_limit_429"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse", "maxRetries": 1},
    "replies": [{"status": 429, "headers": {"retry-after-ms": "5"},
                 "body": json.dumps({"error": {"code": "usage_limit_reached", "plan_type": "PRO"}})}],
}

specs["retry_terminal_429"] = {
    **specs["plain"],
    "options": {"apiKey": JWT, "transport": "sse", "maxRetries": 2},
    "replies": [{"status": 429, "body": json.dumps({"error": {"type": "insufficient_quota", "message": "You exceeded your current quota"}})}],
}

for name, spec in specs.items():
    with open(os.path.join(OUT, f"{name}.spec.json"), "w") as f:
        json.dump(spec, f, ensure_ascii=False, indent=1)
print(" ".join(specs))
