// quiver-chat widget — dumb renderer. All behavior lives in the engine;
// this page only draws what the WebSocket feed sends.
// No build step: plain ES module served as-is by quiver-chat.

// Twitch retired v2 scale paths AND extensions don't exist on this CDN:
// /emoticons/v1/{id}/2.0 is the live, id-stable pattern (verified 200s).
const EMOTE_CDN = "https://static-cdn.jtvnw.net/emoticons/v1/{id}/2.0";

let badgeUrls = {}; // "set_id/version" -> image url
let maxMessages = 30;
let roleCss = {}; // badge set id -> css snippet for message rows
// provider tag -> {emote name -> url}; always fully populated, gating is
// done client-side via emoteFlags so disabled providers STRIP tokens.
let thirdParty = {};
let emoteFlags = { twitch: true, unicode: true, seventv: true, bttv: true, ffz: true };
// Lookup precedence when the same code exists in multiple providers.
const PROVIDER_ORDER = ["ffz", "bttv", "seventv"];
const EXPIRE_FADE_MS = 350;

fetch("/emotes.json")
  .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
  .then((d) => {
    thirdParty = d.providers || {};
    const n = Object.values(thirdParty).reduce((a, m) => a + Object.keys(m).length, 0);
    console.debug("[quiver] third-party emotes loaded:", n);
  })
  .catch((e) => console.debug("[quiver] no third-party emotes:", e));

function lookupThirdParty(token) {
  for (const p of PROVIDER_ORDER) {
    if (!emoteFlags[p]) continue; // provider disabled
    const url = thirdParty[p]?.[token];
    if (url) return url;
  }
  return null;
}

// Unicode pictograph runs (incl. ZWJ sequences + variation selectors).
const UNICODE_EMOJI_RE = /[\p{Extended_Pictographic}\u{1F3FB}-\u{1F3FF}\uFE0F\u200D]+/gu;

function appendTokens(wrap, text) {
  if (!emoteFlags.unicode) {
    // Disabled unicode = strip emoji characters from text entirely.
    text = text.replace(UNICODE_EMOJI_RE, "");
    if (!text.trim()) return;
  }
  for (const part of text.split(/(\s+)/)) {
    if (!part) continue;
    if (/^\s+$/.test(part)) {
      wrap.append(document.createTextNode(part));
      continue;
    }
    const url = lookupThirdParty(part);
    if (url) {
      const img = document.createElement("img");
      img.className = "emote";
      img.src = url;
      img.alt = part;
      wrap.append(img);
    } else {
      wrap.append(document.createTextNode(part));
    }
  }
}
function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

function renderBadges(m) {
  const wrap = el("span", "badges");
  for (const b of m.badges || []) {
    const url = badgeUrls[`${b.id}/${b.version}`];
    if (!url) continue; // unknown badge — skip silently
    const img = document.createElement("img");
    img.className = "badge";
    img.src = url;
    img.alt = b.id;
    wrap.append(img);
  }
  return wrap;
}

// Split text on emote ranges: plain slices as text nodes, emote spans as imgs.
function renderText(m) {
  const wrap = el("span", "text");
  const emotes = [...(m.emotes || [])].sort((a, b) => a.start - b.start);
  let cursor = 0;
  for (const e of emotes) {
    if (e.start < cursor || e.end > m.text.length) continue; // malformed range
    if (e.start > cursor) appendTokens(wrap, m.text.slice(cursor, e.start));
    if (emoteFlags.twitch) {
      const img = document.createElement("img");
      img.className = "emote";
      img.src = EMOTE_CDN.replace("{id}", encodeURIComponent(e.id));
      img.alt = m.text.slice(e.start, e.end);
      wrap.append(img);
    } else if (!emoteFlags.unicode) {
      // Both disabled: the code counts as an emoji — strip it.
    }
    cursor = e.end;
  }
  if (cursor < m.text.length) appendTokens(wrap, m.text.slice(cursor));
  return wrap;
}

function renderMessage(m) {
  const row = el("div", "msg");
  row.dataset.id = m.id;

  // Per-role classes: one per configured badge id the sender carries.
  for (const b of m.badges || []) {
    if (roleCss[b.id]) row.classList.add(`role-${CSS.escape(b.id)}`);
  }

  // Reply thread header: compact strip above the row content.
  if (m.reply_to) {
    const header = el("div", "reply");
    header.append(
      el("span", "reply-arrow", "↩"),
      el("span", "reply-user", m.reply_to.display_name),
      el("span", "reply-text", m.reply_to.text),
    );
    // Click jumps to nothing (parent may be expired) but title hints.
    header.title = m.reply_to.text;
    row.append(header);
  }

  row.append(renderBadges(m));

  const user = el("span", "user", m.display_name);
  if (m.color) user.style.color = m.color;
  row.append(user);

  if (m.is_action) {
    // /me lines: whole line italic in the sender's color, no separator.
    row.classList.add("action");
    if (m.color) row.style.color = m.color;
    row.append(renderText(m));
  } else {
    row.append(el("span", "sep", ":"));
    row.append(renderText(m));
  }
  return row;
}

function trimTo(max) {
  while (chat.children.length > max) chat.firstElementChild?.remove();
}

function expire(ids) {
  for (const id of ids) {
    const node = chat.querySelector(`[data-id="${CSS.escape(id)}"]`);
    if (!node) continue;
    node.classList.add("expiring");
    setTimeout(() => node.remove(), EXPIRE_FADE_MS);
  }
}

function applyMeta(meta) {
  if (!meta) return;
  if (meta.badges) badgeUrls = meta.badges;
  if (meta.emote_flags) emoteFlags = meta.emote_flags;
  // Role map drives BOTH class assignment on new rows and the injected
  // sheet — forgetting to store it here meant styles existed but no row
  // ever matched them.
  roleCss = meta.role_css || {};
  if (meta.theme) {
    if (meta.theme.font_size_px) chat.style.fontSize = `${meta.theme.font_size_px}px`;
    if (meta.theme.max_messages) maxMessages = meta.theme.max_messages;
  }
  applyUserStyles(meta.role_css, meta.custom_css);
}

// Injected stylesheet order: builtin < role-css < custom-css.
// At equal specificity later sheets win — so per-role snippets beat
// built-ins, and the user's global custom_css beats everything.
function applyUserStyles(roleCssMap, customCss) {
  roleCssMap = roleCssMap || {};
  let roleEl = document.getElementById("role-css");
  let customEl = document.getElementById("custom-css");

  const roleText = Object.values(roleCssMap).join("\n");
  if (!roleText) roleEl?.remove();
  else {
    if (!roleEl) {
      roleEl = document.createElement("style");
      roleEl.id = "role-css";
      document.head.append(roleEl);
    }
    roleEl.textContent = roleText;
  }

  if (!customCss) customEl?.remove();
  else {
    if (!customEl) {
      customEl = document.createElement("style");
      customEl.id = "custom-css";
      document.head.append(customEl);
    }
    customEl.textContent = customCss;
  }

  // Re-assert order — but ONLY between nodes still CONNECTED to the DOM.
  // A removed node leaves its variable truthy; append() would resurrect it
  // (this exact bug: outline survived custom_css=None).
  if (roleEl?.isConnected && customEl?.isConnected) {
    document.head.append(roleEl, customEl);
  }
}

const EVENT_STYLES = {
  sub: { icon: "★", bg: "rgba(145,70,255,.25)", border: "#9146FF" },
  gift_sub: { icon: "🎁", bg: "rgba(145,70,255,.18)", border: "#9146FF" },
  mystery_gift: { icon: "🎁🎁", bg: "rgba(145,70,255,.22)", border: "#9146FF" },
  raid: { icon: "⚔", bg: "rgba(255,140,0,.22)", border: "#FF8C00" },
};
const EVENT_BANNER_MS = 8000;

function showEvent(ev) {
  const style = EVENT_STYLES[ev.kind];
  if (!style) return;
  const banner = el("div", "event");
  banner.style.background = style.bg;
  banner.style.borderColor = style.border;

  let text = "";
  switch (ev.kind) {
    case "sub":
      text = ev.is_resub
        ? `${ev.display_name} resubbed (${ev.cumulative_months} mo${ev.streak_months ? `, ${ev.streak_months} streak` : ""})`
        : `${ev.display_name} subscribed!`;
      if (ev.message) banner.title = ev.message;
      break;
    case "gift_sub":
      text = `${ev.gifter_display_name || "An anonymous gifter"} gifted a sub to ${ev.recipient_display_name}`;
      break;
    case "mystery_gift":
      text = `${ev.gifter_display_name || "An anonymous gifter"} is gifting ${ev.mass_gift_count} subs!`;
      break;
    case "raid":
      text = `${ev.from_display_name} raided with ${ev.viewers} viewers!`;
      break;
  }
  banner.append(el("span", "event-icon", style.icon), el("span", "event-text", text));

  // Banners ride ABOVE the chat flow and leave on their own.
  chat.prepend(banner);
  setTimeout(() => {
    banner.classList.add("expiring");
    setTimeout(() => banner.remove(), EXPIRE_FADE_MS);
  }, EVENT_BANNER_MS);
}

function handle(wire) {
  switch (wire.type) {
    case "snapshot":
      applyMeta(wire.meta);
      chat.replaceChildren(...(wire.messages || []).map(renderMessage));
      break;
    case "message":
      chat.append(renderMessage(wire.message));
      trimTo(maxMessages);
      break;
    case "expire":
      expire(wire.ids || []);
      break;
    case "config":
      // Hot reload: theme/badges/custom_css changed server-side.
      applyMeta(wire.meta);
      break;
    case "event":
      showEvent(wire.event);
      break;
    case "clear":
      // Channel swapped: history wiped server-side.
      chat.replaceChildren();
      break;
    case "reload":
      // Frontend files changed on disk. Guard against rapid loops: a page
      // that just booted ignores reload frames for a moment.
      if (Date.now() - bootMs > 1500) location.reload();
      break;
  }
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);

  ws.onmessage = (ev) => {
    try {
      handle(JSON.parse(ev.data));
    } catch (err) {
      // Surface handler failures — a silent catch here once hid a whole
      // class of "styles don't apply" bugs.
      console.error("[quiver] frame handling failed:", err);
    }
  };
  ws.onclose = () => {
    // OBS browser sources survive reconnects; retry with backoff.
    setTimeout(connect, 2000);
  };
}

const bootMs = Date.now();
const chat = document.getElementById("chat");
connect();
