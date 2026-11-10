// Local browser-only Help capture. Uses a disposable headless Chrome profile.
// No native SparkBox app, credentials, real nodes or user browser profile are opened.
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
const root = resolve(import.meta.dirname, '..');
const origin = process.env.SPARKBOX_DEMO_ORIGIN ?? 'http://localhost:1739';
if (!/^http:\/\/(127\.0\.0\.1|localhost):\d+$/.test(origin)) throw Error('Only a local Demo origin is allowed');
const chromePath = process.env.CHROME_PATH ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const profile = await mkdtemp(join(tmpdir(), 'sparkbox-public-chrome-'));
const child = spawn(chromePath, ['--headless=new', '--no-first-run', '--disable-background-networking', '--disable-component-update', '--disable-sync', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], {stdio:'ignore'});
const wait = ms => new Promise(r=>setTimeout(r,ms));
let ws;
try {
  let port;
  for (let i=0;i<100;i++) { try { port=(await readFile(join(profile,'DevToolsActivePort'),'utf8')).split('\n')[0]; break; } catch { await wait(100); } }
  if (!port) throw Error('Headless browser did not start');
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  ws = new WebSocket(targets.find(t=>t.type==='page').webSocketDebuggerUrl);
  await new Promise((yes,no)=>{ws.onopen=yes;ws.onerror=no;});
  let seq=0; const pending=new Map();
  ws.onmessage=e=>{const m=JSON.parse(e.data);if(!m.id)return;const p=pending.get(m.id);if(!p)return;pending.delete(m.id);clearTimeout(p.timer);m.error?p.no(Error(JSON.stringify(m.error))):p.yes(m.result);};
  const cdp=(method,params={})=>new Promise((yes,no)=>{const id=++seq;const timer=setTimeout(()=>{pending.delete(id);no(Error(`Timed out: ${method}`));},15000);pending.set(id,{yes,no,timer});ws.send(JSON.stringify({id,method,params}));});
  const evaluate=async expression=>{const r=await cdp('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result.value;};
  await cdp('Page.enable');
  await cdp('Emulation.setDeviceMetricsOverride',{width:1280,height:900,deviceScaleFactor:1,mobile:false});
  const manifest=JSON.parse(await readFile(join(root,'docs/help/manifest.json'),'utf8'));
  const shots=JSON.parse(await readFile(join(root,'docs/help/shots.json'),'utf8'));
  const params={node:{id:'cell1'},switch:{id:'crs804'},terminal:{node:'cell1'},inspect:{node:'cell1',id:'vllm-head',name:'vllm-head'},editor:{place:'cell1',path:'/home/sparkuser/notes.txt'},help:{id:'getting-started'},settings:{section:'general'}};
  const items=Object.entries(manifest.windowHelp).filter(([,id])=>manifest.articles.find(a=>a.id===id)?.type==='feature').map(([kind,id])=>({kind,id,params:params[kind]??{}}));
  items.push({kind:'settings',id:'settings-fence',params:{section:'fence'}});
  items.push({kind:'node',id:'memory-on-a-spark',params:{id:'cell1'}});
  const report=[];
  const forbidden=(process.env.SPARKBOX_CAPTURE_FORBIDDEN ?? '').split('|').filter(Boolean);
  for (const item of items) {
    const article=manifest.articles.find(a=>a.id===item.id);
    const shotId=`public-${item.id}`;
    const spec=item.kind+(Object.keys(item.params).length?':'+Object.entries(item.params).map(([k,v])=>`${k}=${encodeURIComponent(v)}`).join('&'):'');
    for (const theme of ['light','dark']) {
      const query=new URLSearchParams({reset:'1',still:'1',theme,open:spec});
      if(item.id==='settings-fence')query.set('fencePreview','1');
      await cdp('Page.navigate',{url:`${origin}/mock.html?${query}`});
      let ready=false;
      for(let i=0;i<100;i++){await wait(100);try{ready=await evaluate(`document.querySelector('.win[data-kind="${item.kind}"]') !== null`);}catch{}if(ready)break;}
      if(!ready){ console.error(await evaluate('({url:location.href,body:document.body.innerText,html:document.body.innerHTML.slice(0,3000)})')); throw Error(`Window did not open: ${item.id}`); }
      await wait(450);
      const clip=await evaluate(`(()=>{const r=document.querySelector('.win[data-kind="${item.kind}"]').getBoundingClientRect();return {x:Math.max(0,r.x),y:Math.max(0,r.y),width:Math.min(r.width,innerWidth-r.x),height:Math.min(r.height,innerHeight-r.y),scale:1};})()`);
      const png=await cdp('Page.captureScreenshot',{format:'jpeg',quality:85,clip});
      await writeFile(join(root,`docs/help/img/${shotId}${theme==='dark'?'-dark':''}.jpg`),Buffer.from(png.data,'base64'));
      const text=await evaluate('document.body.innerText');
      if(forbidden.some(value=>text.includes(value)))throw Error(`Private fixture text in ${shotId}`);
      report.push({id:shotId,theme,clip,forbiddenTextChecked:forbidden.length,ownerTextFound:false});
    }
    const caption=`${article.title}, using synthetic Demo data`;
    const file=join(root,'docs/help',article.file);
    let body=await readFile(file,'utf8');
    if(!body.includes(`img/${shotId}.jpg`))body+=`\n![${caption}](img/${shotId}.jpg)\n`;
    await writeFile(file,body);
    if(!shots.shots.some(s=>s.id===shotId))shots.shots.push({id:shotId,source:'demo',js:`window.dispatchEvent(new CustomEvent('sparkbox:open-window', {detail: ${JSON.stringify({kind:item.kind,params:item.params})}}));`,caption,capture:'Disposable headless browser; synthetic public Demo only; scripts/capture-public-help.mjs'});
    console.log(`Captured ${shotId} in Light and Dark`);
  }
  await writeFile(join(root,'docs/help/shots.json'),JSON.stringify(shots,null,2)+'\n');
  await writeFile(join(root,'../evidence/public-help-capture.json'),JSON.stringify(report,null,2)+'\n');
} finally {
  ws?.close(); child.kill('SIGTERM');
  await new Promise(r=>{if(child.exitCode!==null)r();else child.once('exit',r);});
  await rm(profile,{recursive:true,force:true});
}
