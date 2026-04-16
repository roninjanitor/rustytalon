// RustyTalon Web Gateway - Client

let token = '';
let eventSource = null;
let logEventSource = null;
let currentTab = 'chat';
let currentThreadId = null;
let assistantThreadId = null;
let hasMore = false;
let oldestTimestamp = null;
let loadingOlder = false;
let jobEvents = new Map(); // job_id -> Array of events
let jobListRefreshTimer = null;
const JOB_EVENTS_CAP = 500;

// Activity panel: tracks the live in-progress turn's collapsible activity log
let pendingActivityEl = null; // the .activity-panel DOM node being built for the current turn
let lastToolEntryEl = null;   // the last <details> tool entry, for attaching results
let pendingTokensIn = 0;      // accumulated input tokens for current turn
let pendingTokensOut = 0;     // accumulated output tokens for current turn

// Conversation-level token tracking (persists across turns for the current thread)
let convTokensIn = 0;   // total input tokens for current conversation (from DB + live)
let convTokensOut = 0;  // total output tokens for current conversation (from DB + live)

// --- Auth ---

function authenticate() {
  token = document.getElementById('token-input').value.trim();
  if (!token) {
    document.getElementById('auth-error').textContent = 'Token required';
    return;
  }

  // Test the token against the health-ish endpoint (chat/threads requires auth)
  apiFetch('/api/chat/threads')
    .then(() => {
      sessionStorage.setItem('rustytalon_token', token);
      document.getElementById('auth-screen').style.display = 'none';
      document.getElementById('app').style.display = 'flex';
      // Strip token from URL so it's not visible in the address bar
      const cleaned = new URL(window.location);
      cleaned.searchParams.delete('token');
      window.history.replaceState({}, '', cleaned.pathname + cleaned.search);
      connectSSE();
      connectLogSSE();
      startGatewayStatusPolling();
      loadThreads();
      loadMemoryTree();
      loadJobs();
    })
    .catch(() => {
      sessionStorage.removeItem('rustytalon_token');
      document.getElementById('auth-screen').style.display = '';
      document.getElementById('app').style.display = 'none';
      document.getElementById('auth-error').textContent = 'Invalid token';
    });
}

document.getElementById('token-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') authenticate();
});

// Auto-authenticate from URL param or saved session
(function autoAuth() {
  const params = new URLSearchParams(window.location.search);
  const urlToken = params.get('token');
  if (urlToken) {
    document.getElementById('token-input').value = urlToken;
    authenticate();
    return;
  }
  const saved = sessionStorage.getItem('rustytalon_token');
  if (saved) {
    document.getElementById('token-input').value = saved;
    // Hide auth screen immediately to prevent flash, authenticate() will
    // restore it if the token turns out to be invalid.
    document.getElementById('auth-screen').style.display = 'none';
    document.getElementById('app').style.display = 'flex';
    authenticate();
  }
})();

// --- API helper ---

function apiFetch(path, options) {
  const opts = options || {};
  opts.headers = opts.headers || {};
  opts.headers['Authorization'] = 'Bearer ' + token;
  if (opts.body && typeof opts.body === 'object') {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(opts.body);
  }
  return fetch(path, opts).then((res) => {
    if (!res.ok) throw new Error(res.status + ' ' + res.statusText);
    return res.json();
  });
}

// --- SSE ---

function connectSSE() {
  if (eventSource) eventSource.close();

  eventSource = new EventSource('/api/chat/events?token=' + encodeURIComponent(token));

  eventSource.onopen = () => {
    document.getElementById('sse-dot').classList.remove('disconnected');
    document.getElementById('sse-status').textContent = 'Connected';
  };

  eventSource.onerror = () => {
    document.getElementById('sse-dot').classList.add('disconnected');
    document.getElementById('sse-status').textContent = 'Reconnecting...';
  };

  eventSource.addEventListener('response', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    sealActivityPanel();
    addMessage('assistant', data.content);
    setStatus('');
    enableChatInput();
    // Refresh thread list so new titles appear after first message
    loadThreads();
  });

  eventSource.addEventListener('thinking', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    setStatus(data.message, true);
  });

  eventSource.addEventListener('tool_started', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    addToolEntry(data.name, data.input);
    setStatus('Running: ' + data.name, true);
  });

  eventSource.addEventListener('tool_completed', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    const icon = data.success ? '\u2713' : '\u2717';
    markLastToolCompleted(data.success);
    setStatus('Tool ' + data.name + ' ' + icon);
  });

  eventSource.addEventListener('tool_result', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    updateLastToolResult(data.preview);
  });

  eventSource.addEventListener('tokens_used', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    const inTok = data.input_tokens || 0;
    const outTok = data.output_tokens || 0;
    pendingTokensIn += inTok;
    pendingTokensOut += outTok;
    convTokensIn += inTok;
    convTokensOut += outTok;
    updateActivityPanelLabel();
    renderConversationTokenStats();
  });

  eventSource.addEventListener('stream_chunk', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    appendToLastAssistant(data.content);
  });

  eventSource.addEventListener('status', (e) => {
    const data = JSON.parse(e.data);
    if (!isCurrentThread(data.thread_id)) return;
    setStatus(data.message);
    // "Done" and "Awaiting approval" are terminal signals from the agent:
    // the agentic loop finished, so re-enable input as a safety net in case
    // the response SSE event is empty or lost.
    if (data.message === 'Done' || data.message === 'Interrupted') {
      sealActivityPanel();
      enableChatInput();
    }
    if (data.message === 'Awaiting approval') {
      enableChatInput();
    }
  });

  eventSource.addEventListener('job_started', (e) => {
    const data = JSON.parse(e.data);
    showJobCard(data);
  });

  eventSource.addEventListener('approval_needed', (e) => {
    const data = JSON.parse(e.data);
    sealActivityPanel();
    showApproval(data);
  });

  eventSource.addEventListener('auth_required', (e) => {
    const data = JSON.parse(e.data);
    showAuthCard(data);
  });

  eventSource.addEventListener('auth_completed', (e) => {
    const data = JSON.parse(e.data);
    removeAuthCard(data.extension_name);
    if (data.success) {
      showToast(data.message, 'success');
    } else {
      showToast(data.message, 'error');
    }
    enableChatInput();
    if (currentTab === 'channels') loadChannels();
    // Advance wizard if it's waiting for OAuth completion
    if (wizardState && wizardState.name === data.extension_name && wizardState.waitingForAuth) {
      wizardState.waitingForAuth = false;
      if (data.success) {
        wizardState.entry.authenticated = true;
        wizardState.entry.active = true;
        wizardState.steps = buildWizardSteps(wizardState.entry, wizardState.authInfo);
        wizardAdvance();
      }
    }
  });

  eventSource.addEventListener('error', (e) => {
    if (e.data) {
      const data = JSON.parse(e.data);
      if (!isCurrentThread(data.thread_id)) return;
      addMessage('system', 'Error: ' + data.message);
      enableChatInput();
    }
  });

  // Job event listeners (activity stream for all sandbox jobs)
  const jobEventTypes = [
    'job_message', 'job_tool_use', 'job_tool_result',
    'job_status', 'job_result'
  ];
  for (const evtType of jobEventTypes) {
    eventSource.addEventListener(evtType, (e) => {
      const data = JSON.parse(e.data);
      const jobId = data.job_id;
      if (!jobId) return;
      if (!jobEvents.has(jobId)) jobEvents.set(jobId, []);
      const events = jobEvents.get(jobId);
      events.push({ type: evtType, data: data, ts: Date.now() });
      // Cap per-job events to prevent memory leak
      while (events.length > JOB_EVENTS_CAP) events.shift();
      // If the Activity tab is currently visible for this job, refresh it
      refreshActivityTab(jobId);
      // Auto-refresh job list when on jobs tab (debounced)
      if ((evtType === 'job_result' || evtType === 'job_status') && currentTab === 'jobs' && !currentJobId) {
        clearTimeout(jobListRefreshTimer);
        jobListRefreshTimer = setTimeout(loadJobs, 200);
      }
      // Clean up finished job events after a viewing window
      if (evtType === 'job_result') {
        setTimeout(() => jobEvents.delete(jobId), 60000);
      }
    });
  }
}

// Check if an SSE event belongs to the currently viewed thread.
// Events without a thread_id (legacy) are always shown.
function isCurrentThread(threadId) {
  if (!threadId) return true;
  if (!currentThreadId) return true;
  return threadId === currentThreadId;
}

// --- Chat ---

function sendMessage() {
  const input = document.getElementById('chat-input');
  const sendBtn = document.getElementById('send-btn');
  const content = input.value.trim();
  if (!content) return;

  addMessage('user', content);
  input.value = '';
  autoResizeTextarea(input);
  setStatus('Sending...', true);

  sendBtn.disabled = true;
  input.disabled = true;

  apiFetch('/api/chat/send', {
    method: 'POST',
    body: { content, thread_id: currentThreadId || undefined },
  }).catch((err) => {
    addMessage('system', 'Failed to send: ' + err.message);
    setStatus('');
    enableChatInput();
  });
}

function enableChatInput() {
  const input = document.getElementById('chat-input');
  const sendBtn = document.getElementById('send-btn');
  sendBtn.disabled = false;
  input.disabled = false;
  input.focus();
}

function sendApprovalAction(requestId, action) {
  apiFetch('/api/chat/approval', {
    method: 'POST',
    body: { request_id: requestId, action: action, thread_id: currentThreadId },
  }).catch((err) => {
    addMessage('system', 'Failed to send approval: ' + err.message);
  });

  // Disable buttons and show confirmation on the card
  const card = document.querySelector('.approval-card[data-request-id="' + requestId + '"]');
  if (card) {
    const buttons = card.querySelectorAll('.approval-actions button');
    buttons.forEach((btn) => {
      btn.disabled = true;
    });
    const actions = card.querySelector('.approval-actions');
    const label = document.createElement('span');
    label.className = 'approval-resolved';
    const labelText = action === 'approve' ? 'Approved' : action === 'always' ? 'Always approved' : 'Denied';
    label.textContent = labelText;
    actions.appendChild(label);
  }
}

function renderMarkdown(text) {
  if (typeof marked !== 'undefined') {
    let html = marked.parse(text);
    // Sanitize HTML output to prevent XSS from tool output or LLM responses.
    html = sanitizeRenderedHtml(html);
    // Inject copy buttons into <pre> blocks
    html = html.replace(/<pre>/g, '<pre class="code-block-wrapper"><button class="copy-btn" onclick="copyCodeBlock(this)">Copy</button>');
    return html;
  }
  return escapeHtml(text);
}

// Strip dangerous HTML elements and attributes from rendered markdown.
// This prevents XSS from tool output or prompt injection in LLM responses.
function sanitizeRenderedHtml(html) {
  html = html.replace(/<script\b[^<]*(?:(?!<\/script>)<[^<]*)*<\/script>/gi, '');
  html = html.replace(/<iframe\b[^>]*>[\s\S]*?<\/iframe>/gi, '');
  html = html.replace(/<object\b[^>]*>[\s\S]*?<\/object>/gi, '');
  html = html.replace(/<embed\b[^>]*\/?>/gi, '');
  html = html.replace(/<form\b[^>]*>[\s\S]*?<\/form>/gi, '');
  html = html.replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, '');
  html = html.replace(/<link\b[^>]*\/?>/gi, '');
  html = html.replace(/<base\b[^>]*\/?>/gi, '');
  html = html.replace(/<meta\b[^>]*\/?>/gi, '');
  // Remove event handler attributes (onclick, onerror, onload, etc.)
  html = html.replace(/\s+on\w+\s*=\s*"[^"]*"/gi, '');
  html = html.replace(/\s+on\w+\s*=\s*'[^']*'/gi, '');
  html = html.replace(/\s+on\w+\s*=\s*[^\s>]+/gi, '');
  // Remove javascript: and data: URLs in href/src attributes
  html = html.replace(/(href|src|action)\s*=\s*["']?\s*javascript\s*:/gi, '$1="');
  html = html.replace(/(href|src|action)\s*=\s*["']?\s*data\s*:/gi, '$1="');
  return html;
}

function copyCodeBlock(btn) {
  const pre = btn.parentElement;
  const code = pre.querySelector('code');
  const text = code ? code.textContent : pre.textContent;
  navigator.clipboard.writeText(text).then(() => {
    btn.textContent = 'Copied!';
    setTimeout(() => { btn.textContent = 'Copy'; }, 1500);
  });
}

function addMessage(role, content) {
  const container = document.getElementById('chat-messages');
  const div = document.createElement('div');
  div.className = 'message ' + role;
  if (role === 'user') {
    div.textContent = content;
  } else {
    div.setAttribute('data-raw', content);
    div.innerHTML = renderMarkdown(content);
  }
  container.appendChild(div);
  container.scrollTop = container.scrollHeight;
}

function appendToLastAssistant(chunk) {
  const container = document.getElementById('chat-messages');
  const messages = container.querySelectorAll('.message.assistant');
  if (messages.length > 0) {
    const last = messages[messages.length - 1];
    const raw = (last.getAttribute('data-raw') || '') + chunk;
    last.setAttribute('data-raw', raw);
    last.innerHTML = renderMarkdown(raw);
    container.scrollTop = container.scrollHeight;
  } else {
    addMessage('assistant', chunk);
  }
}

function setStatus(text, spinning) {
  const el = document.getElementById('chat-status');
  if (!text) {
    el.innerHTML = '';
    return;
  }
  el.innerHTML = (spinning ? '<div class="spinner"></div>' : '') + escapeHtml(text);
}

// --- Activity panel (per-turn collapsible thinking/tool log) ---

function ensureActivityPanel() {
  if (pendingActivityEl) return pendingActivityEl;
  const container = document.getElementById('chat-messages');
  const panel = document.createElement('div');
  panel.className = 'activity-panel';
  const toggle = document.createElement('button');
  toggle.className = 'activity-toggle';
  toggle.textContent = 'Activity (0 steps) \u25B8';
  const entries = document.createElement('div');
  entries.className = 'activity-entries';
  toggle.addEventListener('click', () => {
    const open = entries.style.display !== 'none';
    entries.style.display = open ? 'none' : 'block';
    toggle.textContent = buildActivityLabel(entries, !open);
  });
  panel.appendChild(toggle);
  panel.appendChild(entries);
  container.appendChild(panel);
  container.scrollTop = container.scrollHeight;
  pendingActivityEl = panel;
  return panel;
}

function buildActivityLabel(entries, isOpen) {
  const count = entries.querySelectorAll('.activity-entry').length;
  const totalTokens = pendingTokensIn + pendingTokensOut;
  const tokenStr = totalTokens > 0 ? formatTokenCount(totalTokens) + ' tok' : '';
  if (count === 0 && tokenStr) {
    return tokenStr + ' ' + (isOpen ? '\u25BE' : '\u25B8');
  }
  const sep = tokenStr ? ' \u00B7 ' + tokenStr : '';
  return 'Activity (' + count + ' steps' + sep + ') ' + (isOpen ? '\u25BE' : '\u25B8');
}

function formatTokenCount(n) {
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
  return String(n);
}

function updateActivityPanelLabel() {
  if (!pendingActivityEl) return;
  const entries = pendingActivityEl.querySelector('.activity-entries');
  const toggle = pendingActivityEl.querySelector('.activity-toggle');
  const isOpen = entries.style.display !== 'none';
  toggle.textContent = buildActivityLabel(entries, isOpen);
}

function addActivityEntry(type, label) {
  const panel = ensureActivityPanel();
  const entries = panel.querySelector('.activity-entries');
  const entry = document.createElement('div');
  entry.className = 'activity-entry activity-' + type;
  entry.textContent = label;
  entries.appendChild(entry);
  updateActivityPanelLabel();
  document.getElementById('chat-messages').scrollTop = document.getElementById('chat-messages').scrollHeight;
}

// Extract the most meaningful single-line param value from tool input for inline display.
function extractKeyParam(input) {
  if (!input || typeof input !== 'object') return null;
  for (const k of ['query', 'url', 'command', 'path', 'message', 'content', 'text', 'name']) {
    if (typeof input[k] === 'string' && input[k].length > 0) {
      const v = input[k];
      return v.length > 60 ? v.slice(0, 57) + '\u2026' : v;
    }
  }
  for (const v of Object.values(input)) {
    if (typeof v === 'string' && v.length > 0) {
      return v.length > 60 ? v.slice(0, 57) + '\u2026' : v;
    }
  }
  return null;
}

function addToolEntry(name, input) {
  const panel = ensureActivityPanel();
  const entries = panel.querySelector('.activity-entries');

  const details = document.createElement('details');
  details.className = 'activity-entry activity-tool';

  const summary = document.createElement('summary');
  summary.className = 'activity-tool-summary';

  const nameSpan = document.createElement('span');
  nameSpan.className = 'activity-tool-name';
  nameSpan.textContent = name;
  summary.appendChild(nameSpan);

  const snippet = extractKeyParam(input);
  if (snippet) {
    const snippetSpan = document.createElement('span');
    snippetSpan.className = 'activity-tool-snippet';
    snippetSpan.textContent = snippet;
    summary.appendChild(snippetSpan);
  }

  const statusSpan = document.createElement('span');
  statusSpan.className = 'activity-tool-status';
  statusSpan.textContent = '\u25CB'; // running indicator
  summary.appendChild(statusSpan);

  details.appendChild(summary);

  if (input && typeof input === 'object' && Object.keys(input).length > 0) {
    const pre = document.createElement('pre');
    pre.className = 'activity-tool-input';
    pre.textContent = JSON.stringify(input, null, 2);
    details.appendChild(pre);
  }

  const resultEl = document.createElement('pre');
  resultEl.className = 'activity-tool-result';
  resultEl.style.display = 'none';
  details.appendChild(resultEl);

  entries.appendChild(details);
  lastToolEntryEl = details;

  updateActivityPanelLabel();
  document.getElementById('chat-messages').scrollTop = document.getElementById('chat-messages').scrollHeight;
}

function markLastToolCompleted(success) {
  if (!lastToolEntryEl) return;
  const statusSpan = lastToolEntryEl.querySelector('.activity-tool-status');
  if (!statusSpan) return;
  statusSpan.textContent = success ? '\u2713' : '\u2717';
  statusSpan.className = 'activity-tool-status ' + (success ? 'tool-ok' : 'tool-err');
}

function updateLastToolResult(preview) {
  if (!lastToolEntryEl) return;
  const resultEl = lastToolEntryEl.querySelector('.activity-tool-result');
  if (!resultEl) return;
  const trimmed = preview && preview.length > 300 ? preview.slice(0, 297) + '\u2026' : (preview || '');
  resultEl.textContent = trimmed;
  resultEl.style.display = trimmed ? '' : 'none';
}

function sealActivityPanel() {
  if (!pendingActivityEl) return;
  const entries = pendingActivityEl.querySelector('.activity-entries');
  const count = entries.querySelectorAll('.activity-entry').length;
  const hasTokens = (pendingTokensIn + pendingTokensOut) > 0;
  if (count === 0 && !hasTokens) {
    // Nothing logged — remove the empty panel
    pendingActivityEl.remove();
  } else {
    // Collapse it (default closed after turn completes)
    entries.style.display = 'none';
    const toggle = pendingActivityEl.querySelector('.activity-toggle');
    toggle.textContent = buildActivityLabel(entries, false);
  }
  pendingActivityEl = null;
  lastToolEntryEl = null;
  pendingTokensIn = 0;
  pendingTokensOut = 0;
}

function showApproval(data) {
  const container = document.getElementById('chat-messages');
  const card = document.createElement('div');
  card.className = 'approval-card';
  card.setAttribute('data-request-id', data.request_id);

  const header = document.createElement('div');
  header.className = 'approval-header';
  header.textContent = 'Tool requires approval';
  card.appendChild(header);

  const toolName = document.createElement('div');
  toolName.className = 'approval-tool-name';
  toolName.textContent = data.tool_name;
  card.appendChild(toolName);

  if (data.description) {
    const desc = document.createElement('div');
    desc.className = 'approval-description';
    desc.textContent = data.description;
    card.appendChild(desc);
  }

  if (data.parameters) {
    const paramsToggle = document.createElement('button');
    paramsToggle.className = 'approval-params-toggle';
    paramsToggle.textContent = 'Show parameters';
    const paramsBlock = document.createElement('pre');
    paramsBlock.className = 'approval-params';
    paramsBlock.textContent = data.parameters;
    paramsBlock.style.display = 'none';
    paramsToggle.addEventListener('click', () => {
      const visible = paramsBlock.style.display !== 'none';
      paramsBlock.style.display = visible ? 'none' : 'block';
      paramsToggle.textContent = visible ? 'Show parameters' : 'Hide parameters';
    });
    card.appendChild(paramsToggle);
    card.appendChild(paramsBlock);
  }

  const actions = document.createElement('div');
  actions.className = 'approval-actions';

  const approveBtn = document.createElement('button');
  approveBtn.className = 'approve';
  approveBtn.textContent = 'Approve';
  approveBtn.addEventListener('click', () => sendApprovalAction(data.request_id, 'approve'));

  const alwaysBtn = document.createElement('button');
  alwaysBtn.className = 'always';
  alwaysBtn.textContent = 'Always';
  alwaysBtn.addEventListener('click', () => sendApprovalAction(data.request_id, 'always'));

  const denyBtn = document.createElement('button');
  denyBtn.className = 'deny';
  denyBtn.textContent = 'Deny';
  denyBtn.addEventListener('click', () => sendApprovalAction(data.request_id, 'deny'));

  actions.appendChild(approveBtn);
  actions.appendChild(alwaysBtn);
  actions.appendChild(denyBtn);
  card.appendChild(actions);

  container.appendChild(card);
  container.scrollTop = container.scrollHeight;
}

function showJobCard(data) {
  const container = document.getElementById('chat-messages');
  const card = document.createElement('div');
  card.className = 'job-card';

  const icon = document.createElement('span');
  icon.className = 'job-card-icon';
  icon.textContent = '\u2692';
  card.appendChild(icon);

  const info = document.createElement('div');
  info.className = 'job-card-info';

  const title = document.createElement('div');
  title.className = 'job-card-title';
  title.textContent = data.title || 'Sandbox Job';
  info.appendChild(title);

  const id = document.createElement('div');
  id.className = 'job-card-id';
  id.textContent = (data.job_id || '').substring(0, 8);
  info.appendChild(id);

  card.appendChild(info);

  const viewBtn = document.createElement('button');
  viewBtn.className = 'job-card-view';
  viewBtn.textContent = 'View Job';
  viewBtn.addEventListener('click', () => {
    switchTab('jobs');
    openJobDetail(data.job_id);
  });
  card.appendChild(viewBtn);

  if (data.browse_url) {
    const browseBtn = document.createElement('a');
    browseBtn.className = 'job-card-browse';
    browseBtn.href = data.browse_url;
    browseBtn.target = '_blank';
    browseBtn.textContent = 'Browse';
    card.appendChild(browseBtn);
  }

  container.appendChild(card);
  container.scrollTop = container.scrollHeight;
}

// --- Auth card ---

function showAuthCard(data) {
  // Remove any existing card for this extension first
  removeAuthCard(data.extension_name);

  const container = document.getElementById('chat-messages');
  const card = document.createElement('div');
  card.className = 'auth-card';
  card.setAttribute('data-extension-name', data.extension_name);

  const header = document.createElement('div');
  header.className = 'auth-header';
  header.textContent = 'Authentication required for ' + data.extension_name;
  card.appendChild(header);

  if (data.instructions) {
    const instr = document.createElement('div');
    instr.className = 'auth-instructions';
    instr.textContent = data.instructions;
    card.appendChild(instr);
  }

  const links = document.createElement('div');
  links.className = 'auth-links';

  if (data.auth_url) {
    const oauthBtn = document.createElement('button');
    oauthBtn.className = 'auth-oauth';
    oauthBtn.textContent = 'Authenticate with ' + data.extension_name;
    oauthBtn.addEventListener('click', () => {
      window.open(data.auth_url, '_blank', 'width=600,height=700');
    });
    links.appendChild(oauthBtn);
  }

  if (data.setup_url) {
    const setupLink = document.createElement('a');
    setupLink.href = data.setup_url;
    setupLink.target = '_blank';
    setupLink.textContent = 'Get your token';
    links.appendChild(setupLink);
  }

  if (links.children.length > 0) {
    card.appendChild(links);
  }

  // Token input
  const tokenRow = document.createElement('div');
  tokenRow.className = 'auth-token-input';

  const tokenInput = document.createElement('input');
  tokenInput.type = 'password';
  tokenInput.placeholder = 'Paste your API key or token';
  tokenInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') submitAuthToken(data.extension_name, tokenInput.value);
  });
  tokenRow.appendChild(tokenInput);
  card.appendChild(tokenRow);

  // Error display (hidden initially)
  const errorEl = document.createElement('div');
  errorEl.className = 'auth-error';
  errorEl.style.display = 'none';
  card.appendChild(errorEl);

  // Action buttons
  const actions = document.createElement('div');
  actions.className = 'auth-actions';

  const submitBtn = document.createElement('button');
  submitBtn.className = 'auth-submit';
  submitBtn.textContent = 'Submit';
  submitBtn.addEventListener('click', () => submitAuthToken(data.extension_name, tokenInput.value));

  const cancelBtn = document.createElement('button');
  cancelBtn.className = 'auth-cancel';
  cancelBtn.textContent = 'Cancel';
  cancelBtn.addEventListener('click', () => cancelAuth(data.extension_name));

  actions.appendChild(submitBtn);
  actions.appendChild(cancelBtn);
  card.appendChild(actions);

  container.appendChild(card);
  container.scrollTop = container.scrollHeight;
  tokenInput.focus();
}

function removeAuthCard(extensionName) {
  const card = document.querySelector('.auth-card[data-extension-name="' + extensionName + '"]');
  if (card) card.remove();
}

function submitAuthToken(extensionName, tokenValue) {
  if (!tokenValue || !tokenValue.trim()) return;

  // Disable submit button while in flight
  const card = document.querySelector('.auth-card[data-extension-name="' + extensionName + '"]');
  if (card) {
    const btns = card.querySelectorAll('button');
    btns.forEach((b) => { b.disabled = true; });
  }

  apiFetch('/api/chat/auth-token', {
    method: 'POST',
    body: { extension_name: extensionName, token: tokenValue.trim() },
  }).then((result) => {
    if (result.success) {
      removeAuthCard(extensionName);
      addMessage('system', result.message);
    } else {
      showAuthCardError(extensionName, result.message);
    }
  }).catch((err) => {
    showAuthCardError(extensionName, 'Failed: ' + err.message);
  });
}

function cancelAuth(extensionName) {
  apiFetch('/api/chat/auth-cancel', {
    method: 'POST',
    body: { extension_name: extensionName },
  }).catch(() => {});
  removeAuthCard(extensionName);
  enableChatInput();
}

function showAuthCardError(extensionName, message) {
  const card = document.querySelector('.auth-card[data-extension-name="' + extensionName + '"]');
  if (!card) return;
  // Re-enable buttons
  const btns = card.querySelectorAll('button');
  btns.forEach((b) => { b.disabled = false; });
  // Show error
  const errorEl = card.querySelector('.auth-error');
  if (errorEl) {
    errorEl.textContent = message;
    errorEl.style.display = 'block';
  }
}

function loadHistory(before) {
  let historyUrl = '/api/chat/history?limit=50';
  if (currentThreadId) {
    historyUrl += '&thread_id=' + encodeURIComponent(currentThreadId);
  }
  if (before) {
    historyUrl += '&before=' + encodeURIComponent(before);
  }

  const isPaginating = !!before;
  if (isPaginating) loadingOlder = true;

  apiFetch(historyUrl).then((data) => {
    const container = document.getElementById('chat-messages');

    if (!isPaginating) {
      // Fresh load: clear and render
      pendingActivityEl = null;
      container.innerHTML = '';
      for (const turn of data.turns) {
        addMessage('user', turn.user_input);
        if (turn.response) {
          addMessage('assistant', turn.response);
        }
      }
    } else {
      // Pagination: prepend older messages
      const savedHeight = container.scrollHeight;
      const fragment = document.createDocumentFragment();
      for (const turn of data.turns) {
        const userDiv = createMessageElement('user', turn.user_input);
        fragment.appendChild(userDiv);
        if (turn.response) {
          const assistantDiv = createMessageElement('assistant', turn.response);
          fragment.appendChild(assistantDiv);
        }
      }
      container.insertBefore(fragment, container.firstChild);
      // Restore scroll position so the user doesn't jump
      container.scrollTop = container.scrollHeight - savedHeight;
    }

    hasMore = data.has_more || false;
    oldestTimestamp = data.oldest_timestamp || null;
  }).catch(() => {
    // No history or no active thread
  }).finally(() => {
    loadingOlder = false;
    removeScrollSpinner();
  });
}

// Create a message DOM element without appending it (for prepend operations)
function createMessageElement(role, content) {
  const div = document.createElement('div');
  div.className = 'message ' + role;
  if (role === 'user') {
    div.textContent = content;
  } else {
    div.setAttribute('data-raw', content);
    div.innerHTML = renderMarkdown(content);
  }
  return div;
}

function removeScrollSpinner() {
  const spinner = document.getElementById('scroll-load-spinner');
  if (spinner) spinner.remove();
}

// --- Threads ---

function loadThreads() {
  apiFetch('/api/chat/threads').then((data) => {
    // Pinned assistant thread
    if (data.assistant_thread) {
      assistantThreadId = data.assistant_thread.id;
      const el = document.getElementById('assistant-thread');
      const isActive = currentThreadId === assistantThreadId;
      el.className = 'assistant-item' + (isActive ? ' active' : '');
      const meta = document.getElementById('assistant-meta');
      const count = data.assistant_thread.turn_count || 0;
      meta.textContent = count > 0 ? count + ' turns' : '';
    }

    // Regular threads
    const list = document.getElementById('thread-list');
    list.innerHTML = '';
    const threads = data.threads || [];
    for (const thread of threads) {
      const item = document.createElement('div');
      item.className = 'thread-item' + (thread.id === currentThreadId ? ' active' : '');
      const label = document.createElement('span');
      label.className = 'thread-label';
      label.textContent = thread.title || thread.id.substring(0, 8);
      label.title = thread.title ? thread.title + ' (' + thread.id + ')' : thread.id;
      item.appendChild(label);
      const meta = document.createElement('span');
      meta.className = 'thread-meta';
      meta.textContent = (thread.turn_count || 0) + ' turns';
      item.appendChild(meta);
      item.addEventListener('click', () => switchThread(thread.id));
      list.appendChild(item);
    }

    // Default to assistant thread on first load if no thread selected
    if (!currentThreadId && assistantThreadId) {
      switchToAssistant();
    }
  }).catch(() => {});
}

function switchToAssistant() {
  if (!assistantThreadId) return;
  pendingActivityEl = null;
  currentThreadId = assistantThreadId;
  hasMore = false;
  oldestTimestamp = null;
  convTokensIn = 0;
  convTokensOut = 0;
  renderConversationTokenStats();
  loadHistory();
  loadThreads();
  loadConversationTokenStats(assistantThreadId);
}

function switchThread(threadId) {
  pendingActivityEl = null;
  currentThreadId = threadId;
  hasMore = false;
  oldestTimestamp = null;
  convTokensIn = 0;
  convTokensOut = 0;
  renderConversationTokenStats();
  loadHistory();
  loadThreads();
  if (threadId) loadConversationTokenStats(threadId);
}

function loadConversationTokenStats(threadId) {
  apiFetch('/api/chat/threads/' + encodeURIComponent(threadId) + '/tokens')
    .then((data) => {
      // Only apply if still viewing the same thread
      if (currentThreadId !== threadId) return;
      convTokensIn = data.total_input_tokens || 0;
      convTokensOut = data.total_output_tokens || 0;
      renderConversationTokenStats();
    })
    .catch(() => {}); // best-effort, silently ignore
}

function renderConversationTokenStats() {
  const el = document.getElementById('chat-token-stats');
  if (!el) return;
  const total = convTokensIn + convTokensOut;
  if (total === 0) {
    el.style.display = 'none';
    return;
  }
  el.style.display = '';
  el.textContent =
    formatTokenCount(convTokensIn) + ' in \u00B7 ' +
    formatTokenCount(convTokensOut) + ' out \u00B7 ' +
    formatTokenCount(total) + ' total';
}

function createNewThread() {
  apiFetch('/api/chat/thread/new', { method: 'POST' }).then((data) => {
    pendingActivityEl = null;
    currentThreadId = data.id || null;
    document.getElementById('chat-messages').innerHTML = '';
    setStatus('');
    loadThreads();
  }).catch((err) => {
    showToast('Failed to create thread: ' + err.message, 'error');
  });
}

function toggleThreadSidebar() {
  const sidebar = document.getElementById('thread-sidebar');
  sidebar.classList.toggle('collapsed');
  const btn = document.getElementById('thread-toggle-btn');
  btn.innerHTML = sidebar.classList.contains('collapsed') ? '&raquo;' : '&laquo;';
}

// Chat input auto-resize and keyboard handling
const chatInput = document.getElementById('chat-input');
chatInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    sendMessage();
  }
});
chatInput.addEventListener('input', () => autoResizeTextarea(chatInput));

// Infinite scroll: load older messages when scrolled near the top
document.getElementById('chat-messages').addEventListener('scroll', function () {
  if (this.scrollTop < 100 && hasMore && !loadingOlder) {
    loadingOlder = true;
    // Show spinner at top
    const spinner = document.createElement('div');
    spinner.id = 'scroll-load-spinner';
    spinner.className = 'scroll-load-spinner';
    spinner.innerHTML = '<div class="spinner"></div> Loading older messages...';
    this.insertBefore(spinner, this.firstChild);
    loadHistory(oldestTimestamp);
  }
});

function autoResizeTextarea(el) {
  el.style.height = 'auto';
  el.style.height = Math.min(el.scrollHeight, 120) + 'px';
}

// --- Tabs ---

document.querySelectorAll('.tab-bar button[data-tab]').forEach((btn) => {
  btn.addEventListener('click', () => {
    const tab = btn.getAttribute('data-tab');
    switchTab(tab);
  });
});

function switchTab(tab) {
  currentTab = tab;
  document.querySelectorAll('.tab-bar button[data-tab]').forEach((b) => {
    b.classList.toggle('active', b.getAttribute('data-tab') === tab);
  });
  document.querySelectorAll('.tab-panel').forEach((p) => {
    p.classList.toggle('active', p.id === 'tab-' + tab);
  });

  if (tab === 'memory') loadMemoryTree();
  if (tab === 'jobs') loadJobs();
  if (tab === 'routines') loadRoutines();
  if (tab === 'logs') applyLogFilters();
  if (tab === 'skills') loadSkills();
  if (tab === 'extensions') {
    initExtSubTabs();
    initCatalogSearch();
    checkExtensionManagerAvailable().then(() => loadCatalog());
  }
  if (tab === 'channels') loadChannels();
  if (tab === 'analytics') loadAnalytics();
}

// --- Skills ---

let editingSkillName = null;

function loadSkills() {
  const grid = document.getElementById('skills-grid');
  grid.innerHTML = '<div class="empty-state">Loading...</div>';
  apiFetch('/api/skills')
    .then((data) => renderSkillsGrid(data.skills || []))
    .catch(() => { grid.innerHTML = '<div class="empty-state">Failed to load skills</div>'; });
}

function renderSkillsGrid(skills) {
  const grid = document.getElementById('skills-grid');
  if (skills.length === 0) {
    grid.innerHTML = '<div class="skills-empty"><div class="skills-empty-icon">&#128214;</div><p>No skills yet.</p><p class="skills-empty-hint">Create a skill to invoke a reusable prompt with <code>/name</code> in chat.</p></div>';
    return;
  }
  grid.innerHTML = '';
  for (const skill of skills) {
    grid.appendChild(renderSkillCard(skill));
  }
}

function renderSkillCard(skill) {
  const card = document.createElement('div');
  card.className = 'skill-card';

  const header = document.createElement('div');
  header.className = 'skill-card-header';

  const nameEl = document.createElement('div');
  nameEl.className = 'skill-card-name';
  nameEl.textContent = '/' + skill.name;

  const badge = document.createElement('span');
  badge.className = 'skill-card-badge';
  badge.textContent = 'Skill';
  header.append(nameEl, badge);

  const desc = document.createElement('div');
  desc.className = 'skill-card-desc';
  desc.textContent = skill.description || 'No description';

  const preview = document.createElement('div');
  preview.className = 'skill-card-prompt';
  const previewText = skill.prompt || '';
  preview.textContent = previewText.length > 120 ? previewText.slice(0, 120) + '...' : previewText;

  const actions = document.createElement('div');
  actions.className = 'skill-card-actions';

  const runBtn = document.createElement('button');
  runBtn.className = 'skill-card-run-btn';
  runBtn.textContent = 'Run';
  runBtn.title = 'Switch to chat and pre-fill /' + skill.name;
  runBtn.addEventListener('click', () => runSkill(skill.name));

  const editBtn = document.createElement('button');
  editBtn.className = 'skill-card-edit-btn';
  editBtn.textContent = 'Edit';
  editBtn.addEventListener('click', () => openSkillModal(skill));

  const deleteBtn = document.createElement('button');
  deleteBtn.className = 'skill-card-delete-btn';
  deleteBtn.textContent = 'Delete';
  deleteBtn.addEventListener('click', () => deleteSkill(skill.name));

  actions.append(runBtn, editBtn, deleteBtn);
  card.append(header, desc, preview, actions);
  return card;
}

function openSkillModal(skill) {
  skill = skill || null;
  editingSkillName = skill ? skill.name : null;
  document.getElementById('skill-modal-title').textContent = skill ? 'Edit Skill' : 'New Skill';
  const nameInput = document.getElementById('skill-name-input');
  nameInput.value = skill ? skill.name : '';
  // Prevent renaming (name is the workspace path key)
  nameInput.disabled = !!skill;
  document.getElementById('skill-desc-input').value = skill ? (skill.description || '') : '';
  document.getElementById('skill-prompt-input').value = skill ? (skill.prompt || '') : '';
  document.getElementById('skill-save-btn').disabled = false;
  document.getElementById('skill-save-btn').textContent = 'Save';
  document.getElementById('skill-modal-overlay').style.display = 'flex';
  if (!skill) nameInput.focus();
  else document.getElementById('skill-prompt-input').focus();
}

function closeSkillModal() {
  document.getElementById('skill-modal-overlay').style.display = 'none';
  editingSkillName = null;
}

function skillModalOverlayClick(e) {
  if (e.target === document.getElementById('skill-modal-overlay')) closeSkillModal();
}

function saveSkill() {
  let name = document.getElementById('skill-name-input').value.trim().toLowerCase().replace(/\s+/g, '-');
  const description = document.getElementById('skill-desc-input').value.trim();
  const prompt = document.getElementById('skill-prompt-input').value.trim();

  if (!name) { showToast('Skill name is required', 'error'); return; }
  if (!/^[a-z0-9-]+$/.test(name)) { showToast('Name must use only lowercase letters, digits, and hyphens', 'error'); return; }
  if (!prompt) { showToast('Prompt is required', 'error'); return; }

  const saveBtn = document.getElementById('skill-save-btn');
  saveBtn.disabled = true;
  saveBtn.textContent = 'Saving...';

  apiFetch('/api/skills', { method: 'POST', body: { name, description, prompt } })
    .then(() => {
      closeSkillModal();
      loadSkills();
      showToast('Skill saved', 'success');
    })
    .catch((err) => {
      showToast('Failed to save skill: ' + err.message, 'error');
      saveBtn.disabled = false;
      saveBtn.textContent = 'Save';
    });
}

function deleteSkill(name) {
  if (!confirm('Delete skill /' + name + '?')) return;
  apiFetch('/api/skills/' + encodeURIComponent(name), { method: 'DELETE' })
    .then(() => {
      loadSkills();
      showToast('Skill deleted', 'success');
    })
    .catch((err) => showToast('Delete failed: ' + err.message, 'error'));
}

function runSkill(name) {
  switchTab('chat');
  const input = document.getElementById('chat-input');
  input.value = '/' + name + ' ';
  input.focus();
  autoResizeTextarea(input);
  // Place cursor at end so user can append args naturally
  input.setSelectionRange(input.value.length, input.value.length);
}

// --- Custom extension install ---

function openCustomInstallModal() {
  document.getElementById('custom-ext-name').value = '';
  document.getElementById('custom-ext-url').value = '';
  document.getElementById('custom-ext-kind').value = '';
  document.getElementById('custom-install-btn').disabled = false;
  document.getElementById('custom-install-btn').textContent = 'Install';
  document.getElementById('custom-install-overlay').style.display = 'flex';
  document.getElementById('custom-ext-name').focus();
}

function closeCustomInstallModal() {
  document.getElementById('custom-install-overlay').style.display = 'none';
}

function customInstallOverlayClick(e) {
  if (e.target === document.getElementById('custom-install-overlay')) closeCustomInstallModal();
}

function installCustomExtension() {
  const name = document.getElementById('custom-ext-name').value.trim();
  const url = document.getElementById('custom-ext-url').value.trim();
  const kind = document.getElementById('custom-ext-kind').value || undefined;

  if (!name) { showToast('Name is required', 'error'); return; }
  if (!url) { showToast('URL is required', 'error'); return; }

  const btn = document.getElementById('custom-install-btn');
  btn.disabled = true;
  btn.textContent = 'Installing...';

  apiFetch('/api/extensions/install', { method: 'POST', body: { name, url, kind } })
    .then((res) => {
      closeCustomInstallModal();
      if (res.success) {
        showToast(res.message || 'Extension installed', 'success');
        // Switch to installed tab to show the result
        document.querySelectorAll('.ext-sub-tab').forEach((b) => b.classList.remove('active'));
        document.querySelectorAll('.ext-panel').forEach((p) => p.classList.remove('active'));
        document.querySelector('.ext-sub-tab[data-subtab="installed"]').classList.add('active');
        document.getElementById('ext-panel-installed').classList.add('active');
        loadInstalledExtensions();
      } else {
        showToast(res.message || 'Install failed', 'error');
      }
    })
    .catch((err) => {
      showToast('Install failed: ' + err.message, 'error');
      btn.disabled = false;
      btn.textContent = 'Install';
    });
}

// --- Memory (filesystem tree) ---

let memorySearchTimeout = null;
let currentMemoryPath = null;
let currentMemoryContent = null;
// Tree state: nested nodes persisted across renders
// { name, path, is_dir, children: [] | null, expanded: bool, loaded: bool }
let memoryTreeState = null;

document.getElementById('memory-search').addEventListener('input', (e) => {
  clearTimeout(memorySearchTimeout);
  const query = e.target.value.trim();
  if (!query) {
    loadMemoryTree();
    return;
  }
  memorySearchTimeout = setTimeout(() => searchMemory(query), 300);
});

function loadMemoryTree() {
  // Only load top-level on first load (or refresh)
  apiFetch('/api/memory/list?path=').then((data) => {
    memoryTreeState = data.entries.map((e) => ({
      name: e.name,
      path: e.path,
      is_dir: e.is_dir,
      children: e.is_dir ? null : undefined,
      expanded: false,
      loaded: false,
    }));
    renderTree();
  }).catch(() => {});
}

function renderTree() {
  const container = document.getElementById('memory-tree');
  container.innerHTML = '';
  if (!memoryTreeState || memoryTreeState.length === 0) {
    container.innerHTML = '<div class="tree-item" style="color:var(--text-secondary)">No files in workspace</div>';
    return;
  }
  renderNodes(memoryTreeState, container, 0);
}

function renderNodes(nodes, container, depth) {
  for (const node of nodes) {
    const row = document.createElement('div');
    row.className = 'tree-row';
    row.style.paddingLeft = (depth * 16 + 8) + 'px';

    if (node.is_dir) {
      const arrow = document.createElement('span');
      arrow.className = 'expand-arrow' + (node.expanded ? ' expanded' : '');
      arrow.textContent = '\u25B6';
      arrow.addEventListener('click', (e) => {
        e.stopPropagation();
        toggleExpand(node);
      });
      row.appendChild(arrow);

      const label = document.createElement('span');
      label.className = 'tree-label dir';
      label.textContent = node.name;
      label.addEventListener('click', () => toggleExpand(node));
      row.appendChild(label);
    } else {
      const spacer = document.createElement('span');
      spacer.className = 'expand-arrow-spacer';
      row.appendChild(spacer);

      const label = document.createElement('span');
      label.className = 'tree-label file';
      label.textContent = node.name;
      label.addEventListener('click', () => readMemoryFile(node.path));
      row.appendChild(label);
    }

    container.appendChild(row);

    if (node.is_dir && node.expanded && node.children) {
      const childContainer = document.createElement('div');
      childContainer.className = 'tree-children';
      renderNodes(node.children, childContainer, depth + 1);
      container.appendChild(childContainer);
    }
  }
}

function toggleExpand(node) {
  if (node.expanded) {
    node.expanded = false;
    renderTree();
    return;
  }

  if (node.loaded) {
    node.expanded = true;
    renderTree();
    return;
  }

  // Lazy-load children
  apiFetch('/api/memory/list?path=' + encodeURIComponent(node.path)).then((data) => {
    node.children = data.entries.map((e) => ({
      name: e.name,
      path: e.path,
      is_dir: e.is_dir,
      children: e.is_dir ? null : undefined,
      expanded: false,
      loaded: false,
    }));
    node.loaded = true;
    node.expanded = true;
    renderTree();
  }).catch(() => {});
}

function readMemoryFile(path) {
  currentMemoryPath = path;
  // Update breadcrumb
  document.getElementById('memory-breadcrumb-path').innerHTML = buildBreadcrumb(path);
  document.getElementById('memory-edit-btn').style.display = 'inline-block';

  // Exit edit mode if active
  cancelMemoryEdit();

  apiFetch('/api/memory/read?path=' + encodeURIComponent(path)).then((data) => {
    currentMemoryContent = data.content;
    const viewer = document.getElementById('memory-viewer');
    // Render markdown if it's a .md file
    if (path.endsWith('.md')) {
      viewer.innerHTML = '<div class="memory-rendered">' + renderMarkdown(data.content) + '</div>';
      viewer.classList.add('rendered');
    } else {
      viewer.textContent = data.content;
      viewer.classList.remove('rendered');
    }
  }).catch((err) => {
    currentMemoryContent = null;
    document.getElementById('memory-viewer').innerHTML = '<div class="empty">Error: ' + escapeHtml(err.message) + '</div>';
  });
}

function startMemoryEdit() {
  if (!currentMemoryPath || currentMemoryContent === null) return;
  document.getElementById('memory-viewer').style.display = 'none';
  const editor = document.getElementById('memory-editor');
  editor.style.display = 'flex';
  const textarea = document.getElementById('memory-edit-textarea');
  textarea.value = currentMemoryContent;
  textarea.focus();
}

function cancelMemoryEdit() {
  document.getElementById('memory-viewer').style.display = '';
  document.getElementById('memory-editor').style.display = 'none';
}

function saveMemoryEdit() {
  if (!currentMemoryPath) return;
  const content = document.getElementById('memory-edit-textarea').value;
  apiFetch('/api/memory/write', {
    method: 'POST',
    body: { path: currentMemoryPath, content: content },
  }).then(() => {
    showToast('Saved ' + currentMemoryPath, 'success');
    cancelMemoryEdit();
    readMemoryFile(currentMemoryPath);
  }).catch((err) => {
    showToast('Save failed: ' + err.message, 'error');
  });
}

function buildBreadcrumb(path) {
  const parts = path.split('/');
  let html = '<a onclick="loadMemoryTree()">workspace</a>';
  let current = '';
  for (const part of parts) {
    current += (current ? '/' : '') + part;
    html += ' / <a onclick="readMemoryFile(\'' + escapeHtml(current) + '\')">' + escapeHtml(part) + '</a>';
  }
  return html;
}

function searchMemory(query) {
  apiFetch('/api/memory/search', {
    method: 'POST',
    body: { query, limit: 20 },
  }).then((data) => {
    const tree = document.getElementById('memory-tree');
    tree.innerHTML = '';
    if (data.results.length === 0) {
      tree.innerHTML = '<div class="tree-item" style="color:var(--text-secondary)">No results</div>';
      return;
    }
    for (const result of data.results) {
      const item = document.createElement('div');
      item.className = 'search-result';
      const snippet = snippetAround(result.content, query, 120);
      item.innerHTML = '<div class="path">' + escapeHtml(result.path) + '</div>'
        + '<div class="snippet">' + highlightQuery(snippet, query) + '</div>';
      item.addEventListener('click', () => readMemoryFile(result.path));
      tree.appendChild(item);
    }
  }).catch(() => {});
}

function snippetAround(text, query, len) {
  const lower = text.toLowerCase();
  const idx = lower.indexOf(query.toLowerCase());
  if (idx < 0) return text.substring(0, len);
  const start = Math.max(0, idx - Math.floor(len / 2));
  const end = Math.min(text.length, start + len);
  let s = text.substring(start, end);
  if (start > 0) s = '...' + s;
  if (end < text.length) s = s + '...';
  return s;
}

function highlightQuery(text, query) {
  if (!query) return escapeHtml(text);
  const escaped = escapeHtml(text);
  const queryEscaped = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp('(' + queryEscaped + ')', 'gi');
  return escaped.replace(re, '<mark>$1</mark>');
}

// --- Logs ---

const LOG_MAX_ENTRIES = 2000;
let logsPaused = false;
let logBuffer = []; // buffer while paused

function connectLogSSE() {
  if (logEventSource) logEventSource.close();

  logEventSource = new EventSource('/api/logs/events?token=' + encodeURIComponent(token));

  logEventSource.addEventListener('log', (e) => {
    const entry = JSON.parse(e.data);
    if (logsPaused) {
      logBuffer.push(entry);
      return;
    }
    appendLogEntry(entry);
  });

  logEventSource.onerror = () => {
    // Silent reconnect
  };
}

function appendLogEntry(entry) {
  const output = document.getElementById('logs-output');

  // Level filter
  const levelFilter = document.getElementById('logs-level-filter').value;
  const targetFilter = document.getElementById('logs-target-filter').value.trim().toLowerCase();

  const div = document.createElement('div');
  div.className = 'log-entry level-' + entry.level;
  div.setAttribute('data-level', entry.level);
  div.setAttribute('data-target', entry.target);

  const ts = document.createElement('span');
  ts.className = 'log-ts';
  ts.textContent = entry.timestamp.substring(11, 23);
  div.appendChild(ts);

  const lvl = document.createElement('span');
  lvl.className = 'log-level';
  lvl.textContent = entry.level.padEnd(5);
  div.appendChild(lvl);

  const tgt = document.createElement('span');
  tgt.className = 'log-target';
  tgt.textContent = entry.target;
  div.appendChild(tgt);

  const msg = document.createElement('span');
  msg.className = 'log-msg';
  msg.textContent = entry.message;
  div.appendChild(msg);

  div.addEventListener('click', () => div.classList.toggle('expanded'));

  // Apply current filters as visibility
  const matchesLevel = levelFilter === 'all' || entry.level === levelFilter;
  const matchesTarget = !targetFilter || entry.target.toLowerCase().includes(targetFilter);
  if (!matchesLevel || !matchesTarget) {
    div.style.display = 'none';
  }

  output.appendChild(div);

  // Cap entries
  while (output.children.length > LOG_MAX_ENTRIES) {
    output.removeChild(output.firstChild);
  }

  // Auto-scroll
  if (document.getElementById('logs-autoscroll').checked) {
    output.scrollTop = output.scrollHeight;
  }
}

function toggleLogsPause() {
  logsPaused = !logsPaused;
  const btn = document.getElementById('logs-pause-btn');
  btn.textContent = logsPaused ? 'Resume' : 'Pause';

  if (!logsPaused) {
    // Flush buffer
    for (const entry of logBuffer) {
      appendLogEntry(entry);
    }
    logBuffer = [];
  }
}

function clearLogs() {
  if (!confirm('Clear all logs?')) return;
  document.getElementById('logs-output').innerHTML = '';
  logBuffer = [];
}

// Re-apply filters when level or target changes
document.getElementById('logs-level-filter').addEventListener('change', applyLogFilters);
document.getElementById('logs-target-filter').addEventListener('input', applyLogFilters);

function applyLogFilters() {
  const levelFilter = document.getElementById('logs-level-filter').value;
  const targetFilter = document.getElementById('logs-target-filter').value.trim().toLowerCase();
  const entries = document.querySelectorAll('#logs-output .log-entry');
  for (const el of entries) {
    const matchesLevel = levelFilter === 'all' || el.getAttribute('data-level') === levelFilter;
    const matchesTarget = !targetFilter || el.getAttribute('data-target').toLowerCase().includes(targetFilter);
    el.style.display = (matchesLevel && matchesTarget) ? '' : 'none';
  }
}

// --- Extensions ---

let extSubTabsInit = false;
let catalogSearchInit = false;
let extManagerAvailable = true; // assume available until status check says otherwise
let catalogKindFilter = 'all';
let wizardState = null;
let catalogAllEntries = [];
let catalogPage = 0;
const CATALOG_PAGE_SIZE = 12;

async function checkExtensionManagerAvailable() {
  try {
    const data = await apiFetch('/api/gateway/status');
    extManagerAvailable = data.extension_manager_available !== false;
    const banner = document.getElementById('ext-setup-banner');
    if (banner) {
      banner.style.display = extManagerAvailable ? 'none' : 'flex';
    }
  } catch (_) {
    // If status check fails, leave banner hidden and assume available
  }
}

function initExtSubTabs() {
  if (extSubTabsInit) return;
  extSubTabsInit = true;

  document.querySelectorAll('.ext-sub-tab').forEach((btn) => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.ext-sub-tab').forEach((b) => b.classList.remove('active'));
      document.querySelectorAll('.ext-panel').forEach((p) => p.classList.remove('active'));
      btn.classList.add('active');
      const sub = btn.dataset.subtab;
      document.getElementById('ext-panel-' + sub).classList.add('active');
      if (sub === 'catalog') loadCatalog();
      if (sub === 'installed') loadInstalledExtensions();
      if (sub === 'tools') loadExtensionTools();
    });
  });

  const closeBtn = document.getElementById('wizard-close-btn');
  if (closeBtn) closeBtn.addEventListener('click', closeWizard);

  document.getElementById('wizard-overlay').addEventListener('click', (e) => {
    if (e.target === document.getElementById('wizard-overlay')) closeWizard();
  });
}

function initCatalogSearch() {
  if (catalogSearchInit) return;
  catalogSearchInit = true;

  const input = document.getElementById('ext-catalog-search');
  let timer;
  input.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      const q = input.value.trim();
      if (!q) { loadCatalog(); return; }
      const body = { query: q, kind: catalogKindFilter !== 'all' ? catalogKindFilter : undefined };
      apiFetch('/api/extensions/catalog/search', { method: 'POST', body })
        .then((data) => { catalogPage = 0; renderCatalogGrid(data.entries || []); })
        .catch(() => {});
    }, 300);
  });

  document.querySelectorAll('.ext-kind-filter').forEach((btn) => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.ext-kind-filter').forEach((b) => b.classList.remove('active'));
      btn.classList.add('active');
      catalogKindFilter = btn.dataset.kind;
      document.getElementById('ext-catalog-search').value = '';
      loadCatalog();
    });
  });
}

function loadCatalog() {
  const grid = document.getElementById('catalog-grid');
  grid.innerHTML = '<div class="empty-state">Loading...</div>';
  const qs = catalogKindFilter !== 'all' ? '?kind=' + catalogKindFilter : '';
  apiFetch('/api/extensions/catalog' + qs)
    .then((data) => {
      catalogPage = 0;
      renderCatalogGrid(data.entries || []);
    })
    .catch(() => { grid.innerHTML = '<div class="empty-state">Failed to load catalog</div>'; });
}

function renderCatalogGrid(entries) {
  // Channels are managed in the Channels tab — exclude them from Extensions.
  catalogAllEntries = entries.filter((e) => e.kind !== 'wasm_channel');
  renderCatalogPage();
}

function renderCatalogPage() {
  const grid = document.getElementById('catalog-grid');
  const pagination = document.getElementById('catalog-pagination');

  if (catalogAllEntries.length === 0) {
    grid.innerHTML = '<div class="empty-state">No extensions found</div>';
    if (pagination) pagination.innerHTML = '';
    return;
  }

  const totalPages = Math.ceil(catalogAllEntries.length / CATALOG_PAGE_SIZE);
  const start = catalogPage * CATALOG_PAGE_SIZE;
  const page = catalogAllEntries.slice(start, start + CATALOG_PAGE_SIZE);

  grid.innerHTML = '';
  for (const entry of page) {
    grid.appendChild(renderCatalogCard(entry));
  }

  if (pagination) {
    pagination.innerHTML = '';
    if (totalPages <= 1) return;

    const prevBtn = document.createElement('button');
    prevBtn.className = 'catalog-page-btn';
    prevBtn.textContent = '\u2190 Prev';
    prevBtn.disabled = catalogPage === 0;
    prevBtn.addEventListener('click', () => { catalogPage--; renderCatalogPage(); });
    pagination.appendChild(prevBtn);

    const info = document.createElement('span');
    info.className = 'catalog-page-info';
    info.textContent = (catalogPage + 1) + ' / ' + totalPages;
    pagination.appendChild(info);

    const nextBtn = document.createElement('button');
    nextBtn.className = 'catalog-page-btn';
    nextBtn.textContent = 'Next \u2192';
    nextBtn.disabled = catalogPage >= totalPages - 1;
    nextBtn.addEventListener('click', () => { catalogPage++; renderCatalogPage(); });
    pagination.appendChild(nextBtn);
  }
}

function renderCatalogCard(entry) {
  const card = document.createElement('div');
  card.className = 'catalog-card';

  const header = document.createElement('div');
  header.className = 'catalog-card-header';

  const nameEl = document.createElement('span');
  nameEl.className = 'catalog-card-name';
  nameEl.textContent = entry.display_name || entry.name;
  header.appendChild(nameEl);

  const kindBadge = document.createElement('span');
  kindBadge.className = 'ext-kind kind-' + entry.kind;
  kindBadge.textContent = kindLabel(entry.kind);
  header.appendChild(kindBadge);
  card.appendChild(header);

  const desc = document.createElement('div');
  desc.className = 'catalog-card-desc';
  desc.textContent = entry.description || '';
  card.appendChild(desc);

  const footer = document.createElement('div');
  footer.className = 'catalog-card-footer';

  if (entry.installed && entry.status !== 'not_installed') {
    const badge = document.createElement('span');
    badge.className = 'catalog-status-badge status-' + (entry.status || '').replace('_', '-');
    badge.textContent = statusLabel(entry.status);
    footer.appendChild(badge);

    if (entry.status !== 'active') {
      const manageBtn = document.createElement('button');
      manageBtn.className = 'catalog-card-manage-btn';
      manageBtn.textContent = entry.status === 'needs_auth' ? 'Set Up' : 'Manage';
      manageBtn.addEventListener('click', () => openWizard(entry));
      footer.appendChild(manageBtn);
    }
  } else if (entry.installable === false) {
    const badge = document.createElement('span');
    badge.className = 'catalog-status-badge status-build-required';
    badge.textContent = 'Setup guide \u2192';
    badge.title = 'Click to view setup instructions';
    badge.addEventListener('click', () => openSetupGuide(entry));
    footer.appendChild(badge);
  } else {
    const installBtn = document.createElement('button');
    installBtn.className = 'catalog-card-install-btn';
    installBtn.textContent = 'Install';
    if (!extManagerAvailable) {
      installBtn.disabled = true;
      installBtn.title = 'Set SECRETS_MASTER_KEY in .env to enable installation';
    } else {
      installBtn.addEventListener('click', () => openWizard(entry));
    }
    footer.appendChild(installBtn);
  }

  card.appendChild(footer);
  return card;
}

function kindLabel(kind) {
  return { mcp_server: 'MCP', wasm_tool: 'WASM', wasm_channel: 'Channel' }[kind] || kind;
}

function statusLabel(status) {
  return {
    active: 'Active',
    needs_auth: 'Needs Auth',
    inactive: 'Inactive',
    error: 'Error',
    not_installed: 'Not Installed',
  }[status] || status;
}

// --- Installed extensions list ---

function loadInstalledExtensions() {
  const extList = document.getElementById('extensions-list');
  extList.innerHTML = '<div class="empty-state">Loading...</div>';
  apiFetch('/api/extensions')
    .then((data) => {
      // Channels are managed in the Channels tab — exclude them here.
      const extensions = (data.extensions || []).filter((e) => e.kind !== 'wasm_channel');
      if (extensions.length === 0) {
        extList.innerHTML = '<div class="empty-state">No extensions installed</div>';
      } else {
        extList.innerHTML = '';
        for (const ext of extensions) {
          extList.appendChild(renderExtensionCard(ext));
        }
      }
    })
    .catch(() => { extList.innerHTML = '<div class="empty-state">Failed to load extensions</div>'; });
}

function loadExtensionTools() {
  const toolsTbody = document.getElementById('tools-tbody');
  const toolsEmpty = document.getElementById('tools-empty');
  apiFetch('/api/extensions/tools')
    .then((data) => {
      if (!data.tools || data.tools.length === 0) {
        toolsTbody.innerHTML = '';
        toolsEmpty.style.display = 'block';
      } else {
        toolsEmpty.style.display = 'none';
        toolsTbody.innerHTML = data.tools.map((t) =>
          '<tr><td>' + escapeHtml(t.name) + '</td><td>' + escapeHtml(t.description) + '</td></tr>'
        ).join('');
      }
    })
    .catch(() => {});
}

function renderExtensionCard(ext) {
  const card = document.createElement('div');
  card.className = 'ext-card';

  const header = document.createElement('div');
  header.className = 'ext-header';

  const name = document.createElement('span');
  name.className = 'ext-name';
  name.textContent = ext.name;
  header.appendChild(name);

  const kind = document.createElement('span');
  kind.className = 'ext-kind kind-' + ext.kind;
  kind.textContent = kindLabel(ext.kind);
  header.appendChild(kind);

  const statusClass = { active: 'authed', needs_auth: 'unauthed', error: 'unauthed', inactive: 'unauthed' }[ext.status] || 'unauthed';
  const authDot = document.createElement('span');
  authDot.className = 'ext-auth-dot ' + statusClass;
  authDot.title = statusLabel(ext.status || (ext.authenticated ? 'active' : 'needs_auth'));
  header.appendChild(authDot);

  // Gear button in the header, right-aligned — keeps it away from action buttons
  const configBtn = document.createElement('button');
  configBtn.type = 'button';
  configBtn.className = 'btn-ext-config-header';
  configBtn.title = 'Configure';
  configBtn.textContent = '\u2699'; // ⚙
  configBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    toggleExtensionConfig(ext.name, card);
  });
  header.appendChild(configBtn);

  card.appendChild(header);

  if (ext.description) {
    const desc = document.createElement('div');
    desc.className = 'ext-desc';
    desc.textContent = ext.description;
    card.appendChild(desc);
  }

  if (ext.error) {
    const errEl = document.createElement('div');
    errEl.className = 'ext-desc';
    errEl.style.color = 'var(--danger)';
    errEl.textContent = ext.error;
    card.appendChild(errEl);
  }

  if (ext.url) {
    const url = document.createElement('div');
    url.className = 'ext-url';
    url.textContent = ext.url;
    url.title = ext.url;
    card.appendChild(url);
  }

  if (ext.tools && ext.tools.length > 0) {
    const tools = document.createElement('div');
    tools.className = 'ext-tools';
    tools.textContent = 'Tools: ' + ext.tools.join(', ');
    card.appendChild(tools);
  }

  const actions = document.createElement('div');
  actions.className = 'ext-actions';

  if (ext.status === 'needs_auth' || ext.status === 'error') {
    const setupBtn = document.createElement('button');
    setupBtn.className = 'btn-ext activate';
    setupBtn.textContent = 'Set Up';
    setupBtn.addEventListener('click', () => openWizard(ext));
    actions.appendChild(setupBtn);
  } else if (!ext.active) {
    const activateBtn = document.createElement('button');
    activateBtn.className = 'btn-ext activate';
    activateBtn.textContent = 'Activate';
    activateBtn.addEventListener('click', () => activateExtension(ext.name));
    actions.appendChild(activateBtn);
  } else {
    const activeLabel = document.createElement('span');
    activeLabel.className = 'ext-active-label';
    activeLabel.textContent = 'Active';
    actions.appendChild(activeLabel);
  }

  const removeBtn = document.createElement('button');
  removeBtn.className = 'btn-ext remove';
  removeBtn.textContent = 'Remove';
  removeBtn.addEventListener('click', () => removeExtension(ext.name));
  actions.appendChild(removeBtn);

  card.appendChild(actions);
  return card;
}

function activateExtension(name) {
  apiFetch('/api/extensions/' + encodeURIComponent(name) + '/activate', { method: 'POST' })
    .then((res) => {
      if (res.success) {
        loadInstalledExtensions();
        if (document.getElementById('ext-panel-catalog').classList.contains('active')) {
          loadCatalog();
        }
        return;
      }
      if (res.auth_url) {
        showToast('Opening authentication for ' + name, 'info');
        window.open(res.auth_url, '_blank');
      } else if (res.awaiting_token) {
        showToast(res.instructions || 'Please provide an API token for ' + name, 'info');
      } else {
        showToast('Activate failed: ' + res.message, 'error');
      }
      loadInstalledExtensions();
    })
    .catch((err) => showToast('Activate failed: ' + err.message, 'error'));
}

function removeExtension(name) {
  if (!confirm('Remove extension "' + name + '"?')) return;
  apiFetch('/api/extensions/' + encodeURIComponent(name) + '/remove', { method: 'POST' })
    .then((res) => {
      if (!res.success) {
        showToast('Remove failed: ' + res.message, 'error');
      } else {
        showToast('Removed ' + name, 'success');
      }
      loadInstalledExtensions();
      loadCatalog();
    })
    .catch((err) => showToast('Remove failed: ' + err.message, 'error'));
}

// --- Channels tab ---

function loadChannels() {
  const list = document.getElementById('channels-list');
  list.innerHTML = '<div class="empty-state">Loading...</div>';

  Promise.all([
    apiFetch('/api/channels'),
    apiFetch('/api/extensions/catalog?kind=wasm_channel').catch(() => ({ entries: [] })),
  ]).then(([runningData, catalogData]) => {
    const running = new Map((runningData.channels || []).map((ch) => [ch.name, ch]));
    const catalog = (catalogData.entries || []);

    // Merge: catalog is the source of truth for what exists; running overrides status.
    const seen = new Set();
    const cards = [];

    for (const entry of catalog) {
      seen.add(entry.name);
      const liveChannel = running.get(entry.name);
      cards.push(renderChannelCard(entry, liveChannel || null));
    }

    // Any running channels not in the catalog (e.g. custom ones).
    for (const ch of (runningData.channels || [])) {
      if (!seen.has(ch.name)) {
        cards.push(renderChannelCard({ name: ch.name, description: ch.description, kind: 'wasm_channel' }, ch));
      }
    }

    list.innerHTML = '';
    if (cards.length === 0) {
      list.innerHTML = '<div class="empty-state">No channels available.</div>';
    } else {
      for (const card of cards) list.appendChild(card);
    }
  }).catch(() => {
    list.innerHTML = '<div class="empty-state">Failed to load channels.</div>';
  });
}

// entry = catalog entry (or minimal object for custom channels)
// liveChannel = running channel from /api/channels, or null if not loaded
function renderChannelCard(entry, liveChannel) {
  const card = document.createElement('div');
  card.className = 'ext-card';

  const header = document.createElement('div');
  header.className = 'ext-header';

  const nameEl = document.createElement('span');
  nameEl.className = 'ext-name';
  nameEl.textContent = entry.display_name || entry.name;
  header.appendChild(nameEl);

  const kindEl = document.createElement('span');
  kindEl.className = 'ext-kind kind-wasm_channel';
  kindEl.textContent = 'Channel';
  header.appendChild(kindEl);

  const isRunning = !!liveChannel;
  const isEnabled = isRunning && (liveChannel.enabled !== false);
  const dot = document.createElement('span');
  dot.className = 'ext-auth-dot ' + (isRunning ? (isEnabled ? 'authed' : 'unauthed') : 'unauthed');
  dot.title = isRunning ? (isEnabled ? 'Running' : 'Running (disabled on next restart)') : 'Not installed';
  header.appendChild(dot);

  // Gear config button — only useful when the channel is running (config is persisted to DB).
  if (isRunning) {
    const configBtn = document.createElement('button');
    configBtn.type = 'button';
    configBtn.className = 'btn-ext-config-header';
    configBtn.title = 'Configure';
    configBtn.textContent = '\u2699';
    configBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      toggleExtensionConfig(entry.name, card);
    });
    header.appendChild(configBtn);
  }

  card.appendChild(header);

  const desc = entry.description || (liveChannel && liveChannel.description);
  if (desc) {
    const descEl = document.createElement('div');
    descEl.className = 'ext-desc';
    descEl.textContent = desc;
    card.appendChild(descEl);
  }

  const actions = document.createElement('div');
  actions.className = 'ext-actions';

  if (isRunning) {
    // Token setup wizard
    const tokenBtn = document.createElement('button');
    tokenBtn.className = 'btn-ext activate';
    tokenBtn.textContent = 'Set Token';
    tokenBtn.addEventListener('click', () => {
      openWizard(Object.assign({ installed: true, authenticated: false, active: true, status: 'needs_auth', tools: [], kind: 'wasm_channel' }, entry));
    });
    actions.appendChild(tokenBtn);

    const activeLabel = document.createElement('span');
    activeLabel.className = 'ext-active-label';
    activeLabel.textContent = isEnabled ? 'Running' : 'Running (restarts disabled)';
    actions.appendChild(activeLabel);

    // Enable / Disable toggle
    const toggleBtn = document.createElement('button');
    toggleBtn.className = 'btn-ext ' + (isEnabled ? 'remove' : 'activate');
    toggleBtn.textContent = isEnabled ? 'Disable' : 'Enable';
    toggleBtn.addEventListener('click', () => {
      const endpoint = isEnabled
        ? '/api/channels/' + encodeURIComponent(liveChannel.name) + '/disable'
        : '/api/channels/' + encodeURIComponent(liveChannel.name) + '/enable';
      apiFetch(endpoint, { method: 'POST' })
        .then((resp) => {
          showNotification(resp.message || (isEnabled ? 'Channel disabled.' : 'Channel enabled.'));
          loadChannels();
        })
        .catch((err) => showNotification('Failed: ' + (err.message || err), true));
    });
    actions.appendChild(toggleBtn);
  } else {
    // Not installed — show setup guide
    const guideBtn = document.createElement('button');
    guideBtn.className = 'btn-ext activate';
    guideBtn.textContent = 'Setup Guide';
    guideBtn.addEventListener('click', () => openSetupGuide(entry));
    actions.appendChild(guideBtn);

    const statusLabel = document.createElement('span');
    statusLabel.className = 'ext-active-label';
    statusLabel.style.color = 'var(--text-secondary)';
    statusLabel.textContent = 'Not installed';
    actions.appendChild(statusLabel);
  }

  card.appendChild(actions);
  return card;
}

// --- Extension inline config panel ---

// Name of the extension whose config panel is currently open (or null).
let openConfigName = null;

function toggleExtensionConfig(name, card) {
  // Close the panel if this card is already open.
  const existing = card.querySelector('.ext-config-panel');
  if (existing) {
    existing.remove();
    openConfigName = null;
    return;
  }

  // Close any panel open on a different card.
  if (openConfigName) {
    const otherPanel = document.querySelector('.ext-config-panel');
    if (otherPanel) otherPanel.remove();
    openConfigName = null;
  }

  const panel = document.createElement('div');
  panel.className = 'ext-config-panel';
  panel.innerHTML = '<div class="empty-state">Loading\u2026</div>';
  card.appendChild(panel);
  openConfigName = name;

  apiFetch('/api/extensions/' + encodeURIComponent(name) + '/config')
    .then((data) => {
      panel.innerHTML = '';
      renderExtensionConfigForm(name, data, panel);
    })
    .catch((err) => {
      panel.innerHTML =
        '<div class="ext-desc" style="color:var(--danger)">Failed to load config: ' +
        escapeHtml(err.message) +
        '</div>';
    });
}

function renderExtensionConfigForm(name, data, container) {
  if (!data.schema || !data.schema.properties) {
    container.innerHTML = '<div class="empty-state">No configurable fields for this extension.</div>';
    return;
  }

  const props = data.schema.properties;
  const form = document.createElement('form');
  form.className = 'ext-config-form';

  for (const [field, prop] of Object.entries(props)) {
    const group = document.createElement('div');
    group.className = 'ext-config-field';

    const label = document.createElement('label');
    label.textContent = field.replace(/_/g, ' ');
    group.appendChild(label);

    const currentVal = Object.prototype.hasOwnProperty.call(data.values, field)
      ? data.values[field]
      : undefined;

    if (prop.type === 'boolean') {
      const input = document.createElement('input');
      input.type = 'checkbox';
      input.name = field;
      input.checked =
        currentVal !== undefined ? Boolean(currentVal) : Boolean(prop.default);
      group.appendChild(input);
    } else if (Array.isArray(prop.enum)) {
      const select = document.createElement('select');
      select.name = field;
      const selected = currentVal !== undefined ? currentVal : prop.default;
      for (const opt of prop.enum) {
        const option = document.createElement('option');
        option.value = opt;
        option.textContent = opt;
        if (selected === opt) option.selected = true;
        select.appendChild(option);
      }
      group.appendChild(select);
    } else if (prop.type === 'array') {
      const input = document.createElement('input');
      input.type = 'text';
      input.name = field;
      const arrVal =
        currentVal !== undefined ? currentVal : (prop.default || []);
      input.value = Array.isArray(arrVal) ? arrVal.join(', ') : String(arrVal);
      input.placeholder = 'Comma-separated values';
      group.appendChild(input);
    } else {
      const input = document.createElement('input');
      input.type =
        prop.type === 'integer' || prop.type === 'number' ? 'number' : 'text';
      input.name = field;
      const sv =
        currentVal !== undefined
          ? currentVal
          : prop.default !== undefined
          ? prop.default
          : '';
      input.value = sv !== null && sv !== undefined ? String(sv) : '';
      if (prop.nullable) input.placeholder = 'Optional';
      if (prop.minimum !== undefined) input.min = String(prop.minimum);
      group.appendChild(input);
    }

    if (prop.description) {
      const hint = document.createElement('small');
      hint.className = 'ext-config-hint';
      hint.textContent = prop.description;
      group.appendChild(hint);
    }

    form.appendChild(group);
  }

  const saveBtn = document.createElement('button');
  saveBtn.type = 'submit';
  saveBtn.className = 'btn-ext activate ext-config-save';
  saveBtn.textContent = 'Save';
  form.appendChild(saveBtn);

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    saveExtensionConfig(name, form, props);
  });

  container.appendChild(form);
}

function saveExtensionConfig(name, form, schemaProps) {
  const values = {};
  for (const [field, prop] of Object.entries(schemaProps)) {
    const el = form.elements[field];
    if (!el) continue;

    if (prop.type === 'boolean') {
      values[field] = el.checked;
    } else if (prop.type === 'array') {
      const raw = el.value.trim();
      values[field] = raw
        ? raw
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean)
        : [];
    } else if (prop.type === 'integer') {
      values[field] = el.value !== '' ? parseInt(el.value, 10) : null;
    } else if (prop.type === 'number') {
      values[field] = el.value !== '' ? parseFloat(el.value) : null;
    } else {
      values[field] = el.value !== '' ? el.value : null;
    }
  }

  apiFetch('/api/extensions/' + encodeURIComponent(name) + '/config', {
    method: 'PUT',
    body: { values },
  })
    .then(() => showToast('Config saved for ' + name, 'success'))
    .catch((err) => showToast('Save failed: ' + err.message, 'error'));
}

// --- Setup Guide Modal ---

async function openSetupGuide(entry) {
  const overlay = document.getElementById('setup-guide-overlay');
  const title = document.getElementById('setup-guide-title');
  const body = document.getElementById('setup-guide-body');
  const closeBtn = document.getElementById('setup-guide-close');

  title.textContent = (entry.display_name || entry.name) + ' — Setup Guide';
  body.innerHTML = '<div class="empty-state">Loading…</div>';
  overlay.style.display = 'flex';

  closeBtn.onclick = () => { overlay.style.display = 'none'; };
  overlay.onclick = (e) => { if (e.target === overlay) overlay.style.display = 'none'; };

  if (entry.docs_file) {
    try {
      const res = await fetch('/api/docs/' + entry.docs_file, {
        headers: { 'Authorization': 'Bearer ' + token }
      });
      if (res.ok) {
        const md = await res.text();
        body.innerHTML = marked.parse(md);
        // Make all links open in new tab
        body.querySelectorAll('a').forEach(a => a.setAttribute('target', '_blank'));
        return;
      }
    } catch (_) {}
  }

  // Fallback: channel is pre-installed in Docker but no dedicated guide exists yet
  body.innerHTML = marked.parse([
    '# ' + (entry.display_name || entry.name) + ' Setup',
    '',
    entry.description || '',
    '',
    '## Getting Started',
    '',
    entry.kind === 'wasm_channel'
      ? 'This channel is pre-installed in the RustyTalon Docker image. To activate it, set the required credentials as environment variables in your `.env` file and restart the container.'
      : 'This extension must be installed before use. Run `rustytalon tool install` to install it.',
    '',
    'Check the [documentation](https://github.com/nicklozano/rustytalon/tree/main/docs) for setup instructions.',
  ].join('\n'));
}

// --- Wizard ---

async function openWizard(entry) {
  wizardState = { name: entry.name, entry: Object.assign({}, entry), step: 0, steps: [], authInfo: null, activateError: null, waitingForAuth: false };
  document.getElementById('wizard-overlay').style.display = 'flex';

  try {
    const info = await apiFetch('/api/extensions/' + encodeURIComponent(entry.name) + '/auth-info');
    wizardState.authInfo = info.info || info;
  } catch (_) { /* proceed without auth info */ }

  wizardState.steps = buildWizardSteps(wizardState.entry, wizardState.authInfo);
  renderWizard();
}

function closeWizard() {
  document.getElementById('wizard-overlay').style.display = 'none';
  wizardState = null;
}

function buildWizardSteps(entry, authInfo) {
  const steps = ['overview'];
  if (!entry.installed) steps.push('install');
  const needsAuth = authInfo && authInfo.auth_type !== 'none' && authInfo.auth_type !== 'dcr' && !entry.authenticated;
  if (needsAuth) steps.push('auth');
  if (entry.installed && !entry.active && !needsAuth) steps.push('activate');
  steps.push('done');
  return steps;
}

function renderWizard() {
  if (!wizardState) return;
  const { entry, authInfo, steps, step } = wizardState;
  const container = document.getElementById('wizard-steps');
  container.innerHTML = '';

  // Progress dots
  const progress = document.createElement('div');
  progress.className = 'wizard-progress';
  steps.forEach((_, i) => {
    const dot = document.createElement('div');
    dot.className = 'wizard-progress-dot' + (i < step ? ' done' : i === step ? ' current' : '');
    progress.appendChild(dot);
  });
  container.appendChild(progress);

  // Current step content
  const stepName = steps[step];
  container.appendChild(renderWizardStep(stepName, entry, authInfo));
}

function renderWizardStep(stepName, entry, authInfo) {
  const div = document.createElement('div');

  switch (stepName) {
    case 'overview': {
      const title = document.createElement('div');
      title.className = 'wizard-step-title';
      title.textContent = entry.display_name || entry.name;
      div.appendChild(title);

      const desc = document.createElement('div');
      desc.className = 'wizard-step-desc';
      desc.textContent = entry.description || '';
      div.appendChild(desc);

      if (authInfo && authInfo.auth_type !== 'none') {
        const credHeader = document.createElement('div');
        credHeader.className = 'wizard-step-label';
        credHeader.textContent = "What you'll need:";
        div.appendChild(credHeader);

        const credList = document.createElement('ul');
        credList.className = 'wizard-credential-list';
        const li = document.createElement('li');
        li.textContent = authInfo.display_name ? authInfo.display_name + ' credentials' : 'API credentials';
        if (authInfo.setup_url) {
          const link = document.createElement('a');
          link.className = 'wizard-link';
          link.href = authInfo.setup_url;
          link.target = '_blank';
          link.rel = 'noopener noreferrer';
          link.textContent = 'Get credentials';
          li.appendChild(link);
        }
        credList.appendChild(li);
        div.appendChild(credList);
      }

      const actions = document.createElement('div');
      actions.className = 'wizard-actions';
      const cancelBtn = document.createElement('button');
      cancelBtn.className = 'wizard-btn-secondary';
      cancelBtn.textContent = 'Cancel';
      cancelBtn.onclick = closeWizard;
      actions.appendChild(cancelBtn);

      const nextBtn = document.createElement('button');
      nextBtn.className = 'wizard-btn-primary';
      if (!entry.installed) nextBtn.textContent = 'Install';
      else if (!entry.authenticated && authInfo && authInfo.auth_type !== 'none') nextBtn.textContent = 'Set Up Auth';
      else nextBtn.textContent = 'Continue';
      nextBtn.onclick = () => wizardAdvance();
      actions.appendChild(nextBtn);
      div.appendChild(actions);
      break;
    }

    case 'install': {
      const title = document.createElement('div');
      title.className = 'wizard-step-title';
      title.textContent = 'Installing ' + (entry.display_name || entry.name);
      div.appendChild(title);

      const desc = document.createElement('div');
      desc.className = 'wizard-step-desc';
      desc.textContent = 'Click Install to add this extension.';
      div.appendChild(desc);

      const actions = document.createElement('div');
      actions.className = 'wizard-actions';
      const backBtn = document.createElement('button');
      backBtn.className = 'wizard-btn-secondary';
      backBtn.textContent = 'Back';
      backBtn.onclick = () => wizardBack();
      actions.appendChild(backBtn);

      const installBtn = document.createElement('button');
      installBtn.className = 'wizard-btn-primary';
      installBtn.textContent = 'Install';
      installBtn.onclick = async () => {
        installBtn.disabled = true;
        installBtn.textContent = 'Installing...';
        try {
          const res = await apiFetch('/api/extensions/install', {
            method: 'POST',
            body: { name: entry.name, kind: entry.kind },
          });
          if (res.success) {
            wizardState.entry.installed = true;
            wizardState.steps = buildWizardSteps(wizardState.entry, wizardState.authInfo);
            // Reset to step 1 (first step after overview) rather than incrementing
            // the old index — the steps array has been rebuilt so the old index
            // would skip the newly-inserted auth/activate step.
            wizardState.step = 1;
            renderWizard();
          } else {
            showToast('Install failed: ' + res.message, 'error');
            installBtn.disabled = false;
            installBtn.textContent = 'Retry';
          }
        } catch (e) {
          showToast('Install failed: ' + e.message, 'error');
          installBtn.disabled = false;
          installBtn.textContent = 'Retry';
        }
      };
      actions.appendChild(installBtn);
      div.appendChild(actions);
      break;
    }

    case 'auth': {
      const title = document.createElement('div');
      title.className = 'wizard-step-title';
      title.textContent = 'Connect ' + ((authInfo && authInfo.display_name) || entry.display_name || entry.name);
      div.appendChild(title);

      if (authInfo && authInfo.instructions) {
        const instr = document.createElement('div');
        instr.className = 'wizard-step-desc';
        instr.textContent = authInfo.instructions;
        div.appendChild(instr);
      }

      const actions = document.createElement('div');
      actions.className = 'wizard-actions';
      const backBtn = document.createElement('button');
      backBtn.className = 'wizard-btn-secondary';
      backBtn.textContent = 'Back';
      backBtn.onclick = () => wizardBack();
      actions.appendChild(backBtn);

      if (authInfo && authInfo.oauth_available) {
        const oauthBtn = document.createElement('button');
        oauthBtn.className = 'wizard-btn-primary';
        oauthBtn.textContent = 'Authorize with ' + ((authInfo && authInfo.display_name) || entry.name);
        oauthBtn.onclick = async () => {
          oauthBtn.disabled = true;
          oauthBtn.textContent = 'Opening...';
          try {
            const res = await apiFetch('/api/extensions/' + encodeURIComponent(entry.name) + '/activate', { method: 'POST' });
            if (res.auth_url) {
              window.open(res.auth_url, '_blank');
              wizardState.waitingForAuth = true;
              oauthBtn.textContent = 'Waiting for authorization...';
            } else if (res.success) {
              wizardState.entry.authenticated = true;
              wizardState.entry.active = true;
              wizardState.steps = buildWizardSteps(wizardState.entry, wizardState.authInfo);
              wizardState.step = 1;
              renderWizard();
            } else {
              showToast(res.message, 'error');
              oauthBtn.disabled = false;
              oauthBtn.textContent = 'Retry';
            }
          } catch (e) {
            showToast(e.message, 'error');
            oauthBtn.disabled = false;
            oauthBtn.textContent = 'Retry';
          }
        };
        actions.appendChild(oauthBtn);
      } else {
        // Manual token entry
        const label = document.createElement('label');
        label.className = 'wizard-step-label';
        label.textContent = 'API Token';
        div.appendChild(label);

        const tokenInput = document.createElement('input');
        tokenInput.type = 'password';
        tokenInput.className = 'wizard-token-input';
        tokenInput.placeholder = (authInfo && authInfo.token_hint) || 'Paste your API token';
        div.appendChild(tokenInput);

        if (authInfo && authInfo.token_hint) {
          const hint = document.createElement('div');
          hint.className = 'wizard-hint';
          hint.textContent = authInfo.token_hint;
          div.appendChild(hint);
        }

        const submitBtn = document.createElement('button');
        submitBtn.className = 'wizard-btn-primary';
        submitBtn.textContent = 'Connect';
        submitBtn.onclick = async () => {
          const token = tokenInput.value.trim();
          if (!token) { showToast('Token is required', 'error'); return; }
          submitBtn.disabled = true;
          submitBtn.textContent = 'Connecting...';
          try {
            const res = await apiFetch('/api/chat/auth-token', {
              method: 'POST',
              body: { extension_name: entry.name, token },
            });
            if (res.success || res.status === 'authenticated') {
              wizardState.entry.authenticated = true;
              wizardState.steps = buildWizardSteps(wizardState.entry, wizardState.authInfo);
              wizardState.step = 1;
              renderWizard();
            } else {
              showToast((res.message) || 'Auth failed', 'error');
              submitBtn.disabled = false;
              submitBtn.textContent = 'Retry';
            }
          } catch (e) {
            showToast(e.message, 'error');
            submitBtn.disabled = false;
            submitBtn.textContent = 'Retry';
          }
        };
        actions.appendChild(submitBtn);
      }

      div.appendChild(actions);
      break;
    }

    case 'activate': {
      const title = document.createElement('div');
      title.className = 'wizard-step-title';
      title.textContent = 'Activating...';
      div.appendChild(title);

      // Auto-activate when this step is entered
      setTimeout(async () => {
        try {
          const res = await apiFetch('/api/extensions/' + encodeURIComponent(entry.name) + '/activate', { method: 'POST' });
          if (res.success) {
            wizardState.entry.active = true;
          } else {
            wizardState.activateError = res.message;
          }
        } catch (e) {
          wizardState.activateError = e.message;
        }
        wizardAdvance();
      }, 0);
      break;
    }

    case 'done': {
      const title = document.createElement('div');
      title.className = 'wizard-step-title';
      if (wizardState.activateError) {
        title.textContent = 'Setup encountered an issue';
      } else if (entry.active) {
        title.textContent = (entry.display_name || entry.name) + ' is ready';
      } else {
        title.textContent = 'Setup complete';
      }
      div.appendChild(title);

      const desc = document.createElement('div');
      desc.className = 'wizard-step-desc';
      if (wizardState.activateError) {
        desc.textContent = wizardState.activateError;
      } else if (entry.active) {
        desc.textContent = 'The extension is installed and active. You can now use it.';
      } else if (entry.kind === 'wasm_channel') {
        desc.textContent = 'Channel installed. Restart RustyTalon to activate the channel.';
      } else {
        desc.textContent = 'Extension is set up. You can activate it from the Installed tab.';
      }
      div.appendChild(desc);

      const actions = document.createElement('div');
      actions.className = 'wizard-actions';
      const doneBtn = document.createElement('button');
      doneBtn.className = 'wizard-btn-primary';
      doneBtn.textContent = 'Done';
      doneBtn.onclick = () => {
        closeWizard();
        loadCatalog();
        loadInstalledExtensions();
      };
      actions.appendChild(doneBtn);
      div.appendChild(actions);
      break;
    }

    default:
      break;
  }

  return div;
}

function wizardAdvance() {
  if (!wizardState) return;
  wizardState.step = Math.min(wizardState.step + 1, wizardState.steps.length - 1);
  renderWizard();
}

function wizardBack() {
  if (!wizardState) return;
  wizardState.step = Math.max(wizardState.step - 1, 0);
  renderWizard();
}

// --- Jobs ---

let currentJobId = null;
let currentJobSubTab = 'overview';
let jobFilesTreeState = null;

function loadJobs() {
  currentJobId = null;
  jobFilesTreeState = null;

  // Rebuild DOM if renderJobDetail() destroyed it (it wipes .jobs-container innerHTML).
  const container = document.querySelector('.jobs-container');
  if (!document.getElementById('jobs-summary')) {
    container.innerHTML =
      '<div class="jobs-summary" id="jobs-summary"></div>'
      + '<table class="jobs-table" id="jobs-table"><thead><tr>'
      + '<th>ID</th><th>Title</th><th>Status</th><th>Created</th><th>Actions</th>'
      + '</tr></thead><tbody id="jobs-tbody"></tbody></table>'
      + '<div class="empty-state" id="jobs-empty" style="display:none">No jobs found</div>';
  }

  Promise.all([
    apiFetch('/api/jobs/summary'),
    apiFetch('/api/jobs'),
  ]).then(([summary, jobList]) => {
    renderJobsSummary(summary);
    renderJobsList(jobList.jobs);
  }).catch(() => {});
}

function renderJobsSummary(s) {
  document.getElementById('jobs-summary').innerHTML = ''
    + summaryCard('Total', s.total, '')
    + summaryCard('In Progress', s.in_progress, 'active')
    + summaryCard('Completed', s.completed, 'completed')
    + summaryCard('Failed', s.failed, 'failed')
    + summaryCard('Stuck', s.stuck, 'stuck');
}

function summaryCard(label, count, cls) {
  return '<div class="summary-card ' + cls + '">'
    + '<div class="count">' + count + '</div>'
    + '<div class="label">' + label + '</div>'
    + '</div>';
}

function renderJobsList(jobs) {
  const tbody = document.getElementById('jobs-tbody');
  const empty = document.getElementById('jobs-empty');

  if (jobs.length === 0) {
    tbody.innerHTML = '';
    empty.style.display = 'block';
    return;
  }

  empty.style.display = 'none';
  tbody.innerHTML = jobs.map((job) => {
    const shortId = job.id.substring(0, 8);
    const stateClass = job.state.replace(' ', '_');

    let actionBtns = '';
    if (job.state === 'pending' || job.state === 'in_progress') {
      actionBtns = '<button class="btn-cancel" onclick="event.stopPropagation(); cancelJob(\'' + job.id + '\')">Cancel</button>';
    } else if (job.state === 'failed' || job.state === 'interrupted') {
      actionBtns = '<button class="btn-restart" onclick="event.stopPropagation(); restartJob(\'' + job.id + '\')">Restart</button>';
    }

    return '<tr class="job-row" onclick="openJobDetail(\'' + job.id + '\')">'
      + '<td title="' + escapeHtml(job.id) + '">' + shortId + '</td>'
      + '<td>' + escapeHtml(job.title) + '</td>'
      + '<td><span class="badge ' + stateClass + '">' + escapeHtml(job.state) + '</span></td>'
      + '<td>' + formatDate(job.created_at) + '</td>'
      + '<td>' + actionBtns + '</td>'
      + '</tr>';
  }).join('');
}

function cancelJob(jobId) {
  if (!confirm('Cancel this job?')) return;
  apiFetch('/api/jobs/' + jobId + '/cancel', { method: 'POST' })
    .then(() => {
      showToast('Job cancelled', 'success');
      if (currentJobId) openJobDetail(currentJobId);
      else loadJobs();
    })
    .catch((err) => {
      showToast('Failed to cancel job: ' + err.message, 'error');
    });
}

function restartJob(jobId) {
  apiFetch('/api/jobs/' + jobId + '/restart', { method: 'POST' })
    .then((res) => {
      showToast('Job restarted as ' + (res.new_job_id || '').substring(0, 8), 'success');
      loadJobs();
    })
    .catch((err) => {
      showToast('Failed to restart job: ' + err.message, 'error');
    });
}

function openJobDetail(jobId) {
  currentJobId = jobId;
  currentJobSubTab = 'activity';
  apiFetch('/api/jobs/' + jobId).then((job) => {
    renderJobDetail(job);
  }).catch((err) => {
    addMessage('system', 'Failed to load job: ' + err.message);
    closeJobDetail();
  });
}

function closeJobDetail() {
  currentJobId = null;
  jobFilesTreeState = null;
  loadJobs();
}

function renderJobDetail(job) {
  const container = document.querySelector('.jobs-container');
  const stateClass = job.state.replace(' ', '_');

  container.innerHTML = '';

  // Header
  const header = document.createElement('div');
  header.className = 'job-detail-header';

  let headerHtml = '<button class="btn-back" onclick="closeJobDetail()">&larr; Back</button>'
    + '<h2>' + escapeHtml(job.title) + '</h2>'
    + '<span class="badge ' + stateClass + '">' + escapeHtml(job.state) + '</span>';

  if (job.state === 'failed' || job.state === 'interrupted') {
    headerHtml += '<button class="btn-restart" onclick="restartJob(\'' + job.id + '\')">Restart</button>';
  }
  if (job.browse_url) {
    headerHtml += '<a class="btn-browse" href="' + escapeHtml(job.browse_url) + '" target="_blank">Browse Files</a>';
  }

  header.innerHTML = headerHtml;
  container.appendChild(header);

  // Sub-tab bar
  const tabs = document.createElement('div');
  tabs.className = 'job-detail-tabs';
  const subtabs = ['overview', 'activity', 'files'];
  for (const st of subtabs) {
    const btn = document.createElement('button');
    btn.textContent = st.charAt(0).toUpperCase() + st.slice(1);
    btn.className = st === currentJobSubTab ? 'active' : '';
    btn.addEventListener('click', () => {
      currentJobSubTab = st;
      renderJobDetail(job);
    });
    tabs.appendChild(btn);
  }
  container.appendChild(tabs);

  // Content
  const content = document.createElement('div');
  content.className = 'job-detail-content';
  container.appendChild(content);

  switch (currentJobSubTab) {
    case 'overview': renderJobOverview(content, job); break;
    case 'files': renderJobFiles(content, job); break;
    case 'activity': renderJobActivity(content, job); break;
  }
}

function metaItem(label, value) {
  return '<div class="meta-item"><div class="meta-label">' + escapeHtml(label)
    + '</div><div class="meta-value">' + escapeHtml(String(value != null ? value : '-'))
    + '</div></div>';
}

function formatDuration(secs) {
  if (secs == null) return '-';
  if (secs < 60) return secs + 's';
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  if (m < 60) return m + 'm ' + s + 's';
  const h = Math.floor(m / 60);
  return h + 'h ' + (m % 60) + 'm';
}

function renderJobOverview(container, job) {
  // Metadata grid
  const grid = document.createElement('div');
  grid.className = 'job-meta-grid';
  grid.innerHTML = metaItem('Job ID', job.id)
    + metaItem('State', job.state)
    + metaItem('Created', formatDate(job.created_at))
    + metaItem('Started', formatDate(job.started_at))
    + metaItem('Completed', formatDate(job.completed_at))
    + metaItem('Duration', formatDuration(job.elapsed_secs))
    + (job.job_mode ? metaItem('Mode', job.job_mode) : '');
  container.appendChild(grid);

  // Description
  if (job.description) {
    const descSection = document.createElement('div');
    descSection.className = 'job-description';
    const descHeader = document.createElement('h3');
    descHeader.textContent = 'Description';
    descSection.appendChild(descHeader);
    const descBody = document.createElement('div');
    descBody.className = 'job-description-body';
    descBody.innerHTML = renderMarkdown(job.description);
    descSection.appendChild(descBody);
    container.appendChild(descSection);
  }

  // State transitions timeline
  if (job.transitions.length > 0) {
    const timelineSection = document.createElement('div');
    timelineSection.className = 'job-timeline-section';
    const tlHeader = document.createElement('h3');
    tlHeader.textContent = 'State Transitions';
    timelineSection.appendChild(tlHeader);

    const timeline = document.createElement('div');
    timeline.className = 'timeline';
    for (const t of job.transitions) {
      const entry = document.createElement('div');
      entry.className = 'timeline-entry';
      const dot = document.createElement('div');
      dot.className = 'timeline-dot';
      entry.appendChild(dot);
      const info = document.createElement('div');
      info.className = 'timeline-info';
      info.innerHTML = '<span class="badge ' + t.from.replace(' ', '_') + '">' + escapeHtml(t.from) + '</span>'
        + ' &rarr; '
        + '<span class="badge ' + t.to.replace(' ', '_') + '">' + escapeHtml(t.to) + '</span>'
        + '<span class="timeline-time">' + formatDate(t.timestamp) + '</span>'
        + (t.reason ? '<div class="timeline-reason">' + escapeHtml(t.reason) + '</div>' : '');
      entry.appendChild(info);
      timeline.appendChild(entry);
    }
    timelineSection.appendChild(timeline);
    container.appendChild(timelineSection);
  }
}

function renderJobFiles(container, job) {
  container.innerHTML = '<div class="job-files">'
    + '<div class="job-files-sidebar"><div class="job-files-tree"></div></div>'
    + '<div class="job-files-viewer"><div class="empty-state">Select a file to view</div></div>'
    + '</div>';

  container._jobId = job ? job.id : null;

  apiFetch('/api/jobs/' + job.id + '/files/list?path=').then((data) => {
    jobFilesTreeState = data.entries.map((e) => ({
      name: e.name,
      path: e.path,
      is_dir: e.is_dir,
      children: e.is_dir ? null : undefined,
      expanded: false,
      loaded: false,
    }));
    renderJobFilesTree();
  }).catch(() => {
    const treeContainer = document.querySelector('.job-files-tree');
    if (treeContainer) {
      treeContainer.innerHTML = '<div class="tree-item" style="color:var(--text-secondary)">No project files</div>';
    }
  });
}

function renderJobFilesTree() {
  const treeContainer = document.querySelector('.job-files-tree');
  if (!treeContainer) return;
  treeContainer.innerHTML = '';
  if (!jobFilesTreeState || jobFilesTreeState.length === 0) {
    treeContainer.innerHTML = '<div class="tree-item" style="color:var(--text-secondary)">No files in workspace</div>';
    return;
  }
  renderJobFileNodes(jobFilesTreeState, treeContainer, 0);
}

function renderJobFileNodes(nodes, container, depth) {
  for (const node of nodes) {
    const row = document.createElement('div');
    row.className = 'tree-row';
    row.style.paddingLeft = (depth * 16 + 8) + 'px';

    if (node.is_dir) {
      const arrow = document.createElement('span');
      arrow.className = 'expand-arrow' + (node.expanded ? ' expanded' : '');
      arrow.textContent = '\u25B6';
      arrow.addEventListener('click', (e) => {
        e.stopPropagation();
        toggleJobFileExpand(node);
      });
      row.appendChild(arrow);

      const label = document.createElement('span');
      label.className = 'tree-label dir';
      label.textContent = node.name;
      label.addEventListener('click', () => toggleJobFileExpand(node));
      row.appendChild(label);
    } else {
      const spacer = document.createElement('span');
      spacer.className = 'expand-arrow-spacer';
      row.appendChild(spacer);

      const label = document.createElement('span');
      label.className = 'tree-label file';
      label.textContent = node.name;
      label.addEventListener('click', () => readJobFile(node.path));
      row.appendChild(label);
    }

    container.appendChild(row);

    if (node.is_dir && node.expanded && node.children) {
      const childContainer = document.createElement('div');
      childContainer.className = 'tree-children';
      renderJobFileNodes(node.children, childContainer, depth + 1);
      container.appendChild(childContainer);
    }
  }
}

function getJobId() {
  const container = document.querySelector('.job-detail-content');
  return (container && container._jobId) || null;
}

function toggleJobFileExpand(node) {
  if (node.expanded) {
    node.expanded = false;
    renderJobFilesTree();
    return;
  }
  if (node.loaded) {
    node.expanded = true;
    renderJobFilesTree();
    return;
  }
  const jobId = getJobId();
  apiFetch('/api/jobs/' + jobId + '/files/list?path=' + encodeURIComponent(node.path)).then((data) => {
    node.children = data.entries.map((e) => ({
      name: e.name,
      path: e.path,
      is_dir: e.is_dir,
      children: e.is_dir ? null : undefined,
      expanded: false,
      loaded: false,
    }));
    node.loaded = true;
    node.expanded = true;
    renderJobFilesTree();
  }).catch(() => {});
}

function readJobFile(path) {
  const viewer = document.querySelector('.job-files-viewer');
  if (!viewer) return;
  const jobId = getJobId();
  apiFetch('/api/jobs/' + jobId + '/files/read?path=' + encodeURIComponent(path)).then((data) => {
    viewer.innerHTML = '<div class="job-files-path">' + escapeHtml(path) + '</div>'
      + '<pre class="job-files-content">' + escapeHtml(data.content) + '</pre>';
  }).catch((err) => {
    viewer.innerHTML = '<div class="empty-state">Error: ' + escapeHtml(err.message) + '</div>';
  });
}

// --- Activity tab (unified for all sandbox jobs) ---

let activityCurrentJobId = null;
// Track how many live SSE events we've already rendered so refreshActivityTab
// only appends new ones (avoids duplicates on each SSE tick).
let activityRenderedLiveIndex = 0;

function renderJobActivity(container, job) {
  activityCurrentJobId = job ? job.id : null;
  activityRenderedLiveIndex = 0;

  container.innerHTML = '<div class="activity-toolbar">'
    + '<select id="activity-type-filter">'
    + '<option value="all">All Events</option>'
    + '<option value="message">Messages</option>'
    + '<option value="tool_use">Tool Calls</option>'
    + '<option value="tool_result">Results</option>'
    + '</select>'
    + '<label class="logs-checkbox"><input type="checkbox" id="activity-autoscroll" checked> Auto-scroll</label>'
    + '</div>'
    + '<div class="activity-terminal" id="activity-terminal"></div>'
    + '<div class="activity-input-bar" id="activity-input-bar">'
    + '<input type="text" id="activity-prompt-input" placeholder="Send follow-up prompt..." />'
    + '<button id="activity-send-btn">Send</button>'
    + '<button id="activity-done-btn" title="Signal done">Done</button>'
    + '</div>';

  document.getElementById('activity-type-filter').addEventListener('change', applyActivityFilter);

  const terminal = document.getElementById('activity-terminal');
  const input = document.getElementById('activity-prompt-input');
  const sendBtn = document.getElementById('activity-send-btn');
  const doneBtn = document.getElementById('activity-done-btn');

  sendBtn.addEventListener('click', () => sendJobPrompt(job.id, false));
  doneBtn.addEventListener('click', () => sendJobPrompt(job.id, true));
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') sendJobPrompt(job.id, false);
  });

  // Load persisted events from DB, then catch up with any live SSE events
  apiFetch('/api/jobs/' + job.id + '/events').then((data) => {
    if (data.events && data.events.length > 0) {
      for (const evt of data.events) {
        appendActivityEvent(terminal, evt.event_type, evt.data);
      }
    }
    appendNewLiveEvents(terminal, job.id);
  }).catch(() => {
    appendNewLiveEvents(terminal, job.id);
  });
}

function appendNewLiveEvents(terminal, jobId) {
  const live = jobEvents.get(jobId) || [];
  for (let i = activityRenderedLiveIndex; i < live.length; i++) {
    const evt = live[i];
    appendActivityEvent(terminal, evt.type.replace('job_', ''), evt.data);
  }
  activityRenderedLiveIndex = live.length;
  const autoScroll = document.getElementById('activity-autoscroll');
  if (!autoScroll || autoScroll.checked) {
    terminal.scrollTop = terminal.scrollHeight;
  }
}

function applyActivityFilter() {
  const filter = document.getElementById('activity-type-filter').value;
  const events = document.querySelectorAll('#activity-terminal .activity-event');
  for (const el of events) {
    if (filter === 'all') {
      el.style.display = '';
    } else {
      el.style.display = el.getAttribute('data-event-type') === filter ? '' : 'none';
    }
  }
}

function appendActivityEvent(terminal, eventType, data) {
  if (!terminal) return;
  const el = document.createElement('div');
  el.className = 'activity-event activity-event-' + eventType;
  el.setAttribute('data-event-type', eventType);

  // Respect current filter
  const filterEl = document.getElementById('activity-type-filter');
  if (filterEl && filterEl.value !== 'all' && filterEl.value !== eventType) {
    el.style.display = 'none';
  }

  switch (eventType) {
    case 'message':
      el.innerHTML = '<span class="activity-role">' + escapeHtml(data.role || 'assistant') + '</span> '
        + '<span class="activity-content">' + escapeHtml(data.content || '') + '</span>';
      break;
    case 'tool_use':
      el.innerHTML = '<details class="activity-tool-block"><summary>'
        + '<span class="activity-tool-icon">&#9881;</span> '
        + escapeHtml(data.tool_name || 'tool')
        + '</summary><pre class="activity-tool-input">'
        + escapeHtml(typeof data.input === 'string' ? data.input : JSON.stringify(data.input, null, 2))
        + '</pre></details>';
      break;
    case 'tool_result':
      el.innerHTML = '<details class="activity-tool-block activity-tool-result"><summary>'
        + '<span class="activity-tool-icon">&#10003;</span> '
        + escapeHtml(data.tool_name || 'result')
        + '</summary><pre class="activity-tool-output">'
        + escapeHtml(data.output || '')
        + '</pre></details>';
      break;
    case 'status':
      el.innerHTML = '<span class="activity-status">' + escapeHtml(data.message || '') + '</span>';
      break;
    case 'result':
      el.className += ' activity-final';
      const success = data.success !== false;
      el.innerHTML = '<span class="activity-result-status" data-success="' + success + '">'
        + escapeHtml(data.message || data.status || 'done') + '</span>';
      if (data.session_id) {
        el.innerHTML += ' <span class="activity-session-id">session: ' + escapeHtml(data.session_id) + '</span>';
      }
      break;
    default:
      el.innerHTML = '<span class="activity-status">' + escapeHtml(JSON.stringify(data)) + '</span>';
  }

  terminal.appendChild(el);
}

function refreshActivityTab(jobId) {
  if (activityCurrentJobId !== jobId) return;
  if (currentJobSubTab !== 'activity') return;
  const terminal = document.getElementById('activity-terminal');
  if (!terminal) return;
  appendNewLiveEvents(terminal, jobId);
}

function sendJobPrompt(jobId, done) {
  const input = document.getElementById('activity-prompt-input');
  const content = input ? input.value.trim() : '';
  if (!content && !done) return;

  apiFetch('/api/jobs/' + jobId + '/prompt', {
    method: 'POST',
    body: { content: content || '(done)', done: done },
  }).then(() => {
    if (input) input.value = '';
    if (done) {
      const bar = document.getElementById('activity-input-bar');
      if (bar) bar.innerHTML = '<span class="activity-status">Done signal sent</span>';
    }
  }).catch((err) => {
    const terminal = document.getElementById('activity-terminal');
    if (terminal) {
      appendActivityEvent(terminal, 'status', { message: 'Failed to send: ' + err.message });
    }
  });
}

// --- Routines ---

let currentRoutineId = null;

function loadRoutines() {
  currentRoutineId = null;

  // Restore list view if detail was open
  const detail = document.getElementById('routine-detail');
  if (detail) detail.style.display = 'none';
  const table = document.getElementById('routines-table');
  if (table) table.style.display = '';

  Promise.all([
    apiFetch('/api/routines/summary'),
    apiFetch('/api/routines'),
  ]).then(([summary, listData]) => {
    renderRoutinesSummary(summary);
    renderRoutinesList(listData.routines);
  }).catch(() => {});
}

function renderRoutinesSummary(s) {
  document.getElementById('routines-summary').innerHTML = ''
    + summaryCard('Total', s.total, '')
    + summaryCard('Enabled', s.enabled, 'active')
    + summaryCard('Disabled', s.disabled, '')
    + summaryCard('Failing', s.failing, 'failed')
    + summaryCard('Runs Today', s.runs_today, 'completed');
}

function renderRoutinesList(routines) {
  const tbody = document.getElementById('routines-tbody');
  const empty = document.getElementById('routines-empty');

  if (!routines || routines.length === 0) {
    tbody.innerHTML = '';
    empty.style.display = 'block';
    return;
  }

  empty.style.display = 'none';
  tbody.innerHTML = routines.map((r) => {
    const statusClass = r.status === 'active' ? 'completed'
      : r.status === 'failing' ? 'failed'
      : 'pending';

    const toggleLabel = r.enabled ? 'Disable' : 'Enable';
    const toggleClass = r.enabled ? 'btn-cancel' : 'btn-restart';

    return '<tr class="routine-row" onclick="openRoutineDetail(\'' + r.id + '\')">'
      + '<td>' + escapeHtml(r.name) + '</td>'
      + '<td>' + escapeHtml(r.trigger_summary) + '</td>'
      + '<td>' + escapeHtml(r.action_type) + '</td>'
      + '<td>' + formatRelativeTime(r.last_run_at) + '</td>'
      + '<td>' + formatRelativeTime(r.next_fire_at) + '</td>'
      + '<td>' + r.run_count + '</td>'
      + '<td><span class="badge ' + statusClass + '">' + escapeHtml(r.status) + '</span></td>'
      + '<td>'
      + '<button class="' + toggleClass + '" onclick="event.stopPropagation(); toggleRoutine(\'' + r.id + '\')">' + toggleLabel + '</button> '
      + '<button class="btn-restart" onclick="event.stopPropagation(); triggerRoutine(\'' + r.id + '\')">Run</button> '
      + '<button class="btn-cancel" onclick="event.stopPropagation(); deleteRoutine(\'' + r.id + '\', \'' + escapeHtml(r.name) + '\')">Delete</button>'
      + '</td>'
      + '</tr>';
  }).join('');
}

function openRoutineDetail(id) {
  currentRoutineId = id;
  apiFetch('/api/routines/' + id).then((routine) => {
    renderRoutineDetail(routine);
  }).catch((err) => {
    showToast('Failed to load routine: ' + err.message, 'error');
  });
}

function closeRoutineDetail() {
  currentRoutineId = null;
  loadRoutines();
}

function renderRoutineDetail(routine) {
  const table = document.getElementById('routines-table');
  if (table) table.style.display = 'none';
  document.getElementById('routines-empty').style.display = 'none';

  const detail = document.getElementById('routine-detail');
  detail.style.display = 'block';

  const statusClass = !routine.enabled ? 'pending'
    : routine.consecutive_failures > 0 ? 'failed'
    : 'completed';
  const statusLabel = !routine.enabled ? 'disabled'
    : routine.consecutive_failures > 0 ? 'failing'
    : 'active';

  let html = '<div class="job-detail-header">'
    + '<button class="btn-back" onclick="closeRoutineDetail()">&larr; Back</button>'
    + '<h2>' + escapeHtml(routine.name) + '</h2>'
    + '<span class="badge ' + statusClass + '">' + escapeHtml(statusLabel) + '</span>'
    + '</div>';

  // Metadata grid
  html += '<div class="job-meta-grid">'
    + metaItem('Routine ID', routine.id)
    + metaItem('Enabled', routine.enabled ? 'Yes' : 'No')
    + metaItem('Run Count', routine.run_count)
    + metaItem('Failures', routine.consecutive_failures)
    + metaItem('Last Run', formatDate(routine.last_run_at))
    + metaItem('Next Fire', formatDate(routine.next_fire_at))
    + metaItem('Created', formatDate(routine.created_at))
    + '</div>';

  // Description
  if (routine.description) {
    html += '<div class="job-description"><h3>Description</h3>'
      + '<div class="job-description-body">' + escapeHtml(routine.description) + '</div></div>';
  }

  // Trigger config
  html += '<div class="job-description"><h3>Trigger</h3>'
    + '<pre class="action-json">' + escapeHtml(JSON.stringify(routine.trigger, null, 2)) + '</pre></div>';

  // Action config
  html += '<div class="job-description"><h3>Action</h3>'
    + '<pre class="action-json">' + escapeHtml(JSON.stringify(routine.action, null, 2)) + '</pre></div>';

  // Recent runs
  if (routine.recent_runs && routine.recent_runs.length > 0) {
    html += '<div class="job-timeline-section"><h3>Recent Runs</h3>'
      + '<table class="routines-table"><thead><tr>'
      + '<th>Trigger</th><th>Started</th><th>Completed</th><th>Status</th><th>Summary</th><th>Tokens</th>'
      + '</tr></thead><tbody>';
    for (const run of routine.recent_runs) {
      const runStatusClass = run.status === 'Ok' ? 'completed'
        : run.status === 'Failed' ? 'failed'
        : run.status === 'Attention' ? 'stuck'
        : 'in_progress';
      html += '<tr>'
        + '<td>' + escapeHtml(run.trigger_type) + '</td>'
        + '<td>' + formatDate(run.started_at) + '</td>'
        + '<td>' + formatDate(run.completed_at) + '</td>'
        + '<td><span class="badge ' + runStatusClass + '">' + escapeHtml(run.status) + '</span></td>'
        + '<td>' + escapeHtml(run.result_summary || '-') + '</td>'
        + '<td>' + (run.tokens_used != null ? run.tokens_used : '-') + '</td>'
        + '</tr>';
    }
    html += '</tbody></table></div>';
  }

  detail.innerHTML = html;
}

function triggerRoutine(id) {
  apiFetch('/api/routines/' + id + '/trigger', { method: 'POST' })
    .then(() => showToast('Routine triggered', 'success'))
    .catch((err) => showToast('Trigger failed: ' + err.message, 'error'));
}

function toggleRoutine(id) {
  apiFetch('/api/routines/' + id + '/toggle', { method: 'POST' })
    .then((res) => {
      showToast('Routine ' + (res.status || 'toggled'), 'success');
      if (currentRoutineId) openRoutineDetail(currentRoutineId);
      else loadRoutines();
    })
    .catch((err) => showToast('Toggle failed: ' + err.message, 'error'));
}

function deleteRoutine(id, name) {
  if (!confirm('Delete routine "' + name + '"?')) return;
  apiFetch('/api/routines/' + id, { method: 'DELETE' })
    .then(() => {
      showToast('Routine deleted', 'success');
      if (currentRoutineId === id) closeRoutineDetail();
      else loadRoutines();
    })
    .catch((err) => showToast('Delete failed: ' + err.message, 'error'));
}

function formatRelativeTime(isoString) {
  if (!isoString) return '-';
  const d = new Date(isoString);
  const now = Date.now();
  const diffMs = now - d.getTime();
  const absDiff = Math.abs(diffMs);
  const future = diffMs < 0;

  if (absDiff < 60000) return future ? 'in <1m' : '<1m ago';
  if (absDiff < 3600000) {
    const m = Math.floor(absDiff / 60000);
    return future ? 'in ' + m + 'm' : m + 'm ago';
  }
  if (absDiff < 86400000) {
    const h = Math.floor(absDiff / 3600000);
    return future ? 'in ' + h + 'h' : h + 'h ago';
  }
  const days = Math.floor(absDiff / 86400000);
  return future ? 'in ' + days + 'd' : days + 'd ago';
}

// --- Gateway status widget ---

let gatewayStatusInterval = null;

function startGatewayStatusPolling() {
  fetchGatewayStatus();
  gatewayStatusInterval = setInterval(fetchGatewayStatus, 30000);
}

function fetchGatewayStatus() {
  apiFetch('/api/gateway/status').then((data) => {
    const popover = document.getElementById('gateway-popover');
    popover.innerHTML = '<div class="gw-stat"><span>SSE clients</span><span>' + (data.sse_clients || 0) + '</span></div>'
      + '<div class="gw-stat"><span>Log clients</span><span>' + (data.log_clients || 0) + '</span></div>'
      + '<div class="gw-stat"><span>Uptime</span><span>' + formatDuration(data.uptime_secs) + '</span></div>';
  }).catch(() => {});
}

// Show/hide popover on hover
document.getElementById('gateway-status-trigger').addEventListener('mouseenter', () => {
  document.getElementById('gateway-popover').classList.add('visible');
});
document.getElementById('gateway-status-trigger').addEventListener('mouseleave', () => {
  document.getElementById('gateway-popover').classList.remove('visible');
});

// --- Keyboard shortcuts ---

document.addEventListener('keydown', (e) => {
  const mod = e.metaKey || e.ctrlKey;
  const tag = (e.target.tagName || '').toLowerCase();
  const inInput = tag === 'input' || tag === 'textarea';

  // Mod+1-6: switch tabs
  if (mod && e.key >= '1' && e.key <= '6') {
    e.preventDefault();
    const tabs = ['chat', 'memory', 'jobs', 'routines', 'logs', 'extensions', 'channels'];
    const idx = parseInt(e.key) - 1;
    if (tabs[idx]) switchTab(tabs[idx]);
    return;
  }

  // Mod+K: focus chat input or memory search
  if (mod && e.key === 'k') {
    e.preventDefault();
    if (currentTab === 'memory') {
      document.getElementById('memory-search').focus();
    } else {
      document.getElementById('chat-input').focus();
    }
    return;
  }

  // Mod+N: new thread
  if (mod && e.key === 'n' && currentTab === 'chat') {
    e.preventDefault();
    createNewThread();
    return;
  }

  // Escape: close job detail or blur input
  if (e.key === 'Escape') {
    if (currentJobId) {
      closeJobDetail();
    } else if (inInput) {
      e.target.blur();
    }
    return;
  }
});

// --- Toasts ---

function showToast(message, type) {
  const container = document.getElementById('toasts');
  const toast = document.createElement('div');
  toast.className = 'toast toast-' + (type || 'info');
  toast.textContent = message;
  container.appendChild(toast);
  // Trigger slide-in
  requestAnimationFrame(() => toast.classList.add('visible'));
  setTimeout(() => {
    toast.classList.remove('visible');
    toast.addEventListener('transitionend', () => toast.remove());
  }, 4000);
}

// --- Utilities ---

function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

function formatDate(isoString) {
  if (!isoString) return '-';
  const d = new Date(isoString);
  return d.toLocaleString();
}

// --- Analytics tab ---

let analyticsRange = '';        // '', '24h', '7d', '30d', '90d'
let analyticsModelSort = { col: 'total_cost', dir: -1 };
let analyticsToolSort  = { col: 'calls', dir: -1 };
let analyticsModels = [];       // last-fetched model rows (for re-sort without refetch)
let analyticsTools  = [];       // last-fetched tool rows

// Wire up range pills on first render
(function initAnalyticsPills() {
  document.addEventListener('DOMContentLoaded', () => {
    const pills = document.getElementById('analytics-range-pills');
    if (!pills) return;
    pills.addEventListener('click', (e) => {
      const btn = e.target.closest('.range-pill');
      if (!btn) return;
      pills.querySelectorAll('.range-pill').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      analyticsRange = btn.dataset.range || '';
      loadAnalytics();
    });
  });
})();

async function loadAnalytics() {
  const qs = analyticsRange ? '?range=' + analyticsRange : '';
  try {
    const [models, jobs, tools, chart] = await Promise.all([
      apiFetch('/api/analytics/models' + qs),
      apiFetch('/api/analytics/jobs'   + qs),
      apiFetch('/api/analytics/tools'  + qs),
      apiFetch('/api/analytics/cost-over-time' + qs),
    ]);

    analyticsModels = models.models || [];
    analyticsTools  = tools.tools  || [];

    renderAnalyticsSummary(models);
    renderCostChart(chart.data || []);
    renderJobsSummary(jobs);
    renderAnalyticsTable(analyticsModels);
    renderAnalyticsToolsTable(analyticsTools);

    const updated = document.getElementById('analytics-updated');
    if (updated) updated.textContent = 'Updated ' + new Date().toLocaleTimeString();
  } catch (e) {
    document.getElementById('analytics-summary').innerHTML =
      '<div class="empty-state">Failed to load analytics: ' + escapeHtml(e.message) + '</div>';
  }
}

function renderAnalyticsSummary(data) {
  const totalTokens = data.total_input_tokens + data.total_output_tokens;
  const totalModels = (data.models || []).length;
  const totalCalls  = (data.models || []).reduce((s, m) => s + m.total_calls, 0);
  const ratio = data.total_output_tokens > 0
    ? (data.total_input_tokens / data.total_output_tokens).toFixed(1) + ':1'
    : '—';

  document.getElementById('analytics-summary').innerHTML =
      analyticsCard('Models',        totalModels,                                    '')
    + analyticsCard('Total Calls',   totalCalls.toLocaleString(),                    '')
    + analyticsCard('Input Tokens',  fmtTokens(data.total_input_tokens),             'accent')
    + analyticsCard('Output Tokens', fmtTokens(data.total_output_tokens),            'accent')
    + analyticsCard('Total Tokens',  fmtTokens(totalTokens),                         '')
    + analyticsCard('In / Out Ratio',ratio,                                          '')
    + analyticsCard('Total Cost',    '$' + fmtCost(data.total_cost_usd),             'cost');
}

function renderJobsSummary(jobs) {
  const ratePct = (jobs.success_rate * 100).toFixed(1) + '%';
  const rateClass = jobs.success_rate >= 0.9 ? 'cost' : jobs.success_rate >= 0.7 ? 'accent' : 'error';
  const dur = jobs.avg_duration_secs > 0 ? fmtDuration(jobs.avg_duration_secs) : '—';

  document.getElementById('analytics-jobs-summary').innerHTML =
      analyticsCard('Total Jobs',     jobs.total_jobs.toLocaleString(),    '')
    + analyticsCard('Completed',      jobs.completed_jobs.toLocaleString(),'cost')
    + analyticsCard('Failed',         jobs.failed_jobs.toLocaleString(),   jobs.failed_jobs > 0 ? 'error' : '')
    + analyticsCard('In Progress',    jobs.in_progress_jobs.toLocaleString(), '')
    + analyticsCard('Success Rate',   ratePct,                              rateClass)
    + analyticsCard('Avg Duration',   dur,                                  '')
    + analyticsCard('Total Job Cost', '$' + fmtCost(jobs.total_cost_usd),  'cost');
}

function analyticsCard(label, value, cls) {
  return '<div class="summary-card ' + cls + '">'
    + '<div class="count">' + value + '</div>'
    + '<div class="label">' + label + '</div>'
    + '</div>';
}

// ── Cost-over-time SVG bar chart ──────────────────────────────────────────────

function renderCostChart(data) {
  const svg   = document.getElementById('analytics-cost-chart');
  const empty = document.getElementById('analytics-chart-empty');
  svg.innerHTML = '';

  const nonZero = data.filter(d => parseFloat(d.cost_usd) > 0);
  if (!nonZero.length) {
    svg.style.display = 'none';
    empty.style.display = '';
    return;
  }
  svg.style.display = '';
  empty.style.display = 'none';

  const W = svg.parentElement.clientWidth || 600;
  const H = 120;
  const padL = 52, padR = 12, padT = 10, padB = 28;
  const chartW = W - padL - padR;
  const chartH = H - padT - padB;

  svg.setAttribute('viewBox', '0 0 ' + W + ' ' + H);
  svg.setAttribute('width', '100%');
  svg.setAttribute('height', H);

  const costs = data.map(d => parseFloat(d.cost_usd));
  const maxCost = Math.max(...costs, 0.000001);
  const barW = Math.max(2, Math.floor(chartW / data.length) - 2);

  // Y-axis label
  const yLabel = document.createElementNS('http://www.w3.org/2000/svg', 'text');
  yLabel.setAttribute('x', padL - 4);
  yLabel.setAttribute('y', padT + chartH / 2);
  yLabel.setAttribute('text-anchor', 'end');
  yLabel.setAttribute('dominant-baseline', 'middle');
  yLabel.setAttribute('class', 'chart-axis-label');
  yLabel.textContent = '$' + fmtCost(maxCost.toString());
  svg.appendChild(yLabel);

  // Zero line
  const zeroLine = document.createElementNS('http://www.w3.org/2000/svg', 'line');
  zeroLine.setAttribute('x1', padL);
  zeroLine.setAttribute('x2', padL + chartW);
  zeroLine.setAttribute('y1', padT + chartH);
  zeroLine.setAttribute('y2', padT + chartH);
  zeroLine.setAttribute('class', 'chart-baseline');
  svg.appendChild(zeroLine);

  data.forEach((d, i) => {
    const cost = parseFloat(d.cost_usd);
    const barH = Math.max(1, (cost / maxCost) * chartH);
    const x    = padL + i * (chartW / data.length) + (chartW / data.length - barW) / 2;
    const y    = padT + chartH - barH;

    const rect = document.createElementNS('http://www.w3.org/2000/svg', 'rect');
    rect.setAttribute('x', x.toFixed(1));
    rect.setAttribute('y', y.toFixed(1));
    rect.setAttribute('width', barW);
    rect.setAttribute('height', barH.toFixed(1));
    rect.setAttribute('class', 'chart-bar');
    rect.setAttribute('rx', '2');

    // Tooltip via <title>
    const title = document.createElementNS('http://www.w3.org/2000/svg', 'title');
    title.textContent = d.day + '\n$' + fmtCost(d.cost_usd) + ' · ' + d.call_count + ' calls';
    rect.appendChild(title);
    svg.appendChild(rect);

    // X-axis date label — only show for first, last, and every ~7th bar
    if (i === 0 || i === data.length - 1 || i % 7 === 0) {
      const label = document.createElementNS('http://www.w3.org/2000/svg', 'text');
      label.setAttribute('x', (x + barW / 2).toFixed(1));
      label.setAttribute('y', padT + chartH + 14);
      label.setAttribute('text-anchor', 'middle');
      label.setAttribute('class', 'chart-axis-label');
      label.textContent = d.day.slice(5); // MM-DD
      svg.appendChild(label);
    }
  });
}

// ── Model table ───────────────────────────────────────────────────────────────

// Sortable table headers — delegate from table
document.addEventListener('DOMContentLoaded', () => {
  const modelTable = document.getElementById('analytics-table');
  if (modelTable) {
    modelTable.querySelector('thead').addEventListener('click', (e) => {
      const th = e.target.closest('th[data-col]');
      if (!th) return;
      const col = th.dataset.col;
      if (analyticsModelSort.col === col) analyticsModelSort.dir *= -1;
      else { analyticsModelSort.col = col; analyticsModelSort.dir = -1; }
      renderAnalyticsTable(analyticsModels);
    });
  }

  const toolsTable = document.getElementById('analytics-tools-table');
  if (toolsTable) {
    toolsTable.querySelector('thead').addEventListener('click', (e) => {
      const th = e.target.closest('th[data-col]');
      if (!th) return;
      const col = th.dataset.col;
      if (analyticsToolSort.col === col) analyticsToolSort.dir *= -1;
      else { analyticsToolSort.col = col; analyticsToolSort.dir = -1; }
      renderAnalyticsToolsTable(analyticsTools);
    });
  }
});

function sortedModels(models) {
  const totalCost = models.reduce((s, m) => s + parseFloat(m.total_cost_usd || 0), 0) || 1;
  return [...models].sort((a, b) => {
    let av, bv;
    switch (analyticsModelSort.col) {
      case 'provider':    av = a.provider; bv = b.provider; break;
      case 'model':       av = a.model;    bv = b.model;    break;
      case 'calls':       av = a.total_calls;  bv = b.total_calls; break;
      case 'input':       av = a.total_input_tokens;  bv = b.total_input_tokens;  break;
      case 'output':      av = a.total_output_tokens; bv = b.total_output_tokens; break;
      case 'ratio':       av = a.total_input_tokens  / (a.total_output_tokens  || 1);
                          bv = b.total_input_tokens  / (b.total_output_tokens  || 1); break;
      case 'latency':     av = a.avg_latency_ms ?? -1; bv = b.avg_latency_ms ?? -1; break;
      case 'cost_share':  av = parseFloat(a.total_cost_usd || 0) / totalCost;
                          bv = parseFloat(b.total_cost_usd || 0) / totalCost; break;
      case 'avg_cost':    av = parseFloat(a.avg_cost_per_call_usd || 0);
                          bv = parseFloat(b.avg_cost_per_call_usd || 0); break;
      default:            av = parseFloat(a.total_cost_usd || 0);
                          bv = parseFloat(b.total_cost_usd || 0);
    }
    if (av < bv) return -analyticsModelSort.dir;
    if (av > bv) return  analyticsModelSort.dir;
    return 0;
  });
}

function renderAnalyticsTable(models) {
  const thead = document.querySelector('#analytics-table thead tr');
  const tbody = document.getElementById('analytics-tbody');
  const empty = document.getElementById('analytics-empty');
  tbody.innerHTML = '';

  // Update sort indicators on headers
  if (thead) {
    thead.querySelectorAll('th[data-col]').forEach(th => {
      th.classList.remove('sort-asc', 'sort-desc');
      if (th.dataset.col === analyticsModelSort.col) {
        th.classList.add(analyticsModelSort.dir === 1 ? 'sort-asc' : 'sort-desc');
      }
    });
  }

  if (!models || models.length === 0) {
    empty.style.display = '';
    return;
  }
  empty.style.display = 'none';

  const totalCost = models.reduce((s, m) => s + parseFloat(m.total_cost_usd || 0), 0) || 1;
  const sorted = sortedModels(models);

  for (const m of sorted) {
    const latencyCell = fmtLatencyCell(m.avg_latency_ms, m.p95_latency_ms);
    const shareVal    = parseFloat(m.total_cost_usd || 0) / totalCost;
    const sharePct    = (shareVal * 100).toFixed(1);
    const ratio       = m.total_output_tokens > 0
      ? (m.total_input_tokens / m.total_output_tokens).toFixed(1)
      : '—';

    const tr = document.createElement('tr');
    tr.innerHTML =
        '<td><span class="provider-badge">' + escapeHtml(m.provider) + '</span></td>'
      + '<td class="model-name">' + escapeHtml(m.model) + '</td>'
      + '<td class="num">' + m.total_calls.toLocaleString() + '</td>'
      + '<td class="num">' + fmtTokens(m.total_input_tokens)  + '</td>'
      + '<td class="num">' + fmtTokens(m.total_output_tokens) + '</td>'
      + '<td class="num">' + ratio + '</td>'
      + '<td class="num">' + latencyCell + '</td>'
      + '<td class="num">'
        + '<div class="cost-share-wrap">'
        + '<span class="cost-share-pct">' + sharePct + '%</span>'
        + '<div class="cost-share-bar"><div class="cost-share-fill" style="width:' + sharePct + '%"></div></div>'
        + '</div></td>'
      + '<td class="num cost">' + fmtCostCell(m.avg_cost_per_call_usd) + '</td>'
      + '<td class="num cost">' + fmtCostCell(m.total_cost_usd) + '</td>';
    tbody.appendChild(tr);
  }
}

function fmtLatencyCell(avg, p95) {
  if (avg == null) return '<span class="text-muted">—</span>';
  const cls = avg < 1000 ? 'lat-green' : avg < 3000 ? 'lat-yellow' : 'lat-red';
  let text = '<span class="lat-badge ' + cls + '">' + fmtMs(avg) + '</span>';
  if (p95 != null) text += ' <span class="text-muted p95-label">p95 ' + fmtMs(p95) + '</span>';
  return text;
}

function fmtMs(ms) {
  if (ms >= 1000) return (ms / 1000).toFixed(1) + ' s';
  return Math.round(ms) + ' ms';
}

// ── Tool usage table ──────────────────────────────────────────────────────────

function sortedTools(tools) {
  return [...tools].sort((a, b) => {
    let av, bv;
    switch (analyticsToolSort.col) {
      case 'tool':         av = a.tool_name;        bv = b.tool_name;        break;
      case 'calls':        av = a.total_calls;       bv = b.total_calls;      break;
      case 'success_rate': av = a.success_rate;      bv = b.success_rate;     break;
      case 'avg_ms':       av = a.avg_duration_ms;   bv = b.avg_duration_ms;  break;
      case 'cost':         av = parseFloat(a.total_cost_usd || 0);
                           bv = parseFloat(b.total_cost_usd || 0); break;
      default:             av = a.total_calls; bv = b.total_calls;
    }
    if (av < bv) return -analyticsToolSort.dir;
    if (av > bv) return  analyticsToolSort.dir;
    return 0;
  });
}

function renderAnalyticsToolsTable(tools) {
  const thead = document.querySelector('#analytics-tools-table thead tr');
  const tbody = document.getElementById('analytics-tools-tbody');
  const empty = document.getElementById('analytics-tools-empty');
  tbody.innerHTML = '';

  if (thead) {
    thead.querySelectorAll('th[data-col]').forEach(th => {
      th.classList.remove('sort-asc', 'sort-desc');
      if (th.dataset.col === analyticsToolSort.col) {
        th.classList.add(analyticsToolSort.dir === 1 ? 'sort-asc' : 'sort-desc');
      }
    });
  }

  if (!tools || tools.length === 0) {
    empty.style.display = '';
    return;
  }
  empty.style.display = 'none';

  for (const t of sortedTools(tools)) {
    const ratePct = (t.success_rate * 100).toFixed(1);
    const rateClass = t.success_rate >= 0.95 ? 'sr-green'
                    : t.success_rate >= 0.80  ? 'sr-yellow'
                    : 'sr-red';
    const dur = t.avg_duration_ms > 0 ? fmtMs(t.avg_duration_ms) : '—';
    const cost = parseFloat(t.total_cost_usd || 0);

    const tr = document.createElement('tr');
    tr.innerHTML =
        '<td class="tool-name-cell">' + escapeHtml(t.tool_name) + '</td>'
      + '<td class="num">' + t.total_calls.toLocaleString() + '</td>'
      + '<td class="num">'
        + '<div class="sr-wrap">'
        + '<span class="sr-badge ' + rateClass + '">' + ratePct + '%</span>'
        + '<span class="sr-counts text-muted">('
        + t.successful_calls + '&#x2714; ' + t.failed_calls + '&#x2718;)</span>'
        + '</div></td>'
      + '<td class="num">' + dur + '</td>'
      + '<td class="num cost">' + (cost > 0 ? fmtCostCell(t.total_cost_usd) : '<span class="text-muted">—</span>') + '</td>';
    tbody.appendChild(tr);
  }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/** Format a token count as e.g. "1.2M", "45.3K", "800" */
function fmtTokens(n) {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
  if (n >= 1_000)     return (n / 1_000).toFixed(1) + 'K';
  return String(n);
}

/** Format a cost decimal string, showing enough precision to be meaningful */
function fmtCost(usd) {
  const n = parseFloat(usd);
  if (isNaN(n)) return '0.000000';
  if (n === 0)  return '0.000000';
  if (n >= 1)   return n.toFixed(4);
  return n.toPrecision(4);
}

function fmtCostCell(usd) {
  return '$' + fmtCost(usd);
}

/** Format seconds as "1h 23m", "45s", etc. */
function fmtDuration(secs) {
  if (secs >= 3600) return Math.floor(secs / 3600) + 'h ' + Math.floor((secs % 3600) / 60) + 'm';
  if (secs >= 60)   return Math.floor(secs / 60) + 'm ' + Math.round(secs % 60) + 's';
  return secs.toFixed(1) + 's';
}
