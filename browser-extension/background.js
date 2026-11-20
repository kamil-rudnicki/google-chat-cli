import {readHome} from './home.js';

const host = 'com.gchat.home';
const homeUrl = 'https://chat.google.com/app/home';
let port;

async function execute(tabId, account, temporary) {
  const [result] = await chrome.scripting.executeScript({
    target: {tabId}, func: readHome, args: [account, temporary],
  });
  return result?.result ?? {error: 'Google Chat Home did not return a list.'};
}

async function listUnread(account) {
  const tabs = await chrome.tabs.query({url: 'https://chat.google.com/*'});
  const homeTabs = tabs.filter(tab => /^https:\/\/chat\.google\.com\/app(?:\/u\/\d+)?\/home\/?(?:[?#].*)?$/.test(tab.url));
  let url = homeUrl;
  let error;
  for (const tab of homeTabs) {
    try {
      const result = await execute(tab.id, account, false);
      if (result.items) return result;
      if (result.needsTemporaryTab) {
        url = tab.url;
        break;
      }
      error = result;
    } catch {
      // The tab may have closed or navigated after it was listed.
    }
  }

  const tab = await chrome.tabs.create({url, active: false});
  try {
    // Wait for Chat's document to replace the initial about:blank.
    for (let i = 0; i < 30; i++) {
      const current = await chrome.tabs.get(tab.id);
      if (current.status === 'complete' && current.url && current.url !== 'about:blank') {
        if (!current.url.startsWith('https://chat.google.com/')) {
          return error ?? {error: 'Sign in to Google Chat in this Chrome profile, then try again.'};
        }
        return await execute(tab.id, account, true);
      }
      await new Promise(resolve => setTimeout(resolve, 200));
    }
    return {error: 'Google Chat did not load in Chrome. Open Chat Home and try again.'};
  } finally {
    await chrome.tabs.remove(tab.id).catch(() => {});
  }
}

function connect() {
  if (port) return;
  const connection = chrome.runtime.connectNative(host);
  port = connection;
  connection.onDisconnect.addListener(() => {
    port = undefined;
    const message = chrome.runtime.lastError?.message;
    if (message) console.warn(message);
    chrome.action.setBadgeText({text: '!' });
  });
  connection.onMessage.addListener(async request => {
    let result;
    try {
      result = await listUnread(request.account);
    } catch (error) {
      result = {error: error.message};
    }
    if (port === connection) connection.postMessage(result);
  });
  chrome.action.setBadgeText({text: ''});
}

chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
chrome.action.onClicked.addListener(connect);
connect();
