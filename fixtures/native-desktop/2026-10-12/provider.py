import http.server
import json
from pathlib import Path
import sys
import threading
import time

root = Path(sys.argv[1])
lock = threading.Lock()

def record(event):
    with lock, (root / 'provider-events.jsonl').open('a') as f:
        f.write(json.dumps(event) + '\n')

def text(message):
    body = message.get('content', '')
    return body if isinstance(body, str) else ''.join(p.get('text', '') for p in body or [])

class Provider(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_): pass
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        messages = request['messages']
        prompt = next((text(m) for m in reversed(messages) if m['role'] == 'user'), '')
        record({'event':'request', 'prompt':prompt, 'body':request})
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Connection', 'close')
        self.end_headers()
        self.close_connection = True
        def chunk(delta, finish=None):
            payload = {'id':'visual-check', 'object':'chat.completion.chunk', 'model':'coding',
                'choices':[{'index':0,'delta':delta,'finish_reason':finish}]}
            self.wfile.write(('data: '+json.dumps(payload)+'\n\n').encode()); self.wfile.flush()
        try:
            chunk({'role':'assistant','content':''})
            if 'Slow stream' in prompt:
                for i in range(1500):
                    chunk({'content':f'Stream marker {i:04d}. '})
                    time.sleep(0.2)
            elif messages[-1]['role'] == 'tool':
                chunk({'content':'Created greeting.rs with a real write tool. This response is from the local scripted qualification provider.'})
            elif 'Create greeting.rs' in prompt:
                chunk({'content':'Writing the requested Rust file with the real engine tool. '})
                chunk({'tool_calls':[{'index':0,'id':'visual-reused','type':'function',
                    'function':{'name':'write','arguments':json.dumps({'path':'greeting.rs','content':'fn main() { println!("native packaged engine"); }\n'})}}]})
                chunk({}, 'tool_calls')
                self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush(); return
            else:
                chunk({'content':'Local scripted qualification reply: '+prompt})
            chunk({}, 'stop')
            self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush()
            record({'event':'completed','prompt':prompt})
        except (BrokenPipeError, ConnectionResetError):
            record({'event':'disconnected','prompt':prompt})

server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Provider)
server.daemon_threads = True
(root/'profile/engine-config/models.json').write_text(json.dumps({'providers':{'visual-test':{
    'baseUrl':f'http://127.0.0.1:{server.server_port}/v1','api':'openai-completions','apiKey':'synthetic',
    'models':[{'id':'coding','name':'Local scripted qualification','input':['text'],'contextWindow':200000,'maxTokens':16384}]}}}))
(root/'provider-port').write_text(str(server.server_port))
print(f'local scripted provider port {server.server_port}',flush=True)
server.serve_forever()
