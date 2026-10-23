import {test, expect} from '@playwright/test';

async function app(page, module, structuredContent, input = {}, replies = [], capabilities) {
  await page.goto('/');
  await page.evaluate(({module, structuredContent, input, replies, capabilities}) => {
    document.body.replaceChildren();
    window.appRequests = [];
    window.addEventListener('message', event => {
      const frame = document.querySelector('iframe');
      if (event.source !== frame.contentWindow) return;
      const message = event.data;
      window.appRequests.push(message);
      const send = value => frame.contentWindow.postMessage({jsonrpc: '2.0', ...value}, '*');
      if (message.method === 'ui/initialize') send({id: message.id, result: {protocolVersion: '2026-01-26', hostCapabilities: capabilities || {openLinks: {}, serverTools: {}, updateModelContext: {text: {}}}, hostContext: {theme: 'light', locale: 'en-GB'}}});
      else if (message.method === 'ui/notifications/initialized') {
        send({method: 'ui/notifications/tool-input', params: {arguments: input}});
        send({method: 'ui/notifications/tool-result', params: {structuredContent}});
      } else if (message.method === 'tools/call') {
        const next = replies.shift();
        send(next?.error ? {id: message.id, error: next.error} : {id: message.id, result: next});
      } else if (message.id != null) send({id: message.id, result: {}});
    });
    const frame = document.createElement('iframe');
    frame.title = 'MCP App';
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.style.cssText = 'border:0;width:100%;height:1100px';
    frame.src = `/mcp-app-preview/${module}`;
    document.body.append(frame);
  }, {module, structuredContent, input, replies, capabilities});
  return page.frameLocator('iframe');
}

test('kChat receives host results, opens permalinks and treats message markup as text', async ({page}) => {
  const frame = await app(page, 'kchat', {data: {id: 'post1', user_id: 'alice', channel_id: 'town', message: '<img src=x onerror=alert(1)>\nHello everyone', create_at: 1792652400000, source_url: 'https://ensemble.kchat.infomaniak.com/ensemble/pl/post1'}, context: {author: {username: 'lara', name: 'Lara Muster'}, channel: {name: 'Kommunikation', type: 'O'}}});
  await expect(frame.getByText('Hello everyone', {exact: false})).toBeVisible();
  await expect(frame.getByRole('heading', {name: 'Lara Muster'})).toBeVisible();
  await expect(frame.getByText('#Kommunikation', {exact: false})).toBeVisible();
  await expect(frame.getByText('alice')).toHaveCount(0);
  await expect(frame.locator('img')).toHaveCount(0);
  await frame.getByRole('button', {name: 'Open in kChat'}).click();
  await expect.poll(() => page.evaluate(() => window.appRequests.find(r => r.method === 'ui/open-link')?.params.url)).toBe('https://ensemble.kchat.infomaniak.com/ensemble/pl/post1');
  await page.screenshot({path: 'test-results/mcp-kchat.png'});
});

test('email preserves safe formatting and blocks active content and tracking requests', async ({page}) => {
  const requests = [];
  page.on('request', request => { if (request.url().includes('tracker.invalid')) requests.push(request.url()); });
  const frame = await app(page, 'mail', {data: {subject: 'Project update', from: {name: 'Alice', email: 'alice@example.org'}, to: [{email: 'bob@example.org'}], body: {type: 'text/html', value: '<h2>Next steps</h2><p>Hello <strong>Bob</strong>.</p><img src="https://tracker.invalid/pixel"><script>alert(1)</script><svg onload="alert(2)"></svg><a href="javascript:alert(3)">Unsafe link</a><a href="https://example.org/notes">Read notes</a>'}, attachments: [{name: 'brief.pdf', mime_type: 'application/pdf', size: 204800}]}, context: {source_url: 'https://mail.infomaniak.com'}});
  await expect(frame.getByRole('heading', {name: 'Project update'})).toBeVisible();
  await expect(frame.locator('strong')).toHaveText('Bob');
  await expect(frame.locator('img,svg,iframe,form')).toHaveCount(0);
  await expect(frame.getByRole('button', {name: 'Unsafe link'})).toHaveCount(0);
  await expect(frame.getByText('brief.pdf')).toBeVisible();
  await expect(frame.getByText('application/pdf · 200 KB')).toBeVisible();
  await expect(frame.getByText('[object Object]')).toHaveCount(0);
  expect(requests).toEqual([]);
  await frame.getByRole('button', {name: 'Read notes'}).click();
  await expect.poll(() => page.evaluate(() => window.appRequests.find(r => r.method === 'ui/open-link')?.params.url)).toBe('https://example.org/notes');
  await page.screenshot({path: 'test-results/mcp-mail.png'});
});

test('kDrive expands and pages each folder using its own drive, folder and cursor', async ({page}) => {
  const folder = (data, file, more = false, cursor = null) => ({structuredContent: {data, context: {drive_id: 9, file_id: file}, has_more: more, cursor}});
  const item = (id, name) => ({id, name, type: 'file', source_url: `https://ksuite.infomaniak.com/all/kdrive/app/drive/9/files/2/preview/pdf/${id}`});
  const frame = await app(page, 'kdrive', {data: [{id: 2, name: 'Design', type: 'dir', source_url: 'https://ksuite.infomaniak.com/all/kdrive/app/drive/9/files/2'}, {...item(42, 'Brief.txt'), size: 1536}], context: {drive_id: 9, file_id: 1, folder_name: 'Projects'}, has_more: true, cursor: 'root-next'}, {}, [
    folder([item(43, 'Sketch.pdf')], 2, true, 'child-next'),
    folder([item(44, 'Final.pdf')], 2),
    folder([item(45, 'Schedule.txt')], 1),
  ]);
  await expect(frame.getByRole('heading', {name: 'Projects'})).toBeVisible();
  await expect(frame.getByText('1.5 KB')).toBeVisible();
  await frame.getByRole('button', {name: 'Open Design in kDrive'}).click();
  await expect.poll(() => page.evaluate(() => window.appRequests.find(r => r.method === 'ui/open-link')?.params.url)).toBe('https://ksuite.infomaniak.com/all/kdrive/app/drive/9/files/2');
  await expect(frame.locator('details')).not.toHaveAttribute('open', '');
  await frame.locator('summary').click();
  await expect(frame.getByRole('button', {name: 'Sketch.pdf'})).toBeVisible();
  await frame.locator('details').getByRole('button', {name: 'Load more items'}).click();
  await expect(frame.getByRole('button', {name: 'Final.pdf'})).toBeVisible();
  await frame.getByRole('button', {name: 'Load more items'}).click();
  await expect(frame.getByRole('button', {name: 'Schedule.txt'})).toBeVisible();
  expect(await page.evaluate(() => window.appRequests.filter(r => r.method === 'tools/call').map(r => r.params))).toEqual([
    {name: 'kdrive_show_folder', arguments: {drive_id: 9, file_id: 2}},
    {name: 'kdrive_show_folder', arguments: {drive_id: 9, file_id: 2, cursor: 'child-next'}},
    {name: 'kdrive_show_folder', arguments: {drive_id: 9, file_id: 1, cursor: 'root-next'}},
  ]);
  await page.screenshot({path: 'test-results/mcp-kdrive.png'});
});

test('kDrive preserves its tree when a later read loses authorisation', async ({page}) => {
  const frame = await app(page, 'kdrive', {data: [{id: 2, name: 'Private', type: 'dir'}], context: {drive_id: 9, file_id: 1}, has_more: true, cursor: 'next'}, {}, [{error: {code: -32000, message: 'Access denied; reconnect'}}]);
  await frame.getByRole('button', {name: 'Load more items'}).click();
  await expect(frame.getByRole('alert')).toContainText('Access denied');
  await expect(frame.locator('summary')).toHaveText('Private');
  await expect(frame.getByRole('button', {name: 'Load more items'})).toBeEnabled();
});

test('calendar compares timelines and shares a suggested start without creating an event', async ({page}) => {
  const slot = {start: '2026-10-22T11:00:00+02:00', end: '2026-10-22T13:00:00+02:00', duration_minutes: 120};
  const frame = await app(page, 'calendar', {data: {timezone: 'Europe/Zurich', duration_minutes: 30, slots: [slot], working_windows: [{start:'2026-10-22T09:00:00+02:00', end:'2026-10-22T17:00:00+02:00'}], calendars: [{calendar_id: '101', name: 'Alice', busy: [{start: '2026-10-22T09:00:00+02:00', end: slot.start}]}, {calendar_id: '102', name: 'Bob', busy: [{start: slot.end, end: '2026-10-22T14:00:00+02:00'}]}]}, context: {}, has_more: false});
  await expect(frame.getByRole('heading', {name: 'Common free time'})).toBeVisible();
  await expect(frame.getByRole('img', {name: /Alice/})).toBeVisible();
  await frame.getByRole('button', {name: /11:00 – 13:00/}).click();
  await expect(frame.getByRole('status')).toContainText('Suggested start shared');
  const requests = await page.evaluate(() => window.appRequests);
  expect(requests.find(r => r.method === 'ui/update-model-context').params.content[0].text).toContain('starting 2026-10-22T11:00:00+02:00');
  expect(requests.some(r => r.method === 'tools/call')).toBe(false);
  await page.screenshot({path: 'test-results/mcp-calendar.png'});
  await page.setViewportSize({width:375,height:812});
  await expect.poll(() => frame.locator('body').evaluate(body => body.scrollWidth)).toBeLessThanOrEqual(375);
  await page.screenshot({path: 'test-results/mcp-calendar-mobile.png'});
});

test('apps reject messages outside the host bridge and report missing host capabilities', async ({page}) => {
  const frame = await app(page, 'kchat', {data: {id: 'post1', message: 'Original', source_url: 'https://example.org'}, context: {}}, {}, [], {});
  await expect(frame.getByText('Original')).toBeVisible();
  await frame.locator('body').evaluate(() => window.postMessage({jsonrpc:'2.0', method:'ui/notifications/tool-result', params:{structuredContent:{data:{message:'Forged'},context:{}}}}, '*'));
  await expect(frame.getByText('Original')).toBeVisible();
  await expect(frame.getByText('Forged')).toHaveCount(0);
  await frame.getByRole('button', {name: 'Open in kChat'}).click();
  await expect(frame.getByRole('alert')).toContainText('cannot open links');
});
