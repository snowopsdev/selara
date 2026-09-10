#!/usr/bin/env python3
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve()
assert subprocess.check_output([binary, '--version'], text=True).strip() == 'selara-codex 0.153.4'
with tempfile.TemporaryDirectory(prefix='selara-production-smoke-') as temp:
    home = Path(temp)
    env = dict(os.environ, CODEX_HOME=temp, HOME=temp)
    args = [binary, 'app-server', '--listen', 'stdio://', '--selara-writing-mode']
    requests = [
        {'id': 1, 'method': 'initialize', 'params': {'selaraWritingMode': 1}},
        {'id': 2, 'method': 'command/exec', 'params': {'command': 'touch SHOULD_NEVER_EXIST'}},
        {'id': 3, 'method': 'thread/start', 'params': {'ephemeral': False, 'model': 'fixture', 'baseInstructions': ''}},
        {'id': 4, 'method': 'thread/start', 'params': {'ephemeral': True, 'model': 'fixture', 'baseInstructions': 'Explicit writing instruction'}},
    ]
    p = subprocess.run(args, input=''.join(json.dumps(r)+'\n' for r in requests), text=True, capture_output=True, cwd=home, env=env, timeout=15)
    assert p.returncode == 0, p.stderr
    frames = [json.loads(line) for line in p.stdout.splitlines()]
    replies = {f['id']: f for f in frames}
    assert replies[1]['result']['selaraWritingMode'] == 1
    assert 'error' in replies[2] and 'error' in replies[3]
    assert replies[4]['result']['thread']['ephemeral'] is True
    assert not (home/'SHOULD_NEVER_EXIST').exists()
    assert not (home/'sessions').exists()
    # The shipped build must not accept test endpoint injection.
    p = subprocess.run(args+['--test-endpoint','http://127.0.0.1:1','--test-policy-dir',temp], stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=15)
    assert p.returncode != 0 and b'unsupported runtime arguments' in p.stderr
print('Production runtime version, capability, method whitelist, ephemeral enforcement, and disabled test overrides passed')
