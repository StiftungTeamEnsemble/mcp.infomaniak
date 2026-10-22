import {test,expect} from '@playwright/test';
const account={user:{id:1,name:'Alice',email:'alice@team-ensemble.ch',status:'active',admin:false},csrf:'test-csrf',endpoint:'https://mcp.example.org/mcp',modules:[{id:'mail',name:'Mail',provider:'infomaniak',enabled:true,level:1,scopes:['workspace:mail','user_info']},{id:'calendar',name:'Calendar',provider:'infomaniak',enabled:false,level:0,scopes:['workspace:calendar','user_info']},{id:'kdrive',name:'kDrive',provider:'infomaniak',enabled:true,level:2,scopes:['drive','user_info']}],connections:[],clients:[]};
test('login uses browser sign-in and adapts to a phone',async({page})=>{
  await page.goto('/');await expect(page.getByRole('link',{name:'Sign in with Infomaniak'})).toHaveAttribute('href','/auth/login');
  await page.setViewportSize({width:375,height:812});await expect(page.getByRole('heading',{name:'A connection to your work.'})).toBeVisible();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  await page.screenshot({path:'test-results/login-mobile.png',fullPage:true});
});
test('pending users see approval without service actions',async({page})=>{
  await page.route('**/api/me',r=>r.fulfill({json:{...account,user:{...account.user,status:'pending'}}}));
  await page.goto('/');await expect(page.getByRole('heading',{name:'Approval pending'})).toBeVisible();await expect(page.getByRole('button',{name:'Save and connect'})).toHaveCount(0);
});
test('personal token setup shows exact Infomaniak form values, sends csrf and keeps consent pending',async({page})=>{
  await page.route('**/api/me',r=>r.fulfill({json:account}));
  await page.route('**/api/authorize?flow=*',r=>r.fulfill({json:{client:'Test assistant',redirect_uri:'https://client.example/callback',scopes:[{scope:'mail:read',available:false}]}}));
  let request;
  await page.route('**/api/token',r=>{request=r.request();return r.fulfill({json:{error_description:'This token belongs to another Infomaniak account'},status:403})});
  await page.goto('/?authorize=flow-id');
  const setup=page.locator('section').filter({has:page.getByRole('heading',{name:'Connect your Infomaniak account'})});
  await expect(setup.getByRole('link',{name:'Open Infomaniak tokens ↗'})).toHaveAttribute('href','https://manager.infomaniak.com/v3/ng/accounts/token/list');
  await expect(setup.locator('.token-fields tr')).toHaveText(['Name des TokensMCP Server Team Ensemble','AnwendungStandardanwendung','ScopesAlle','GültigkeitsdauerUnbegrenzt','Beschreibungleer lassen']);
  await expect(setup.getByRole('button',{name:/copy/i})).toHaveCount(0);
  await expect(setup.getByRole('button',{name:'Save and connect'})).toBeDisabled();
  await setup.getByLabel('Infomaniak API token').fill('personal-token');
  await setup.getByRole('button',{name:'Save and connect'}).click();await expect(page.getByRole('alert')).toContainText('another Infomaniak account');
  expect(request.headers()['x-csrf-token']).toBe('test-csrf');expect(request.postDataJSON()).toEqual({token:'personal-token'});
  await expect(page.getByRole('button',{name:'Allow selected access'})).toBeDisabled();
  await page.screenshot({path:'test-results/connections.png',fullPage:true});
  await page.setViewportSize({width:375,height:812});expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
});
test('saved tokens collapse the setup until the user reopens it',async({page})=>{
  await page.route('**/api/me',r=>r.fulfill({json:{...account,connections:[{provider:'infomaniak',scopes:'*',expires:4102444800,health:'connected',checked:0}]}}));
  await page.goto('/');
  await expect(page.getByRole('heading',{name:'Infomaniak API token saved'})).toBeVisible();await expect(page.getByLabel('Infomaniak API token')).toHaveCount(0);
  await page.getByRole('button',{name:'Show token setup'}).click();await expect(page.getByRole('heading',{name:'Replace your Infomaniak API token'})).toBeVisible();await expect(page.getByLabel('Infomaniak API token')).toBeVisible();
  await page.getByRole('button',{name:'Hide',exact:true}).click();await expect(page.getByRole('heading',{name:'Infomaniak API token saved'})).toBeVisible();
  await page.screenshot({path:'test-results/connections-saved.png',fullPage:true});
});
test('client grants show readable permissions and can be edited in place',async({page})=>{
  const scopes=[{scope:'mail:read',available:true},{scope:'mail:write',available:true},{scope:'calendar:read',available:false},{scope:'calendar:write',available:false},{scope:'kdrive:read',available:true},{scope:'kdrive:write',available:true}];
  await page.route('**/api/me',r=>r.fulfill({json:{...account,scopes,clients:[{id:'grant-1',name:'ChatGPT',scopes:'mail:read mail:write calendar:read'}]}}));
  let request;await page.route('**/api/clients/grant-1/scopes',r=>{request=r.request();return r.fulfill({json:{ok:true,scopes:['mail:read','calendar:read']}})});
  await page.goto('/');
  const grants=page.locator('section').filter({has:page.getByRole('heading',{name:'Client grants'})});
  await expect(grants.getByText('Mail: read & write · Calendar: read only')).toBeVisible();
  await grants.getByRole('button',{name:'Edit permissions'}).click();
  await expect(grants.getByLabel(/Calendar · read calendar:read/)).toBeEnabled();await expect(grants.getByLabel(/Calendar · write actions/)).toBeDisabled();
  await grants.getByLabel(/Mail · write actions/).uncheck();await grants.getByLabel(/kDrive · read kdrive:read/).check();await grants.getByLabel(/kDrive · read kdrive:read/).uncheck();
  await page.screenshot({path:'test-results/grant-edit.png',fullPage:true});
  await grants.getByRole('button',{name:'Save permissions'}).click();
  await expect(page.getByRole('status')).toContainText('Permissions for ChatGPT updated');
  expect(request.postDataJSON()).toEqual({scopes:['mail:read','calendar:read']});expect(request.headers()['x-csrf-token']).toBe('test-csrf');
});
test('administration shows configuration without rendering supplied html',async({page})=>{
  await page.route('**/api/me',r=>r.fulfill({json:{...account,user:{...account.user,admin:true}}}));
  await page.route('**/api/admin',r=>r.fulfill({json:{users:[{...account.user,name:'<img src=x onerror=alert(1)>'}],modules:[{id:'mail',enabled:false,verification:'',default_level:0}],groups:[],memberships:[],policies:[],connections:[],logs:[],manual_approval:false}}));
  await page.goto('/');await page.getByRole('button',{name:'Administration'}).click();await expect(page.getByRole('heading',{name:'People'})).toBeVisible();await expect(page.locator('img')).toHaveCount(0);await expect(page.getByText('<img src=x onerror=alert(1)>')).toBeVisible();
  await page.screenshot({path:'test-results/admin.png',fullPage:true});
});
