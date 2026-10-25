#!/usr/bin/env python3
"""Заглушка llama-server для проверки агента pi.

Отвечает тремя ответами, записанными с живого сервера (сборка b11734, замер на
5090 25.10.2026), и этим позволяет проверить расширение без карты и без модели:

* ``GET /models`` (и ``/v1/models``) — список из одной модели с окном в
  ``meta.n_ctx``. Именно это поле расширение и читает;
* ``GET /props`` — шаблон чата и число мест;
* ``POST /v1/chat/completions`` — минимальный ответ, обычный или потоком.

Запрос к ``/v1/chat/completions`` при этом целиком складывается в файл
``--dump``: только так видно, что до модели дошли и правила агента, и скиллы.

``--shape no-meta`` убирает из ответа ``meta`` — это чужой шлюз, который
окна не называет. Расширение обязано тогда не регистрировать модель вовсе:
объявленное наугад окно означает тихо глупеющего агента.
"""
import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

MODEL = "GLM-4.7-Flash-UD-Q4_K_XL"
N_CTX = 161024
N_CTX_TRAIN = 202752
ANSWER = "готово"

# Шаблон чата настоящего GLM: расширение ищет в нём слово enable_thinking и по
# нему объявляет поддержку размышлений.
CHAT_TEMPLATE = (
    "{%- if enable_thinking is not defined %}{%- set enable_thinking = true %}"
    "{%- endif %}{%- for message in messages %}<|im_start|>{{ message.role }}\n"
    "{{ message.content }}<|im_end|>\n{%- endfor %}"
)


class Handler(BaseHTTPRequestHandler):
    shape = "ok"
    dump = None

    def log_message(self, *_):  # тишина: вывод проверки и так читают глазами
        pass

    def _json(self, obj, code=200):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?")[0].rstrip("/")
        if path.endswith("/models") or path == "":
            entry = {"id": MODEL, "object": "model", "owned_by": "llamacpp"}
            if self.shape != "no-meta":
                entry["meta"] = {"n_ctx": N_CTX, "n_ctx_train": N_CTX_TRAIN}
            self._json({"object": "list", "data": [entry]})
        elif path.endswith("/props"):
            self._json({
                "model_path": "/models/" + MODEL + ".gguf",
                "total_slots": 3,
                "chat_template": CHAT_TEMPLATE,
                "default_generation_settings": {"n_ctx": N_CTX},
            })
        else:
            self._json({"error": "not found"}, 404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        if self.dump:
            with open(self.dump, "wb") as f:
                f.write(raw)
        try:
            req = json.loads(raw or b"{}")
        except ValueError:
            req = {}
        if req.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            for delta in ({"role": "assistant", "content": ""}, {"content": ANSWER}, None):
                chunk = {
                    "id": "stub",
                    "object": "chat.completion.chunk",
                    "model": MODEL,
                    "choices": [{
                        "index": 0,
                        "delta": delta or {},
                        "finish_reason": None if delta else "stop",
                    }],
                }
                self.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n")
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            return
        self._json({
            "id": "stub",
            "object": "chat.completion",
            "model": MODEL,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": ANSWER},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        })


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=18099)
    p.add_argument("--shape", choices=["ok", "no-meta"], default="ok")
    p.add_argument("--dump", help="куда сложить тело запроса к /v1/chat/completions")
    a = p.parse_args()
    Handler.shape = a.shape
    Handler.dump = a.dump
    srv = HTTPServer(("127.0.0.1", a.port), Handler)
    print(f"заглушка на {a.port}, форма {a.shape}", file=sys.stderr, flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
