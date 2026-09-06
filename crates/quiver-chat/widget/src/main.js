// quiver-chat widget — dumb renderer. All behavior lives in the engine;
// this page only draws what the WebSocket feed sends.
// No build step: plain ES module served as-is by quiver-chat.

// Twitch retired v2 scale paths AND extensions don't exist on this CDN:
// /emoticons/v1/{id}/2.0 is the live, id-stable pattern (verified 200s).
// Static emotes: numeric ids (Kappa etc.) live on v1. Animated emotes are
// emotesv2_* ids whose animation lives on the v2 path (image/gif, probed
// live: v1/2.0 = 6KB static png, v2/dark/2.0 = 202KB animated gif).
const EMOTE_CDN = "https://static-cdn.jtvnw.net/emoticons/v1/{id}/2.0";
const EMOTE_CDN_V2 = "https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/2.0";

let badgeUrls = {}; // "set_id/version" -> image url
let maxMessages = 30;
let roleCss = {}; // badge set id -> css snippet for message rows
// provider tag -> {emote name -> url}; always fully populated, gating is
// done client-side via emoteFlags so disabled providers STRIP tokens.
let thirdParty = {};
let emoteFlags = { twitch: true, unicode: true, seventv: true, bttv: true, ffz: true };
// Custom badges from meta: { definitions, per_role, per_user }.
let customBadges = { definitions: {}, per_role: {}, per_user: {} };
// Lookup precedence when the same code exists in multiple providers.
const PROVIDER_ORDER = ["ffz", "bttv", "seventv"];
const EXPIRE_FADE_MS = 350;

function loadEmotes() {
  return fetch("/emotes.json")
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((d) => {
      thirdParty = d.providers || {};
      const n = Object.values(thirdParty).reduce((a, m) => a + Object.keys(m).length, 0);
      console.debug("[quiver] third-party emotes loaded:", n);
    });
}

loadEmotes().catch((e) => console.debug("[quiver] no third-party emotes:", e));

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

// Custom badges render FIRST (before Twitch CDN badges), ordered by
// priority ascending. Set = per-role (for each Twitch badge id the
// sender carries) ∪ per-user (by user id or login), deduplicated.
// Hide semantics:
//   per-role hide_native hides ONLY that role's own native badge
//   per-user hide_native hides ALL native badges for the user

function renderCustomBadges(wrap, m) {
  const chosen = new Set();
  for (const b of m.badges || []) {
    const a = customBadges.per_role?.[b.id];
    if (a) for (const id of a.badges || []) chosen.add(id);
  }
  const ua = customBadges.per_user?.[m.user_id] || customBadges.per_user?.[m.user_login];
  if (ua) for (const id of ua.badges || []) chosen.add(id);

  const ordered = [...chosen].sort((a, b) => {
    const pa = customBadges.definitions[a]?.priority ?? 0;
    const pb = customBadges.definitions[b]?.priority ?? 0;
    return pa - pb;
  });

  for (const id of ordered) {
    const d = customBadges.definitions[id];
    if (!d) continue;
    const img = document.createElement("img");
    img.className = "custom-badge";
    img.src = d.url;
    img.alt = d.label || id;
    // Height is in EM units (scales with the widget font like native
    // badges). 1 = native badge size; CSS default handles absent.
    if (d.height > 0) img.style.height = `${d.height}em`;
    if (d.label) img.title = d.label;
    wrap.append(img);
  }
}

function renderBadges(m) {
  const wrap = el("span", "badges");
  renderCustomBadges(wrap, m); // custom always present regardless

  // per-user hide_native → suppress ALL natives for this user.
  const ua = customBadges.per_user?.[m.user_id] || customBadges.per_user?.[m.user_login];
  const hideAllNative = !!ua?.hide_native;

  for (const b of m.badges || []) {
    if (hideAllNative) continue;
    // per-role hide_native → suppress exactly that role's badge, keep all
    // other native badges the user carries.
    if (customBadges.per_role?.[b.id]?.hide_native) continue;
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

// Build the inline GIF element: mp4 → looping muted <video>, else <img>.
function gifElement(g, alt) {
  const isMp4 = /\.mp4(\?|$)/i.test(g.url);
  if (isMp4) {
    const v = document.createElement("video");
    v.className = "gif";
    v.src = g.url;
    v.muted = true;
    v.autoplay = true;
    v.loop = true;
    v.playsInline = true;
    return v;
  }
  const img = document.createElement("img");
  img.className = "gif";
  img.src = g.url;
  img.alt = alt;
  return img;
}

// Split text on emote AND gif ranges: plain slices as text nodes, ranges as
// imgs (emotes) / gif media. Ranges are char-based, end-exclusive.
function renderText(m) {
  const wrap = el("span", "text");
  const ranges = [
    ...(m.emotes || []).map((e) => ({ ...e, kind: "emote" })),
    ...(m.gifs || []).map((g) => ({ ...g, kind: "gif" })),
  ].sort((a, b) => a.start - b.start);

  // Reply threads: Twitch prepends "@<parent> " to the message. We render
  // the parent above, so hide that prefix (display:none) — ranges for
  // emotes/gifs index against the ORIGINAL text, so the segment must stay
  // in place, invisible, not sliced away. Only strip when the prefix
  // matches the actual parent (never mangle a different @mention).
  let cursor = 0;
  if (m.reply_to && m.text.startsWith("@")) {
    const pre = /^@(\S+)/.exec(m.text);
    if (pre) {
      const id = pre[1].toLowerCase();
      const matchesParent =
        id === m.reply_to.user_login.toLowerCase() ||
        id === m.reply_to.display_name.toLowerCase();
      if (matchesParent) {
        let end = pre[0].length;
        if (m.text[end] === " ") end++;
        const hidden = document.createElement("span");
        hidden.className = "reply-mention-prefix";
        hidden.textContent = m.text.slice(0, end);
        wrap.append(hidden);
        cursor = end;
      }
    }
  }

  for (const r of ranges) {
    if (r.start < cursor || r.end > m.text.length) continue; // malformed/overlap
    if (r.start > cursor) appendTokens(wrap, m.text.slice(cursor, r.start));
    if (r.kind === "emote") {
      if (emoteFlags.twitch) {
        const img = document.createElement("img");
        img.className = "emote";
        const id = encodeURIComponent(r.id);
        if (r.id.startsWith("emotesv2_")) {
          // Animated variant; static v1 as fallback if v2 is unavailable.
          img.src = EMOTE_CDN_V2.replace("{id}", id);
          img.onerror = () => {
            img.onerror = null;
            img.src = EMOTE_CDN.replace("{id}", id);
          };
        } else {
          img.src = EMOTE_CDN.replace("{id}", id);
        }
        img.alt = m.text.slice(r.start, r.end);
        wrap.append(img);
      } else if (!emoteFlags.unicode) {
        // Both disabled: the code counts as an emoji — strip it.
      }
    } else {
      wrap.append(gifElement(r, m.text.slice(r.start, r.end)));
    }
    cursor = r.end;
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
    // Unified card: header + message share one background box.
    row.classList.add("has-reply");
    const header = el("div", "reply");
    header.append(
      el("span", "reply-arrow", "➚"),
      el("span", "reply-user", m.reply_to.display_name),
      el("span", "reply-text", m.reply_to.text),
    );
    // Click jumps to nothing (parent may be expired) but title hints.
    header.title = m.reply_to.text;
    row.append(header);
  }

  // Name + badges + separator + text share ONE wrapping container so
  // word-wrap happens INSIDE the body (beside the name) and badges can
  // never wrap away from the username — they are contiguous inline
  // content with no break point between them.
  const body = el("span", "body");
  body.append(renderBadges(m));
  const user = el("span", "user", m.display_name);
  if (m.color) user.style.color = m.color;
  body.append(user);

  if (m.is_action) {
    // /me lines: whole line italic in the sender's color, no separator.
    row.classList.add("action");
    if (m.color) row.style.color = m.color;
  } else {
    body.append(el("span", "sep", ":"));
  }
  body.append(renderText(m));
  row.append(body);
  watchHeight(row); // re-measure when media loads / role CSS resizes
  return row;
}

function trimTo(max) {
  // Count-cap is a SANITY layer: only .msg nodes count (event banners
  // are never trimmed here), newest message always kept.
  let msgs = 0;
  for (const child of chat.children) {
    if (child.classList.contains("msg")) msgs++;
  }
  if (msgs <= max) return;
  // Remove oldest .msg nodes first (banners untouched).
  for (const child of chat.children) {
    if (msgs <= max) break;
    if (child.classList.contains("msg")) {
      unwatchHeight(child);
      child.remove();
      msgs--;
    }
  }
}

function expire(ids) {
  for (const id of ids) {
    const node = chat.querySelector(`[data-id="${CSS.escape(id)}"]`);
    if (!node) continue;
    node.classList.add("expiring");
    setTimeout(() => {
      unwatchHeight(node);
      node.remove();
    }, EXPIRE_FADE_MS);
  }
}

// A moderator/streamer deleted the message — hide it outright (no fade).
function removeMessage(id) {
  const node = chat.querySelector(`[data-id="${CSS.escape(id)}"]`);
  if (node) {
    unwatchHeight(node);
    node.remove();
  }
}

// ---- height-based overflow management ----------------------------------
// Layered on top of the count cap: prune mode removes oldest .msg nodes
// until the container fits; scroll mode pins a scrollable chat bottom.
// Guard rails: newest message never pruned, .event banners never pruned.

let overflowMode = "prune"; // from meta.theme
const overflowWatch = new ResizeObserver(() => settleOverflow());
// Bounded history of rendered wire messages, so hot config reloads can
// re-render existing rows (badge maps / heights / emote flags change
// live — without this, only NEW rows would show them).
let history = [];

function applyOverflowMode(mode) {
  overflowMode = mode === "scroll" ? "scroll" : "prune";
  chat.classList.toggle("overflow-scroll", overflowMode === "scroll");
}

function watchHeight(node) {
  overflowWatch.observe(node);
}

function unwatchHeight(node) {
  overflowWatch.unobserve(node);
}

// Re-render all history rows (e.g. after a hot config reload changed badge
// maps / heights / emote flags). Event banners are transient DOM-only
// nodes and are preserved across the redraw.
function rerender() {
  const banners = [...chat.querySelectorAll(".event")];
  for (const c of chat.children) unwatchHeight(c);
  chat.replaceChildren(...history.map(renderMessage));
  // Banners were collected in DOM order (newest first — showEvent prepends),
  // so re-prepending in that order must be REVERSED, or the stacking flips
  // (oldest on top) after every hot config reload.
  for (const b of banners.reverse()) chat.prepend(b);
  requestAnimationFrame(settleOverflow);
}

function countMsgs() {
  let n = 0;
  for (const c of chat.children) if (c.classList.contains("msg")) n++;
  return n;
}

function settleOverflow() {
  // Scroll mode: pin to bottom (only when already pinned / overflowing).
  if (overflowMode === "scroll") {
    const pinned =
      chat.scrollTop + chat.clientHeight >= chat.scrollHeight - 1 ||
      chat.scrollTop === 0;
    if (pinned && chat.scrollHeight > chat.clientHeight) {
      chat.scrollTop = chat.scrollHeight;
    }
    return;
  }

  // Prune mode: while content overflows AND more than one .msg exists,
  // drop the oldest .msg (banners and the newest message survive).
  let guard = 100; // bound the loop (safety against pathological heights)
  while (
    chat.scrollHeight > chat.clientHeight + 1 &&
    countMsgs() > 1 &&
    guard-- > 0
  ) {
    for (const c of chat.children) {
      if (c.classList.contains("msg")) {
        unwatchHeight(c);
        c.remove();
        break;
      }
    }
  }
}

function applyMeta(meta) {
  if (!meta) return;
  if (meta.badges) badgeUrls = meta.badges;
  if (meta.emote_flags) emoteFlags = meta.emote_flags;
  if (meta.custom_badges) customBadges = meta.custom_badges;
  // Role map drives BOTH class assignment on new rows and the injected
  // sheet — forgetting to store it here meant styles existed but no row
  // ever matched them.
  roleCss = meta.role_css || {};
  if (meta.theme) {
    if (meta.theme.font_size_px) chat.style.fontSize = `${meta.theme.font_size_px}px`;
    if (meta.theme.max_messages) maxMessages = meta.theme.max_messages;
    if (meta.theme.overflow_mode) applyOverflowMode(meta.theme.overflow_mode);
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
      history = wire.messages || [];
      for (const c of chat.children) unwatchHeight(c);
      chat.replaceChildren(...history.map(renderMessage));
      requestAnimationFrame(settleOverflow);
      break;
    case "message":
      history.push(wire.message);
      if (history.length > maxMessages) history.shift();
      chat.append(renderMessage(wire.message));
      trimTo(maxMessages);
      requestAnimationFrame(settleOverflow);
      break;
    case "expire":
      const expired = new Set(wire.ids || []);
      history = history.filter((m) => !expired.has(m.id));
      expire(wire.ids || []);
      break;
    case "delete":
      history = history.filter((m) => m.id !== wire.id);
      removeMessage(wire.id);
      break;
    case "config":
      // Hot reload: theme/badges/emotes/CSS changed server-side. Global
      // state updates via applyMeta, then RE-RENDER existing rows so
      // badge maps, custom-badge heights, and emote flags take effect
      // live (this was the missing half — rows only updated on F5).
      applyMeta(wire.meta);
      rerender();
      // A channel swap pushes fresh badges inside meta, but the third-party
      // emote map is served separately (/emotes.json) — refetch it on every
      // config frame so a swapped channel doesn't keep rendering the old
      // channel's emotes. Re-render again once the fresh map is in.
      loadEmotes().then(rerender).catch((e) => console.debug("[quiver] emotes refetch failed:", e));
      break;
    case "event":
      showEvent(wire.event);
      break;
    case "clear":
      // Channel swapped: history wiped server-side.
      history = [];
      for (const c of chat.children) unwatchHeight(c);
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
// Container resize (OBS changing source size/DPI) re-triggers overflow logic.
overflowWatch.observe(chat);
applyOverflowMode("prune"); // applied from meta on first snapshot
connect();
