#!/usr/bin/env python3
"""Exercise the patched runtime process against synthetic homes and loopback SSE.
No real credentials are loaded and no model service is contacted.
"""
import argparse
import base64
from datetime import datetime, timezone
import http.server
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time
import unittest

MODEL = {
    'slug': 'fixture-writing', 'display_name': 'Fixture Writing', 'description': None,
    'supported_reasoning_levels': [{'effort': 'low', 'description': 'Fixture'}],
    'shell_type': 'unified_exec', 'visibility': 'list', 'supported_in_api': True,
    'priority': 1, 'upgrade': None, 'base_instructions': 'MODEL_POISON', 'model_messages': None,
    'default_reasoning_summary': 'auto', 'support_verbosity': False, 'default_verbosity': None,
    'apply_patch_tool_type': None, 'truncation_policy': {'mode': 'bytes', 'limit': 10000},
    'effective_context_window_percent': 95, 'experimental_supported_tools': ['shell', 'web_search'],
    'input_modalities': ['text'],
}

def token(payload):
    enc = lambda v: base64.urlsafe_b64encode(json.dumps(v).encode()).decode().rstrip('=')
    return enc({'alg': 'none'}) + '.' + enc(payload) + '.fixture'

def synthetic_auth():
    return {'auth_mode': 'chatgpt', 'tokens': {
        'id_token': token({'email': 'fixture@example.invalid', 'exp': int(time.time()) + 86400,
                           'https://api.openai.com/auth': {'chatgpt_account_id': 'fixture-account', 'chatgpt_plan_type': 'plus'}}),
        'access_token': token({'exp': int(time.time()) + 86400}),
        'refresh_token': 'fixture-refresh', 'account_id': 'fixture-account'},
        'last_refresh': datetime.now(timezone.utc).isoformat()}

class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True
    def __init__(self):
        super().__init__(('127.0.0.1', 0), Handler)
        self.requests = queue.Queue()
        self.mode = 'success'
        self.started = threading.Event()

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_GET(self):
        assert self.path.startswith('/models?client_version=0.153.4'), self.path
        body = json.dumps({'models': [MODEL]}).encode()
        self.send_response(200); self.send_header('Content-Type', 'application/json'); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_POST(self):
        assert self.path == '/responses', self.path
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        self.server.requests.put((body, dict(self.headers)))
        self.server.started.set()
        mode = self.server.mode
        if mode == 'ambiguous-gateway':
            self.send_response(502); self.send_header('Content-Length', '0'); self.end_headers(); return
        self.send_response(200); self.send_header('Content-Type', 'text/event-stream'); self.end_headers()
        if mode == 'delay':
            time.sleep(3)
            return
        item = {'type': 'message', 'id': 'msg_fixture', 'role': 'assistant', 'content': [{'type': 'output_text', 'text': 'Polished café.'}]}
        if mode == 'tool': item = {'type':'function_call', 'id':'tool_fixture', 'call_id':'fixture_call','name':'shell','arguments':'{}'}
        events = [
            {'type': 'response.created', 'response': {'id': 'response_fixture'}},
            {'type': 'response.output_text.delta', 'delta': 'SHOULD_NOT_DUPLICATE'},
            {'type': 'response.output_item.done', 'item': item},
        ]
        if mode != 'truncated':
            events.append({'type': 'response.completed', 'response': {'id': 'response_fixture', 'status': 'completed', 'output': [], 'usage': {'input_tokens': 8, 'output_tokens': 3, 'total_tokens': 11, 'input_tokens_details': {'cached_tokens': 2}}}})
        try:
            for event in events:
                frame = ('data: ' + json.dumps(event, ensure_ascii=False) + '\n\n').encode()
                # Exercise framing across arbitrary boundaries, including UTF-8.
                for i in range(0, len(frame), 7): self.wfile.write(frame[i:i+7]); self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError): pass

class Client:
    def __init__(self, binary, home, policy, endpoint, project):
        env = dict(os.environ, CODEX_HOME=str(home), HOME=str(home.parent), OPENAI_BASE_URL='http://127.0.0.1:1/poison', CODEX_ACCESS_TOKEN='ignore-fixture-poison')
        self.p = subprocess.Popen([str(binary), 'app-server', '--listen', 'stdio://', '--selara-writing-mode', '--test-endpoint', endpoint, '--test-policy-dir', str(policy)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, cwd=project)
        self.q = queue.Queue(); self.next_id = 1; self.pending = []
        def reader():
            for line in self.p.stdout:
                try: self.q.put(json.loads(line))
                except Exception as e: self.q.put(e)
            self.q.put(None)
        threading.Thread(target=reader, daemon=True).start()
    def send(self, method, params):
        ident = self.next_id; self.next_id += 1
        self.p.stdin.write(json.dumps({'jsonrpc':'2.0','id':ident,'method':method,'params':params})+'\n'); self.p.stdin.flush()
        return ident
    def read(self):
        result = self.q.get(timeout=15)
        if result is None: raise AssertionError('runtime exited: ' + self.p.stderr.read())
        if isinstance(result, Exception): raise result
        return result
    def rpc(self, method, params):
        ident = self.send(method, params)
        while True:
            response = self.read()
            if response.get('id') == ident: return response
            self.pending.append(response)
    def ok(self, method, params):
        result = self.rpc(method, params)
        assert 'result' in result, result
        return result['result']
    def close(self):
        self.p.stdin.close()
        try: self.p.wait(timeout=5)
        except subprocess.TimeoutExpired: self.p.kill(); self.p.wait()
        self.p.stdout.close(); self.p.stderr.close()

class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='selara-runtime-fixture-')
        self.root = Path(self.tmp.name); self.home=self.root/'codex'; self.home.mkdir()
        self.policy=self.root/'policy'; self.policy.mkdir()
        self.project=self.root/'project'; self.project.mkdir()
        self.marker=self.root/'SHOULD_NEVER_EXIST'
        fixture=self.root/'should-never-run.sh'; fixture.write_text('#!/bin/sh\ntouch "'+str(self.marker)+'"\n'); fixture.chmod(0o700)
        (self.home/'auth.json').write_text(json.dumps(synthetic_auth()))
        (self.home/'AGENTS.md').write_text('USER_AGENT_POISON')
        (self.project/'AGENTS.md').write_text('PROJECT_AGENT_POISON')
        (self.home/'config.toml').write_text(f'''cli_auth_credentials_store = "file"
model_provider = "poison"
developer_instructions = "USER_CONFIG_POISON"
notify = ["{fixture}"]
[model_providers.poison]
name = "Poison"
base_url = "http://127.0.0.1:1"
[mcp_servers.poison]
command = "{fixture}"
[features]
web_search_request = true
''')
        (self.home/'hooks.json').write_text(json.dumps({'hooks': {'SessionStart': [{'hooks': [{'type':'command','command':str(fixture)}]}]}}))
        (self.home/'skills'/'poison').mkdir(parents=True)
        (self.home/'skills'/'poison'/'SKILL.md').write_text('---\nname: poison\ndescription: always run\n---\nSKILL_POISON')
        (self.project/'.codex').mkdir(); (self.project/'.codex'/'config.toml').write_text('developer_instructions = "PROJECT_CONFIG_POISON"\n')
        self.server=Server(); threading.Thread(target=self.server.serve_forever,daemon=True).start()
        self.client=Client(BINARY,self.home,self.policy,f'http://127.0.0.1:{self.server.server_port}',self.project)
        self.assertEqual(self.client.ok('initialize', {'selaraWritingMode':1,'clientInfo':{'name':'fixture','version':'1'}})['selaraWritingMode'],1)
    def tearDown(self):
        self.client.close(); self.server.shutdown(); self.server.server_close()
        self.assertFalse(self.marker.exists(), 'fixture command executed')
        self.tmp.cleanup()
    def start(self):
        thread=self.client.ok('thread/start', {'ephemeral':True,'model':'fixture-writing','baseInstructions':'Only the explicit writing instruction.'})['thread']['id']
        turn=self.client.ok('turn/start', {'threadId':thread,'input':[{'type':'text','text':'Polish only this sentence.'}]})['turn']['id']
        return thread,turn
    def finish(self, thread, turn):
        events=[]
        while True:
            e=self.client.read(); events.append(e)
            if e.get('method')=='turn/completed':
                self.assertEqual(e['params']['threadId'],thread); self.assertEqual(e['params']['turn']['id'],turn)
                return events
    def test_only_explicit_text_enters_upstream_request(self):
        account=self.client.ok('account/read', {})['account']; self.assertEqual(account['type'],'chatgpt')
        models=self.client.ok('model/list', {})['data']; self.assertEqual([m['id'] for m in models],['fixture-writing'])
        thread,turn=self.start(); events=self.finish(thread,turn)
        self.assertEqual(events[-1]['params']['turn']['status'],'completed',events)
        messages=[e['params']['item']['text'] for e in events if e.get('method')=='item/completed']
        self.assertEqual(messages,['Polished café.'])
        usage=[e['params']['tokenUsage']['last'] for e in events if e.get('method')=='thread/tokenUsage/updated']
        self.assertEqual(usage[0]['totalTokens'],11)
        request,headers=self.server.requests.get(timeout=2)
        self.assertEqual(request, {'model':'fixture-writing','instructions':'Only the explicit writing instruction.','input':[{'type':'message','role':'user','content':[{'type':'input_text','text':'Polish only this sentence.'}]}], 'tools':[],'tool_choice':'none','parallel_tool_calls':False,'store':False,'stream':True})
        self.assertNotIn('POISON',json.dumps(request)); self.assertTrue(any(k.lower()=='authorization' for k in headers))
        self.assertFalse((self.home/'sessions').exists())
        self.client.ok('thread/unsubscribe',{'threadId':thread})
    def test_execution_and_mutation_surfaces_rejected(self):
        for method in ['command/exec','config/value/write','thread/resume','fs/read','mcpServer/oauth/login','skills/list','plugin/install']:
            self.assertIn('error',self.client.rpc(method,{}),method)
        for extra in [{'cwd':str(self.project)},{'tools':[]},{'config':{}},{'developerInstructions':'poison'},{'environments':[]}]:
            p={'ephemeral':True,'model':'fixture-writing','baseInstructions':'explicit'};p.update(extra)
            self.assertIn('error',self.client.rpc('thread/start',p))
        self.assertIn('error',self.client.rpc('thread/start',{'ephemeral':False,'model':'fixture-writing','baseInstructions':'explicit'}))
        thread=self.client.ok('thread/start',{'ephemeral':True,'model':'fixture-writing','baseInstructions':'explicit'})['thread']['id']
        self.assertIn('error',self.client.rpc('turn/start',{'threadId':thread,'input':[{'type':'localImage','path':'/tmp/fixture.png'}]}))
    def test_truncated_stream_never_reports_success_or_partial_text(self):
        self.server.mode='truncated';thread,turn=self.start(); events=self.finish(thread,turn)
        self.assertTrue(self.server.started.is_set(), events)
        self.assertEqual(events[-1]['params']['turn']['status'],'failed')
        self.assertFalse(any(e.get('method') in ['item/completed','thread/tokenUsage/updated'] for e in events))
    def test_tool_output_never_reports_success(self):
        self.server.mode='tool';thread,turn=self.start();events=self.finish(thread,turn)
        self.assertTrue(self.server.started.is_set(), events)
        self.assertEqual(events[-1]['params']['turn']['status'],'failed')
    def test_ambiguous_generation_failure_is_not_replayed(self):
        self.server.mode='ambiguous-gateway';thread,turn=self.start();events=self.finish(thread,turn)
        self.assertEqual(events[-1]['params']['turn']['status'],'failed')
        self.assertEqual(self.server.requests.qsize(), 1, 'non-idempotent generation was replayed')
    def test_interrupt_is_responsive_during_network_request(self):
        self.server.mode='delay';thread,turn=self.start();self.assertTrue(self.server.started.wait(10))
        self.client.ok('turn/interrupt',{'threadId':thread,'turnId':turn})
        events=self.client.pending
        self.assertTrue(any(e.get('method')=='turn/completed' and e['params']['turn']['status']=='interrupted' for e in events),events)
    def test_managed_login_policy_is_preserved(self):
        (self.policy/'requirements.toml').write_text('allowed_login_methods = ["api"]\n')
        self.assertIsNone(self.client.ok('account/read',{})['account'])
        self.assertIn('error',self.client.rpc('account/login/start',{'type':'chatgpt'}))
        self.assertIn('error',self.client.rpc('model/list',{}))
    def test_logout_survives_incompatible_managed_transport(self):
        (self.policy/'requirements.toml').write_text('chatgpt_base_url = "https://managed.example.invalid/backend-api/"\n')
        self.client.ok('account/logout', {})
        self.assertFalse((self.home/'auth.json').exists())
    def test_logout_does_not_depend_on_readable_execution_configuration(self):
        (self.home/'config.toml').write_text('invalid = [')
        self.client.ok('account/logout', {})
        self.assertFalse((self.home/'auth.json').exists())
    def test_logout_recovers_interrupted_generation_write(self):
        (self.home/'.auth.generation').write_text('')
        self.client.ok('account/logout', {})
        self.assertFalse((self.home/'auth.json').exists())
        self.assertEqual(len((self.home/'.auth.generation').read_text()), 32)
    def test_api_key_only_account_is_visible_without_model_access(self):
        (self.home/'auth.json').write_text(json.dumps({'auth_mode':'apikey','OPENAI_API_KEY':'fixture-api-key'}))
        account=self.client.ok('account/read', {})['account']
        self.assertEqual(account['type'], 'apiKey')
        self.assertIn('error',self.client.rpc('model/list', {}))
    def test_managed_workspace_policy_is_preserved(self):
        (self.policy/'requirements.toml').write_text('allowed_chatgpt_workspaces = ["different-workspace"]\n')
        self.assertIsNone(self.client.ok('account/read',{})['account'])
        self.assertIn('error',self.client.rpc('model/list',{}))

if __name__=='__main__':
    parser=argparse.ArgumentParser(); parser.add_argument('--binary',type=Path,required=True)
    args,rest=parser.parse_known_args();BINARY=args.binary.resolve()
    unittest.main(argv=[__file__]+rest)
