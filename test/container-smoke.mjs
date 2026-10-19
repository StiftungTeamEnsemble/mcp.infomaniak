// Run after docker build -t ensemble-mcp:local .; no real application credentials are used.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {randomBytes, createHash} from 'node:crypto';
import {setTimeout} from 'node:timers/promises';

const image = process.argv[2] || 'ensemble-mcp:local';
const name = `ensemble-mcp-smoke-${randomBytes(6).toString('hex')}`;
const origin = 'https://mcp.example.org';
const docker = (...args) => execFileSync('docker', args, {encoding:'utf8', timeout:30000}).trim();
let created = false;
let volume = false;
try {
  docker('volume', 'create', name);
  volume = true;
  docker('run', '--detach', '--name', name, '--read-only', '--cap-drop=ALL',
    '--security-opt=no-new-privileges:true', '--tmpfs', '/tmp:size=16m,noexec,nosuid',
    '--publish', '127.0.0.1::3000', '--volume', `${name}:/data`,
    '--env', `PUBLIC_URL=${origin}`, '--env', 'SQLITE_PATH=/data/mcp.sqlite',
    '--env', `CREDENTIAL_ENCRYPTION_KEY=${randomBytes(32).toString('base64')}`,
    '--env', 'INFOMANIAK_CLIENT_ID=smoke-client', '--env', 'INFOMANIAK_CLIENT_SECRET=smoke-secret',
    '--env', 'ALLOWED_EMAIL_DOMAINS=team-ensemble.ch', image);
  created = true;
  let address = `http://${docker('port', name, '3000/tcp')}`;
  const request = (path, options={}) => fetch(address+path, {redirect:'manual', signal:AbortSignal.timeout(3000), ...options});
  async function healthy() {
    for (let i=0; i<120; i++) {
      try { if ((await request('/health')).status === 200) return; } catch {}
      await setTimeout(250);
    }
    throw new Error('Container did not become healthy; check public OIDC discovery/network access');
  }
  await healthy();
  docker('exec', name, 'ensemble-mcp', 'healthcheck');
  assert.equal(docker('exec', name, 'id', '-u'), '10001');
  assert.equal(docker('exec', name, 'stat', '-c', '%a', '/data/mcp.sqlite'), '600');
  for (const path of ['/', '/assets/app.js', '/assets/style.css']) assert.equal((await request(path)).status, 200);
  const metadata = await (await request('/.well-known/oauth-protected-resource/mcp')).json();
  assert.equal(metadata.resource, `${origin}/mcp`);
  assert.deepEqual(metadata.authorization_servers, [origin]);
  const auth = await (await request('/.well-known/oauth-authorization-server')).json();
  assert.equal(auth.issuer, origin);
  assert.deepEqual(auth.code_challenge_methods_supported, ['S256']);
  const login = await request('/auth/login');
  assert.equal(login.status, 303);
  const upstream = new URL(login.headers.get('location'));
  assert.equal(upstream.origin, 'https://login.infomaniak.com');
  assert.equal(upstream.searchParams.get('redirect_uri'), `${origin}/auth/infomaniak/callback`);
  assert.equal(upstream.searchParams.get('code_challenge_method'), 'S256');
  assert.match(login.headers.get('set-cookie'), /Secure; HttpOnly; SameSite=Lax/);
  const unauthenticated = await request('/mcp', {method:'POST', headers:{'Content-Type':'application/json'}, body:'{}'});
  assert.equal(unauthenticated.status, 401);
  assert.match(unauthenticated.headers.get('www-authenticate'), /resource_metadata=/);
  assert.equal((await request('/mcp', {method:'POST', headers:{Origin:'https://evil.example'}})).status, 403);
  assert.equal((await request('/mcp')).status, 405);
  const registration = await request('/register', {method:'POST', headers:{'Content-Type':'application/json'},
    body:JSON.stringify({client_name:'Container smoke test', redirect_uris:['https://client.example/callback'], token_endpoint_auth_method:'none'})});
  assert.equal(registration.status, 201);
  const client = await registration.json();
  docker('stop', '--time', '10', name);
  assert.equal(docker('inspect', '--format', '{{.State.ExitCode}}', name), '0');
  docker('start', name);
  address = `http://${docker('port', name, '3000/tcp')}`;
  await healthy();
  const authorize = new URLSearchParams({client_id:client.client_id, redirect_uri:client.redirect_uris[0],
    response_type:'code', code_challenge_method:'S256', code_challenge:createHash('sha256').update('v'.repeat(64)).digest('base64url'),
    resource:`${origin}/mcp`, scope:'kdrive:read'});
  assert.equal((await request(`/authorize?${authorize}`)).status, 303, 'Client registration must survive restart');
  docker('stop', '--time', '10', name);
  assert.equal(docker('inspect', '--format', '{{.State.ExitCode}}', name), '0');
  console.log('Container smoke passed: OIDC/PKCE, discovery, registration persistence, assets, access rejection, permissions and clean shutdown. Live consent and provider operations were not tested.');
} finally {
  if (created) docker('rm', '--force', name);
  if (volume) docker('volume', 'rm', name);
}
