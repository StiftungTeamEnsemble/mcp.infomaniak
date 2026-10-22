ui.mount(({data: post, context}) => {
  const author = context.author?.name || (context.author?.username ? '@' + context.author.username : 'kChat user');
  const channel = context.channel?.type === 'D' ? 'Direct message' : context.channel?.type === 'G' ? 'Group message' : context.channel?.name ? '#' + context.channel.name : '';
  const when = post.create_at ? ui.date(post.create_at, {dateStyle: 'medium', timeStyle: 'short'}) : '';
  ui.header('kChat', author, [channel, when].filter(Boolean).join(' · '), post.source_url);
  ui.root.append(ui.element('div', post.message || '(Empty message)', 'body'));
  if (post.root_id) ui.root.append(ui.element('p', 'Reply in a thread', 'muted'));
});
