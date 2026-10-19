import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
const routes={
  '/':['web/index.html','text/html'],
  '/assets/app.js':['web/dist/app.js','text/javascript'],
  '/assets/style.css':['web/style.css','text/css'],
  '/card':['web/card.html','text/html'],
};
createServer(async(req,res)=>{
  const path=new URL(req.url,'http://127.0.0.1').pathname;
  const route=routes[path];
  if(!route){res.writeHead(path==='/api/me'?401:404,{'content-type':'application/json'}).end('{}');return}
  try{res.writeHead(200,{'content-type':route[1]}).end(await readFile(route[0]))}catch{res.writeHead(500).end()}
}).listen(3101,'127.0.0.1');
