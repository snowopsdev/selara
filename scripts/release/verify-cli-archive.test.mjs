import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join, basename, resolve} from 'node:path';
import {verifyCliArchive} from './verify-cli-archive.mjs';

function fixture(t, missing) {
  const temp=mkdtempSync(join(tmpdir(),'selara-cli-fixture-'));
  t.after(()=>rmSync(temp,{recursive:true,force:true}));
  const root=fileURLToPath(new URL('../../',import.meta.url));
  const content=join(temp,'selara-0.4.1-macos-arm64'); mkdirSync(content);
  const lock=readFileSync(join(root,'vendor/codex-runtime/runtime.toml'),'utf8');
  const provenance=Object.fromEntries(['source_revision','patches_sha256','target','minimum_macos'].map(key=>[key,lock.match(new RegExp(`^${key} = "([^"]+)"`,'m'))[1]]));
  for(const name of ['selara','selara-codex','selara-codex.LICENSE','selara-codex.NOTICE']) if(name!==missing)writeFileSync(join(content,name),'fixture');
  writeFileSync(join(content,'selara-codex.provenance.json'),JSON.stringify(provenance));
  const archive=join(temp,'archive.tar.gz');
  const pack=()=>execFileSync('tar',['-czf',archive,'-C',temp,basename(content)], {env:{...process.env,COPYFILE_DISABLE:'1'}});
  pack();
  const calls=[];
  const run=(program,args)=>{
    calls.push(program);
    if(program==='python3')return execFileSync(program,args,{encoding:'utf8',stdio:'pipe'});
    if(program==='codesign') {
      const index=args.indexOf('-R');
      if(index!==-1) assert.match(args[index+1], /^=anchor apple generic and certificate leaf\[subject\.OU\] = "0123456789"/);
      return '';
    }
    if(basename(program)==='selara')return 'selara 0.4.1';
    if(basename(program)==='selara-codex')return 'selara-codex 0.153.4';
    throw new Error('Unexpected verification command');
  };
  return {temp,root,content,archive,pack,provenance,calls,run};
}

test('complete CLI archive validates version, runtime provenance, and both signatures',t=>{
 const f=fixture(t);verifyCliArchive(f.archive,f.root,'v0.4.1','0123456789',f.run);
 assert.equal(f.calls.filter(x=>x==='codesign').length,4);
});
test('incomplete or malformed recovered archive cannot reach checksum publication',t=>{
 const f=fixture(t,'selara-codex');
 assert.throws(()=>verifyCliArchive(f.archive,f.root,'v0.4.1','0123456789',f.run),/Missing CLI archive file/);
 assert.equal(f.calls.includes('codesign'),false);
 writeFileSync(f.archive,'malformed archive');
 assert.throws(()=>verifyCliArchive(f.archive,f.root,'v0.4.1','0123456789',f.run));
});
test('wrong version, provenance, or code signature rejects a recovered CLI',t=>{
 const f=fixture(t);
 assert.throws(()=>verifyCliArchive(f.archive,f.root,'v0.4.1','0123456789',(p,a)=>basename(p)==='selara'?'selara 0.4.0':f.run(p,a)),/version does not match/);
 assert.throws(()=>verifyCliArchive(f.archive,f.root,'v0.4.1','0123456789',(p,a)=>{if(p==='codesign')throw Error('invalid signature');return f.run(p,a);}),/invalid signature/);
 f.provenance.patches_sha256='wrong';writeFileSync(join(f.content,'selara-codex.provenance.json'),JSON.stringify(f.provenance));f.pack();
 assert.throws(()=>verifyCliArchive(f.archive,f.root,'v0.4.1','0123456789',f.run),/provenance mismatch/);
});
test('missing expected team rejects recovered CLI before extraction or execution',t=>{
 const f=fixture(t);
 for(const teamId of ['',undefined,'0123456789\n']) assert.throws(()=>verifyCliArchive(f.archive,f.root,'v0.4.1',teamId,f.run),/APPLE_TEAM_ID/);
 assert.deepEqual(f.calls,[]);
});
