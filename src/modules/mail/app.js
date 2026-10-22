// Rebuild only inert formatting. Never insert provider HTML, remote images,
// styles, attributes, forms or scripts into the live document.
function emailBody(raw) {
  const template = document.createElement('template');
  template.innerHTML = raw;
  const allowed = new Set(['P','BR','DIV','SPAN','STRONG','B','EM','I','U','S','UL','OL','LI','BLOCKQUOTE','PRE','CODE','H1','H2','H3','HR','TABLE','THEAD','TBODY','TR','TH','TD']);
  const blocked = new Set(['SCRIPT','STYLE','IFRAME','OBJECT','EMBED','SVG','MATH','FORM','INPUT','BUTTON','TEXTAREA','SELECT','LINK','META','IMG','VIDEO','AUDIO','SOURCE']);
  function copy(node, target, depth = 0) {
    if (depth > 60) return;
    if (node.nodeType === Node.TEXT_NODE) { target.append(document.createTextNode(node.textContent)); return; }
    if (node.nodeType !== Node.ELEMENT_NODE || blocked.has(node.tagName)) return;
    const next = node.tagName === 'A' ? ui.link(node.textContent, node.getAttribute('href')) : allowed.has(node.tagName) ? ui.element(node.tagName.toLowerCase()) : document.createDocumentFragment();
    if (node.tagName !== 'A') for (const child of node.childNodes) copy(child, next, depth + 1);
    target.append(next);
  }
  const body = ui.element('div', null, 'body mail-body');
  for (const node of template.content.childNodes) copy(node, body);
  return body;
}
function people(value) {
  if (!value) return '';
  if (Array.isArray(value)) return value.map(people).filter(Boolean).join(', ');
  if (typeof value === 'string') return value;
  const email = value.email || value.address || '';
  return value.name && value.name !== email ? `${value.name} <${email}>` : email;
}
// Infomaniak returns the body as {type, value}; strings are accepted for other shapes.
function content(mail) {
  const body = mail.body && typeof mail.body === 'object' ? mail.body : {value: mail.body};
  const html = typeof mail.html === 'string' ? mail.html : null;
  const raw = html ?? (typeof body.value === 'string' ? body.value : typeof mail.text === 'string' ? mail.text : '');
  const isHtml = html != null || /html/i.test(body.type || mail.mime_type || '');
  return {raw, isHtml};
}
ui.mount(({data: mail, context}) => {
  const date = mail.date || mail.sent_at;
  ui.header('Mail', mail.subject || '(No subject)', date ? ui.date(date, {dateStyle: 'medium', timeStyle: 'short'}) : '', context.source_url);
  const meta = ui.element('dl', null, 'meta');
  for (const [label, value] of [['From', people(mail.from)], ['To', people(mail.to)], ['Cc', people(mail.cc)]]) {
    if (value) meta.append(ui.element('dt', label), ui.element('dd', value));
  }
  ui.root.append(meta);
  const {raw, isHtml} = content(mail);
  ui.root.append(isHtml ? emailBody(raw) : ui.element('div', raw || '(Empty message)', 'body'));
  if (isHtml) ui.root.append(ui.element('p', 'Remote images and active content are blocked.', 'muted'));
  if (mail.attachments?.length) {
    ui.root.append(ui.element('h2', 'Attachments'));
    const attachments = ui.element('div', null, 'attachments');
    for (const file of mail.attachments) {
      const row = ui.element('div', null, 'attachment');
      row.append(ui.element('span', file.name || file.filename || 'Attachment'), ui.element('span', [file.mime_type, ui.size(file.size)].filter(Boolean).join(' · '), 'muted'));
      attachments.append(row);
    }
    ui.root.append(attachments);
  }
});
