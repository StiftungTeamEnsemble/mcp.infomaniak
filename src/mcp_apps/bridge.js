// MCP Apps 2026-01-26: JSON-RPC over the host's postMessage transport.
// https://github.com/modelcontextprotocol/ext-apps/blob/main/src/spec.types.ts
const ui = (() => {
  const root = document.getElementById('app');
  const pending = new Map();
  let sequence = 0, ready = false, render, input = {}, host = {}, capabilities = {};
  const element = (tag, text, className) => {
    const node = document.createElement(tag);
    if (text != null) node.textContent = String(text);
    if (className) node.className = className;
    return node;
  };
  function error(reason) {
    root.querySelector('[role="alert"]')?.remove();
    const message = element('p', reason?.message || String(reason), 'error');
    message.setAttribute('role', 'alert');
    root.append(message);
  }
  function send(message) { window.parent.postMessage({jsonrpc: '2.0', ...message}, '*'); }
  function request(method, params) {
    return new Promise((resolve, reject) => {
      const id = ++sequence;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error('The host did not respond. Please try again.'));
      }, 95000);
      pending.set(id, {resolve, reject, timer});
      send({id, method, params});
    });
  }
  function notify(method, params = {}) { if (ready) send({method, params}); }
  function context(value = {}) {
    host = {...host, ...value};
    document.documentElement.dataset.theme = host.theme || '';
    for (const [key, value] of Object.entries(host.styles?.variables || {})) {
      if (key.startsWith('--') && typeof value === 'string') document.documentElement.style.setProperty(key, value);
    }
  }
  function result(value) {
    if (value?.isError) throw new Error(value.content?.find(c => c.type === 'text')?.text || 'The operation failed.');
    if (!value?.structuredContent) throw new Error('The tool returned no view data.');
    return value.structuredContent;
  }
  window.addEventListener('message', event => {
    if (event.source !== window.parent || event.data?.jsonrpc !== '2.0') return;
    const message = event.data;
    const saved = pending.get(message.id);
    if (saved && !message.method) {
      clearTimeout(saved.timer);
      pending.delete(message.id);
      if (message.error) saved.reject(new Error(message.error.message || 'The host rejected the request.'));
      else saved.resolve(message.result);
      return;
    }
    if (!ready) return;
    if (message.id != null) {
      if (message.method === 'ping' || message.method === 'ui/resource-teardown') {
        send({id: message.id, result: {}});
        if (message.method === 'ui/resource-teardown') {
          ready = false;
          observer.disconnect();
          for (const saved of pending.values()) {
            clearTimeout(saved.timer);
            saved.reject(new Error('View closed.'));
          }
          pending.clear();
        }
      } else send({id: message.id, error: {code: -32601, message: 'Method not found'}});
      return;
    }
    try {
      if (message.method === 'ui/notifications/tool-input') input = message.params?.arguments || {};
      if (message.method === 'ui/notifications/tool-result') render(result(message.params), input);
      if (message.method === 'ui/notifications/tool-cancelled') error(new Error('The operation was cancelled.'));
      if (message.method === 'ui/notifications/host-context-changed') context(message.params);
    } catch (reason) { error(reason); }
  });
  const observer = new ResizeObserver(() => notify('ui/notifications/size-changed', {height: Math.ceil(document.body.getBoundingClientRect().height)}));
  observer.observe(document.body);
  function button(label, action, className) {
    const node = element('button', label, className);
    node.type = 'button';
    node.onclick = async () => {
      node.disabled = true;
      try { await action(node); } catch (reason) { error(reason); }
      finally { node.disabled = false; }
    };
    return node;
  }
  function safeUrl(raw) {
    try { const url = new URL(raw); return url.protocol === 'https:' && !url.username && !url.password ? url.href : null; }
    catch { return null; }
  }
  function link(label, raw) {
    const url = safeUrl(raw);
    if (!url) return element('span', label);
    return button(label, async () => {
      if (!capabilities.openLinks) throw new Error('This host cannot open links.');
      await request('ui/open-link', {url});
    }, 'link');
  }
  function header(eyebrow, title, subtitle, source) {
    root.replaceChildren();
    const top = element('header');
    top.append(element('p', eyebrow, 'eyebrow'), element('h1', title));
    if (subtitle) top.append(element('p', subtitle, 'muted'));
    if (source) top.append(link('Open in ' + eyebrow, source));
    root.append(top);
  }
  return {
    root, element, button, link, header, error,
    size(bytes) {
      if (!Number.isFinite(bytes)) return '';
      const units = ['B', 'KB', 'MB', 'GB', 'TB'];
      let value = bytes, unit = 0;
      while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
      return new Intl.NumberFormat(host.locale || undefined, {maximumFractionDigits: unit ? 1 : 0}).format(value) + ' ' + units[unit];
    },
    date(value, options = {}, timezone) {
      const date = new Date(value);
      return Number.isNaN(date.getTime()) ? String(value ?? '') : new Intl.DateTimeFormat(host.locale || undefined, {...options, ...(timezone ? {timeZone: timezone} : {})}).format(date);
    },
    async call(name, args) {
      if (!capabilities.serverTools) throw new Error('This host cannot load additional data.');
      return result(await request('tools/call', {name, arguments: args}));
    },
    async select(text) {
      if (!capabilities.updateModelContext) throw new Error('This host cannot share the selected time.');
      await request('ui/update-model-context', {content: [{type: 'text', text}]});
    },
    mount(callback) {
      render = callback;
      request('ui/initialize', {appInfo: {name: document.title, version: '1.0.0'}, appCapabilities: {availableDisplayModes: ['inline']}, protocolVersion: '2026-01-26'})
        .then(value => {
          // The host answers with the version it negotiated; the messages used here are stable across versions.
          if (typeof value?.protocolVersion !== 'string') throw new Error('The host did not complete the MCP Apps handshake.');
          context(value.hostContext);
          capabilities = value.hostCapabilities || {};
          ready = true;
          notify('ui/notifications/initialized');
          notify('ui/notifications/size-changed', {height: Math.ceil(document.body.getBoundingClientRect().height)});
        }).catch(error);
    },
  };
})();
