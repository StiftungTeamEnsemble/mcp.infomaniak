import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile,readdir,access} from 'node:fs/promises';
import {dirname,resolve} from 'node:path';
test('local documentation links resolve',async()=>{
  async function walk(dir){const entries=await readdir(dir,{withFileTypes:true});return(await Promise.all(entries.map(e=>e.isDirectory()?walk(`${dir}/${e.name}`):[`${dir}/${e.name}`]))).flat()}
  const files=['AGENTS.md','README.md',...(await walk('spec')).filter(f=>f.endsWith('.md'))];
  for(const file of files){const text=await readFile(file,'utf8');for(const match of text.matchAll(/\]\(([^)]+)\)/g)){const ref=match[1];if(/^(https?:|#)/.test(ref))continue;const target=resolve(dirname(file),decodeURIComponent(ref.split('#')[0]));await assert.doesNotReject(access(target),`${file}: ${ref}`);}}
});
