ui.mount((initial, input) => {
  const drive = initial.context.drive_id;
  const file = initial.context.file_id;
  ui.header('kDrive', initial.context.folder_name || 'Folder', 'Expand folders to browse their contents.', initial.context.source_url);
  function branch(parent, page, ancestors) {
    const list = ui.element('ul', null, 'tree');
    const footer = ui.element('div');
    const container = ui.element('div');
    container.append(list, footer);
    let cursor = page.cursor, more = page.has_more;
    const seen = new Set();
    function append(items) {
      for (const item of items) {
        if (seen.has(item.id)) continue;
        seen.add(item.id);
        const row = ui.element('li');
        if (item.type === 'dir') {
          const details = ui.element('details');
          const summary = ui.element('summary', item.name, 'folder-label');
          details.append(summary);
          let loaded = false, loading = false;
          details.ontoggle = async () => {
            if (!details.open || loaded || loading) return;
            if (ancestors.includes(item.id) || ancestors.length >= 16) {
              loaded = true;
              details.append(ui.element('p', 'This branch cannot be expanded further. Open it in kDrive.', 'notice'));
              return;
            }
            loading = true;
            const status = ui.element('p', 'Loading folder…', 'muted');
            details.append(status);
            try {
              const next = await load(item.id);
              status.replaceWith(branch(item.id, next, [...ancestors, item.id]));
              loaded = true;
            } catch (reason) {
              status.textContent = 'Could not load this folder. Collapse and expand to retry.';
              ui.error(reason);
            } finally { loading = false; if (!loaded) status.remove(); }
          };
          row.append(details);
        } else {
          row.className = 'file-row';
          row.append(ui.element('span', '▤', 'muted'), ui.link(item.name || 'Untitled file', item.source_url));
          if (Number.isFinite(item.size)) row.append(ui.element('span', ui.size(item.size), 'muted'));
        }
        list.append(row);
      }
    }
    async function load(folder, nextCursor) {
      const args = {drive_id: drive, file_id: folder};
      if (input.type) args.type = input.type;
      if (nextCursor) args.cursor = nextCursor;
      const next = await ui.call('kdrive_show_folder', args);
      if (next.context.drive_id !== drive || next.context.file_id !== folder) throw new Error('The returned folder does not match this branch.');
      return next;
    }
    function paging() {
      footer.replaceChildren();
      if (more) footer.append(ui.button('Load more items', async () => {
        const next = await load(parent, cursor);
        append(next.data);
        cursor = next.cursor;
        more = next.has_more;
        paging();
      }));
      else if (!seen.size) footer.append(ui.element('p', 'Empty folder', 'muted'));
    }
    append(page.data);
    paging();
    return container;
  }
  ui.root.append(branch(file, initial, [file]));
  if (input.cursor) ui.root.append(ui.element('p', 'Showing a continuation page of this folder.', 'muted'));
});
