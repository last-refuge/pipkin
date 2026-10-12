// Execute Pi's pinned undici CONNECT behavior over both HTTP and HTTPS proxy hops.
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { createHash, createPrivateKey } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import http from 'node:http';
import https from 'node:https';
import net from 'node:net';
import { join, dirname } from 'node:path';
const [reference,sdkRoot,output]=process.argv.slice(2);
if(!reference||!sdkRoot||!output)throw new Error('Usage: node scripts/capture-http-tunnel-reference.mjs PI_CHECKOUT PI_INSTALLED_ROOT OUTPUT_JSON');
const pin='d2a311097cbcf669e699479587332ae3988a49d0';
if(execFileSync('git',['-C',reference,'rev-parse','HEAD'],{encoding:'utf8'}).trim()!==pin)throw new Error('Unexpected Pi revision');
execFileSync('git',['-C',reference,'diff','--exit-code','HEAD','--','package-lock.json','packages/coding-agent/src/core/http-dispatcher.ts']);
const require=createRequire(join(sdkRoot,'package.json'));
const {fetch,EnvHttpProxyAgent}=require('undici');
const locked=JSON.parse(await readFile(join(reference,'package-lock.json'))).packages['node_modules/undici'];
const pkg=JSON.parse(await readFile(join(sdkRoot,'node_modules/undici/package.json')));
if(pkg.version!==locked.version)throw new Error('Unexpected undici version');
const pem=async file=>'-----BEGIN CERTIFICATE-----\n'+(await readFile(file)).toString('base64')+'\n-----END CERTIFICATE-----\n';
const ca=await pem('fixtures/native-tls/ca.der'),cert=await pem('fixtures/native-tls/localhost.der');
const key=createPrivateKey({key:await readFile('fixtures/native-tls/localhost-key.der'),format:'der',type:'pkcs8'}).export({format:'pem',type:'pkcs8'});
const listen=server=>new Promise(r=>server.listen(0,'127.0.0.1',r));
const cases=[];
for(const proxyTls of [false,true])for(const targetTls of [false,true])for(const explicitPort of [false,true]){
  const originRequests=[],connects=[];
  const listener=(req,res)=>{let body='';req.on('data',x=>body+=x);req.on('end',()=>{originRequests.push({method:req.method,path:req.url,authorization:req.headers.authorization??null,proxyAuthorization:req.headers['proxy-authorization']??null,body});res.setHeader('content-type','application/json');res.end('{"ok":true}');});};
  const origin=targetTls?https.createServer({cert,key},listener):http.createServer(listener);await listen(origin);
  const proxy=proxyTls?https.createServer({cert,key}):http.createServer();await listen(proxy);
  const sockets=new Set();
  proxy.on('connect',(req,socket,head)=>{
    connects.push({method:req.method,authority:req.url,authorization:req.headers.authorization??null,proxyAuthorization:req.headers['proxy-authorization']??null});
    const upstream=net.connect(origin.address().port,'127.0.0.1',()=>{socket.write('HTTP/1.1 200 Connection Established\r\n\r\n');if(head.length)upstream.write(head);socket.pipe(upstream);upstream.pipe(socket);});
    sockets.add(socket);sockets.add(upstream);socket.on('error',()=>upstream.destroy());upstream.on('error',()=>socket.destroy());
  });
  const proxyUrl=`${proxyTls?'https':'http'}://user:p%40ss@localhost:${proxy.address().port}`;
  const agent=new EnvHttpProxyAgent({httpProxy:proxyUrl,httpsProxy:proxyUrl,noProxy:'',allowH2:false,proxyTunnel:true,requestTls:{ca},proxyTls:{ca}});
  const url=`${targetTls?'https://localhost':'http://provider.test'}${explicitPort?(targetTls?':8443':':8080'):''}/v1/chat`;
  try {const response=await fetch(url,{dispatcher:agent,method:'POST',headers:{authorization:'Bearer origin-secret'},body:'{}'});cases.push({name:`${targetTls?'https':'http'}-via-${proxyTls?'https':'http'}-${explicitPort?'port':'default'}`,url,proxyTls,targetTls,status:response.status,body:await response.json(),connects,originRequests});}
  finally{await agent.close();for(const s of sockets)s.destroy();proxy.close();origin.close();proxy.closeAllConnections();origin.closeAllConnections();}
}
const hash=async file=>createHash('sha256').update(await readFile(file)).digest('hex');
await mkdir(dirname(output),{recursive:true});
await writeFile(output,JSON.stringify({reference:pin,node:process.version,undici:{version:pkg.version,integrity:locked.integrity,entrySha256:await hash(join(sdkRoot,'node_modules/undici/index.js'))},dispatcherSha256:await hash(join(reference,'packages/coding-agent/src/core/http-dispatcher.ts')),scope:'Eight real local origin/proxy combinations using pinned EnvHttpProxyAgent with proxyTunnel:true; synthetic credentials',cases},null,2)+'\n');
