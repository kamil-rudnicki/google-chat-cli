// Chrome serializes this function when it injects it into the Home tab.
export async function readHome(expectedAccount, temporary) {
  if (location.origin !== "https://chat.google.com" ||
      !/^\/app(?:\/u\/\d+)?\/home\/?$/.test(location.pathname)) {
    return {error: "Open Google Chat Home in the Chrome profile connected to gchat."};
  }

  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const deadline = Date.now() + (temporary ? 12_000 : 1_800);
  async function waitFor(read) {
    while (Date.now() < deadline) {
      const result = read();
      if (result) return result;
      await sleep(100);
    }
    throw new Error("Google Chat Home did not finish loading. Check the Chrome tab and try again.");
  }

  const visible = element => Boolean(element?.getClientRects().length);
  const findList = () => Array.from(document.querySelectorAll('[role="list"][jsname="WbL0Ac"][aria-labelledby]'))
    .find(list => list.closest('[jsname="WhB0mf"]'));
  const getText = (element, name) => element.querySelector('[jsname="' + name + '"]')?.innerText.trim() || null;

  try {
    const unread = await waitFor(() => document.querySelector('[role="switch"][jsname="DMn7nd"]'));
    const thread = await waitFor(() => document.querySelector('[role="toolbar"] button[jsname="JIbuQc"]'));
    const account = await waitFor(() => Array.from(document.querySelectorAll('a[aria-label]'))
      .map(link => link.matches('a[href*="accounts.google.com"]') || link.closest('#gb')
        ? link.getAttribute('aria-label').match(/\(([^()\s]+@[^()\s]+)\)\s*$/)?.[1]?.toLowerCase()
        : null)
      .find(Boolean));

    if (expectedAccount && expectedAccount !== account) {
      return {error: "Chrome Chat is signed in as " + account + ". Open Home for " + expectedAccount + " in this profile."};
    }

    const changeUnread = unread.getAttribute('aria-checked') !== 'true';
    const changeThread = thread.getAttribute('aria-pressed') === 'true';
    if ((changeUnread || changeThread) && !temporary) return {needsTemporaryTab: true};

    try {
      if (changeThread) thread.click();
      if (changeUnread) unread.click();
      if (temporary) await sleep(450);

      const ready = await waitFor(() => {
        if (unread.getAttribute('aria-checked') !== 'true' || thread.getAttribute('aria-pressed') === 'true') return null;
        const list = findList();
        const scroller = list?.closest('[jsname="WhB0mf"]');
        if (!scroller || Array.from(scroller.querySelectorAll('[role="progressbar"],[aria-busy="true"]')).some(visible)) return null;
        const rows = Array.from(list.querySelectorAll(':scope > [role="listitem"][data-group-id]'));
        if (rows.some(row => row.dataset.isUnread !== 'true')) return null;
        if (rows.length || visible(list) || scroller.innerText.trim()) return {list, scroller};
        return null;
      });

      const {list, scroller} = ready;
      if (!temporary && scroller.scrollHeight > scroller.clientHeight + 1) return {needsTemporaryTab: true};

      const items = new Map();
      function collect() {
        for (const row of list.querySelectorAll(':scope > [role="listitem"][data-group-id][data-is-unread="true"]')) {
          const {groupId, topicId, displayTimestamp} = row.dataset;
          const isThread = Boolean(row.querySelector('[data-item^="mark-thread-as-"]'));
          const title = getText(row, 'VYJYmb');
          if (!title || (isThread && !topicId)) throw new Error("Google Chat changed the Home row layout.");
          const timestamp = displayTimestamp ? Number(displayTimestamp) : NaN;
          const key = isThread ? groupId + '/' + topicId : groupId;
          items.set(key, {
            title,
            type: isThread ? 'thread' : 'conversation',
            groupId,
            threadId: isThread ? topicId : null,
            timestamp: Number.isFinite(timestamp) ? new Date(timestamp).toISOString() : null,
            messagePreview: getText(row, 'ok3btb'),
            replyPreview: getText(row, 'yJwwfb'),
          });
        }
      }

      if (temporary && scroller.scrollTop > 0) {
        scroller.scrollTop = 0;
        await sleep(200);
      }
      collect();
      if (temporary && scroller.scrollHeight > scroller.clientHeight + 1) {
        let previousBottom = null;
        while (Date.now() < deadline) {
          const before = scroller.scrollHeight;
          scroller.scrollTop += Math.max(scroller.clientHeight * 0.8, 200);
          await sleep(350);
          collect();
          const loading = Array.from(scroller.querySelectorAll('[role="progressbar"],[aria-busy="true"]')).some(visible);
          const atBottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 1;
          const bottom = atBottom && !loading && scroller.scrollHeight === before ? before : null;
          if (bottom !== null && bottom === previousBottom) break;
          previousBottom = bottom;
        }
        if (Date.now() >= deadline) throw new Error("Google Chat Home is still loading unread rows. Try again.");
      }

      return {account, items: Array.from(items.values())};
    } finally {
      if (changeUnread && unread.getAttribute('aria-checked') === 'true') unread.click();
      if (changeThread && thread.getAttribute('aria-pressed') === 'false') thread.click();
    }
  } catch (error) {
    return {error: error.message};
  }
}
