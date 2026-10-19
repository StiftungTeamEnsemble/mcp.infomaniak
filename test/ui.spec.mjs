import {test,expect} from '@playwright/test';
const account={user:{id:1,name:'Alice',email:'alice@team-ensemble.ch',status:'active',admin:false},csrf:'test-csrf',endpoint:'https://mcp.example.org/mcp',modules:[{id:'mail',name:'Mail',enabled:true,level:1},{id:'calendar',name:'Calendar',enabled:false,level:0},{id:'kdrive',name:'kDrive',enabled:true,level:2}],connections:[],clients:[]};
test('login uses browser sign-in and adapts to a phone',async({page})=>{
  await page.goto('/');await expect(page.getByRole('link',{name:'Sign in with Infomaniak'})).toHaveAttribute('href','/auth/login');
  await page.setViewportSize({width:375,height:812});await expect(page.getByRole('heading',{name:'A connection to your work.'})).toBeVisible();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  await page.screenshot({path:'test-results/login-mobile.png',fullPage:true});
});
test('pending users see approval without service actions',async({page})=>{
  await page.route('**/api/me',r=>r.fulfill({json:{...account,user:{...account.user,status:'pending'}}}));
  await page.goto('/');await expect(page.getByRole('heading',{name:'Approval pending'})).toBeVisible();await expect(page.getByRole('button',{name:'Connect ↗',exact:true})).toHaveCount(0);
});
test('service availability, consent return and csrf are reflected in connection actions',async({page})=>{
  await page.route('**/api/me',r=>r.fulfill({json:account}));
  await page.route('**/api/authorize?flow=*',r=>r.fulfill({json:{client:'Test assistant',redirect_uri:'https://client.example/callback',scopes:[{scope:'mail:read',available:false}]}}));
  let request;
  await page.route('**/api/connect/mail?authorize=flow-id',r=>{request=r.request();return r.fulfill({json:{error_description:'Consent needs reconnection'},status:403})});
  await page.goto('/?authorize=flow-id');
  const mail=page.locator('section').filter({has:page.getByRole('heading',{name:'Mail',exact:true})});
  await expect(mail.getByRole('button',{name:'Connect ↗',exact:true})).toBeEnabled();
  const calendar=page.locator('section').filter({has:page.getByRole('heading',{name:'Calendar',exact:true})});await expect(calendar.getByRole('button',{name:'Connect ↗',exact:true})).toBeDisabled();
  await mail.getByRole('button',{name:'Connect ↗',exact:true}).click();await expect(page.getByRole('alert')).toContainText('Consent needs reconnection');expect(request.headers()['x-csrf-token']).toBe('test-csrf');
  await expect(page.getByRole('button',{name:'Allow selected access'})).toBeDisabled();
  await page.screenshot({path:'test-results/connections.png',fullPage:true});
});
test('administration shows configuration without rendering supplied html',async({page})=>{
  await page.route('**/api/me',r=>r.fulfill({json:{...account,user:{...account.user,admin:true}}}));
  await page.route('**/api/admin',r=>r.fulfill({json:{users:[{...account.user,name:'<img src=x onerror=alert(1)>'}],modules:[{id:'mail',enabled:false,verification:'',default_level:0}],groups:[],memberships:[],policies:[],connections:[],logs:[],manual_approval:false}}));
  await page.goto('/');await page.getByRole('button',{name:'Administration'}).click();await expect(page.getByRole('heading',{name:'People'})).toBeVisible();await expect(page.locator('img')).toHaveCount(0);await expect(page.getByText('<img src=x onerror=alert(1)>')).toBeVisible();
  await page.screenshot({path:'test-results/admin.png',fullPage:true});
});
test('MCP cards render text safely and only offer provider source links',async({page})=>{
  await page.goto('/');await page.evaluate(()=>{window.cardMessages=[];window.addEventListener('message',e=>{if(e.data?.jsonrpc!=='2.0')return;window.cardMessages.push(e.data);if(e.data.method==='ui/initialize')e.source.postMessage({jsonrpc:'2.0',id:e.data.id,result:{protocolVersion:'2026-01-26',hostCapabilities:{}}},'*')});const frame=document.createElement('iframe');frame.src='/card';frame.id='test-card';document.body.append(frame)});
  const frame=page.frameLocator('#test-card');await expect(frame.getByRole('heading',{name:'Ensemble'})).toBeVisible();
  await page.evaluate(()=>document.getElementById('test-card').contentWindow.postMessage({jsonrpc:'2.0',method:'ui/notifications/tool-result',params:{structuredContent:{data:[{subject:'<img onerror=alert(1)>',preview:'Private preview',url:'javascript:alert(1)'},{title:'Meeting',description:'Project review'}],context:{source_url:'https://ksuite.infomaniak.com/calendar'}}}},'*'));
  await expect.poll(()=>page.evaluate(()=>window.cardMessages.some(m=>m.method==='ui/notifications/initialized'&&!('id' in m)))).toBe(true);
  await expect(frame.getByText('<img onerror=alert(1)>')).toBeVisible();await expect(frame.locator('img')).toHaveCount(0);await expect(frame.getByRole('button',{name:'Open in Infomaniak'})).toHaveCount(1);
  await frame.getByRole('button',{name:'Open in Infomaniak'}).click();
  await expect.poll(()=>page.evaluate(()=>window.cardMessages.some(m=>m.method==='ui/open-link'&&m.params.url==='https://ksuite.infomaniak.com/calendar'))).toBe(true);
  await page.screenshot({path:'test-results/cards.png',fullPage:true});
});
