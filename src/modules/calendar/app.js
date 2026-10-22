function availabilityView(snapshot, input) {
  const data = snapshot.data;
  const timezone = data.timezone;
  const dayKey = value => new Intl.DateTimeFormat('en-CA', {timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit'}).format(new Date(value));
  const time = value => ui.date(value, {hour: '2-digit', minute: '2-digit'}, timezone);
  const label = calendar => calendar.name || 'Calendar ' + calendar.calendar_id;
  ui.header('Calendar', 'Common free time', `${data.calendars.map(label).join(', ')} · ${data.duration_minutes}-minute meeting · ${timezone}`, snapshot.context.source_url);
  ui.root.append(ui.element('p', 'Free windows within your selected working hours. Choose a window to share a suggested start time with the conversation.', 'muted'));
  const dates = new Map();
  for (const slot of data.slots) {
    const key = dayKey(slot.start);
    if (!dates.has(key)) dates.set(key, []);
    dates.get(key).push(slot);
  }
  const toolbar = ui.element('div', null, 'toolbar');
  const day = ui.element('label', 'Day ');
  const select = ui.element('select');
  select.setAttribute('aria-label', 'Available day');
  for (const [key, slots] of dates) {
    const option = ui.element('option', ui.date(slots[0].start, {weekday: 'short', month: 'short', day: 'numeric'}, timezone));
    option.value = key;
    select.append(option);
  }
  day.append(select);
  const results = ui.element('section');
  toolbar.append(day, ui.element('span', `${data.slots.length} free window${data.slots.length === 1 ? '' : 's'}${snapshot.has_more ? ' on this page' : ''}`, 'badge'));
  let selected;
  function showDay() {
    results.replaceChildren();
    const slots = dates.get(select.value) || [];
    if (!slots.length) return;
    const timeline = ui.element('div', null, 'timeline');
    // Scale by elapsed instants, including days with repeated or missing hours.
    const hours = data.working_windows.find(hours => dayKey(hours.start) === select.value);
    const begin = new Date(hours.start).getTime(), end = new Date(hours.end).getTime();
    const row = (title, intervals, free = false) => {
      const row = ui.element('div', null, 'timeline-row');
      const name = ui.element('span', title, 'timeline-label');
      name.title = title;
      const track = ui.element('div', null, 'track');
      track.setAttribute('role', 'img');
      track.setAttribute('aria-label', `${title}: ${intervals.length ? intervals.map(s => `${time(s.start)}–${time(s.end)}`).join(', ') : 'No busy events'}`);
      for (const interval of intervals) {
        const bar = ui.element('span', null, 'bar' + (free ? ' free' : ''));
        const start = Math.max(begin, new Date(interval.start).getTime());
        const finish = Math.min(end, new Date(interval.end).getTime());
        if (finish <= start) continue;
        bar.style.left = `${(start - begin) / (end - begin) * 100}%`;
        bar.style.width = `${(finish - start) / (end - begin) * 100}%`;
        bar.title = `${time(interval.start)}–${time(interval.end)}`;
        track.append(bar);
      }
      row.append(name, track);
      timeline.append(row);
    };
    const axis = ui.element('div', null, 'axis');
    for (const text of [time(begin), time((begin + end) / 2), time(end)]) axis.append(ui.element('span', text));
    timeline.append(axis);
    for (const calendar of data.calendars) row(label(calendar), calendar.busy.filter(s => dayKey(s.start) <= select.value && dayKey(s.end) >= select.value));
    row('Everyone free', slots, true);
    timeline.append(ui.element('p', 'Brown: busy · Green: common free windows', 'legend'));
    results.append(timeline, ui.element('h2', 'Common free windows'));
    const list = ui.element('div', null, 'slots');
    for (const slot of slots) {
      const button = ui.button('', async node => {
        await ui.select(`Suggested ${data.duration_minutes}-minute meeting starting ${slot.start} (${timezone}); the free window ends ${slot.end}. Compared calendars: ${data.calendars.map(c => `${label(c)} (ID ${c.calendar_id})`).join(', ')}. Recheck availability before booking; no event has been created.`);
        selected?.setAttribute('aria-pressed', 'false');
        node.setAttribute('aria-pressed', 'true');
        selected = node;
        status.textContent = 'Suggested start shared with the conversation. Continue in chat to arrange the meeting.';
      }, 'slot');
      button.setAttribute('aria-pressed', 'false');
      const text = ui.element('span');
      text.append(ui.element('strong', `${time(slot.start)} – ${time(slot.end)}`), ui.element('span', `${slot.duration_minutes} minutes available`, 'muted'));
      button.append(text, ui.element('span', 'Choose start →', 'muted'));
      list.append(button);
    }
    results.append(list);
  }
  const status = ui.element('p', '', 'notice');
  status.setAttribute('role', 'status');
  select.onchange = showDay;
  if (dates.size) {
    ui.root.append(toolbar, results);
    showDay();
  } else ui.root.append(ui.element('p', 'No common free window matches these calendars, working hours and meeting duration.', 'notice'));
  if (snapshot.has_more) ui.root.append(ui.button('More free windows', async () => {
    const nextInput = {...input, from: snapshot.context.next_from};
    availabilityView(await ui.call('calendar_find_free_slots', nextInput), nextInput);
  }));
  ui.root.append(status, ui.element('p', 'Only the supplied calendars were checked. Times are suggestions; recheck before booking.', 'muted'));
}
ui.mount(availabilityView);
