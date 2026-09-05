"""Local-only browser viewer backed by the native observation stream."""

from __future__ import annotations

import json
import threading
from collections import deque
from collections.abc import Mapping
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .client import AgentError, NativeAgent
from .frames import FrameError, decode_preview


PAGE = """<!doctype html><title>MiSTer MagiK 2 watch</title><style>body{background:#101827;color:#dbeafe;font:14px system-ui;margin:20px}canvas{image-rendering:pixelated;border:1px solid #334155;max-width:100%}pre{background:#182235;padding:12px;max-height:240px;overflow:auto}</style><h1>MiSTer MagiK 2</h1><p id=state>Connecting…</p><canvas id=frame></canvas><pre id=logs></pre><script>
const c=document.querySelector('#frame'),x=c.getContext('2d'),s=document.querySelector('#state'),l=document.querySelector('#logs');
async function tick(){try{let q=await fetch('/state').then(r=>r.json());s.textContent=JSON.stringify(q.metrics||q.error||{},null,2);l.textContent=(q.logs||[]).join('\\n');if(q.frame){let b=await fetch('/frame').then(r=>r.arrayBuffer()),d=new DataView(b),w=d.getUint32(32,true),h=d.getUint32(36,true),p=new Uint8Array(b,72);c.width=w;c.height=h;let im=x.createImageData(w,h);for(let i=0,j=0;i<p.length;i+=2,j+=4){let v=p[i]|p[i+1]<<8;im.data[j]=(v>>11)*255/31;im.data[j+1]=((v>>5)&63)*255/63;im.data[j+2]=(v&31)*255/31;im.data[j+3]=255}x.putImageData(im,0,0)}}catch(e){s.textContent=e}setTimeout(tick,300)}tick();
</script>"""


class WatchState:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.metrics: Mapping[str, object] | None = None
        self.logs: deque[str] = deque(maxlen=100)
        self.frame: bytes | None = None
        self.error: str | None = None

    def snapshot(self) -> dict[str, object]:
        with self._lock:
            return {"metrics": self.metrics, "logs": list(self.logs), "frame": self.frame is not None, "error": self.error}

    def consume(self, agent: NativeAgent) -> None:
        try:
            with agent.open_watch() as connection:
                while True:
                    event, body = agent.read_watch_event(connection)
                    with self._lock:
                        if event.operation == "watch-metrics":
                            value = event.fields.get("metrics")
                            if isinstance(value, Mapping):
                                self.metrics = value
                        elif event.operation == "watch-log":
                            line = event.fields.get("line")
                            if isinstance(line, str):
                                self.logs.append(line)
                        elif event.operation == "watch-frame":
                            decode_preview(body)
                            self.frame = body
        except (AgentError, FrameError, OSError, RuntimeError) as error:
            with self._lock:
                self.error = str(error)


def serve(agent: NativeAgent) -> tuple[ThreadingHTTPServer, str]:
    state = WatchState()
    threading.Thread(target=state.consume, args=(agent,), daemon=True).start()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/":
                self._send(HTTPStatus.OK, "text/html; charset=utf-8", PAGE.encode())
            elif self.path == "/state":
                self._send(HTTPStatus.OK, "application/json", json.dumps(state.snapshot()).encode())
            elif self.path == "/frame":
                with state._lock:
                    frame = state.frame
                if frame is None:
                    self._send(HTTPStatus.NOT_FOUND, "text/plain", b"no frame yet")
                else:
                    self._send(HTTPStatus.OK, "application/octet-stream", frame)
            else:
                self._send(HTTPStatus.NOT_FOUND, "text/plain", b"not found")

        def log_message(self, _format: str, *_args: object) -> None:
            return

        def _send(self, status: HTTPStatus, content_type: str, body: bytes) -> None:
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    return server, f"http://127.0.0.1:{server.server_port}/"
