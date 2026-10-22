import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
const routes={
  '/':['web/index.html','text/html'],
  '/assets/app.js':['web/dist/app.js','text/javascript'],
  '/assets/style.css':['web/style.css','text/css'],
};
createServer(async(req,res)=>{
  const path=new URL(req.url,'http://127.0.0.1').pathname;
  const module = path.match(/^\/mcp-app-preview\/(mail|calendar|kchat|kdrive)$/)?.[1];
  if(module){
    const [css,bridge,script]=await Promise.all(['src/mcp_apps/style.css','src/mcp_apps/bridge.js',`src/modules/${module}/app.js`].map(file=>readFile(file,'utf8')));
    res.writeHead(200,{'content-type':'text/html','content-security-policy':"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'none'; connect-src 'none'; frame-ancestors 'self'"}).end(`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${module}</title><style>${css}</style><body><main id="app" aria-live="polite"><p>Loading…</p></main><script>${bridge}\n${script}</script></body></html>`);return;
  }
  const route=routes[path];
  if(!route){res.writeHead(path==='/api/me'?401:404,{'content-type':'application/json'}).end('{}');return}
  try{res.writeHead(200,{'content-type':route[1]}).end(await readFile(route[0]))}catch{res.writeHead(500).end()}
}).listen(3101,'127.0.0.1');
