// Chat bridge: mounts the full Linggen chat panel as an iframe.
// Drop-in replacement for LinggenUI.mount() — same API surface.
// The iframe loads /embed?skill=... which provides the complete chat UI
// (markdown, tool activity, permissions, plans, subagents).

import { createSession, removeSkillSession } from './api.js';

/**
 * Mount a chat iframe into the given element.
 * @param {HTMLElement} el
 * @param {{
 *   skillName: string,
 *   agentId?: string,
 *   modelId?: string,
 *   title?: string,
 *   sessionId?: string,
 *   onSessionCreated?: (sid: string) => void,
 *   onStreamToken?: (fullText: string) => void,
 *   onStreamEnd?: (text: string) => void,
 *   onContentBlock?: (payload: { phase: string, tool?: string, args?: string, blockId?: string, output?: string }) => void,
 * }} options
 * @returns {Promise<ChatInstance>}
 */
async function mount(el, options) {
  const {
    skillName,
    agentId = 'ling',
    onSessionCreated,
    onStreamToken,
    onStreamEnd,
  } = options;

  let modelId = options.modelId || '';
  let sessionId = options.sessionId || null;
  let streamBuffer = '';

  // Detect remote mode via meta tag (injected by ConnectPage)
  const instanceMeta = document.querySelector('meta[name="linggen-instance"]');
  const isRemote = !!instanceMeta;

  // Create a skill-bound session if none provided
  if (!sessionId) {
    const data = await createSession(`${skillName} session`, skillName);
    sessionId = data.id;
    if (onSessionCreated) onSessionCreated(sessionId);
  }

  // Build iframe URL
  const params = new URLSearchParams({
    skill: skillName,
    session: sessionId,
    hide_toolbar: '1',
  });
  if (modelId) params.set('model', modelId);

  let iframeSrc;
  if (isRemote) {
    // Remote: load embed chat via connect page (establishes its own WebRTC).
    // The connect page forwards the query string to /embed on the local peer.
    const instanceId = instanceMeta.getAttribute('content') || '';
    const relayOrigin = (document.querySelector('meta[name="linggen-relay-origin"]') || {}).content || window.location.origin;
    iframeSrc = `${relayOrigin}/app/connect/${instanceId}?${params.toString()}&entry=embed`;
  } else {
    // Local: load embed chat directly from the local server
    iframeSrc = `/embed?${params.toString()}`;
  }

  const iframe = document.createElement('iframe');
  iframe.src = iframeSrc;
  iframe.style.cssText = 'width:100%;height:100%;border:none;';
  iframe.allow = 'clipboard-write';
  el.appendChild(iframe);

  // Resolve the iframe's origin so we can constrain postMessage targetOrigin
  // to it (instead of '*') and reject inbound messages from any other origin.
  // For relative iframe URLs (local mode), this resolves to the parent origin.
  const iframeOrigin = new URL(iframe.src, window.location.href).origin;

  // Listen for events from the iframe
  function handleMessage(e) {
    if (e.origin !== iframeOrigin) return;
    if (e.data?.type !== 'linggen-skill-event') return;
    const { event, payload } = e.data;
    switch (event) {
      case 'stream_token':
        streamBuffer += payload?.text || '';
        if (onStreamToken) onStreamToken(streamBuffer);
        break;
      case 'stream_end':
        // Prefer streamBuffer (raw tokens) over payload.text (may be rendered/stripped)
        if (onStreamEnd) onStreamEnd(streamBuffer || payload?.text || '');
        streamBuffer = '';
        break;
      case 'content_block':
        if (options.onContentBlock) options.onContentBlock(payload);
        break;
      case 'session_created':
        if (payload?.sessionId) {
          sessionId = payload.sessionId;
          if (onSessionCreated) onSessionCreated(sessionId);
        }
        break;
    }
  }
  window.addEventListener('message', handleMessage);

  // Wait for iframe to load
  await new Promise((resolve) => {
    iframe.addEventListener('load', resolve, { once: true });
  });

  /** Send a message to the chat (posts to iframe which triggers chat submit). */
  function send(text) {
    streamBuffer = '';
    iframe.contentWindow?.postMessage({ type: 'linggen-skill', action: 'send', payload: { text } }, iframeOrigin);
  }

  /** Send a message to the agent without showing it in the chat UI. */
  function sendHidden(text) {
    streamBuffer = '';
    iframe.contentWindow?.postMessage({ type: 'linggen-skill', action: 'send_hidden', payload: { text } }, iframeOrigin);
  }

  /** Add a local-only message to the chat display. */
  function addMessage(role, text) {
    const mappedRole = (role === 'ai' || role === 'assistant') ? 'assistant' : role;
    iframe.contentWindow?.postMessage({ type: 'linggen-skill', action: 'add_message', payload: { role: mappedRole, text } }, iframeOrigin);
  }

  return {
    send,
    sendHidden,
    addMessage,
    destroy() {
      window.removeEventListener('message', handleMessage);
      iframe.remove();
    },
    async deleteSession() {
      if (sessionId) {
        try { await removeSkillSession(skillName, sessionId); } catch { /* ignore */ }
        sessionId = null;
      }
    },
    getSessionId() { return sessionId; },
    setOptions(opts) {
      if (opts.modelId) modelId = opts.modelId;
    },
  };
}

window.LinggenUI = { mount };
