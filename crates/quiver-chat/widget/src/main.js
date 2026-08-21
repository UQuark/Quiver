// quiver-chat widget — dumb renderer. All behavior lives in the engine;
// this page only draws what the WebSocket feed sends.
// No build step: plain ES module served as-is by quiver-chat.

function renderMessage(m) {
  const row = document.createElement("div");
  row.className = "msg";
  row.dataset.id = m.id;

  const user = document.createElement("span");
  user.className = "user";
  user.textContent = m.display_name;
  if (m.color) user.style.color = m.color;

  const text = document.createElement("span");
  text.className = "text";
  text.textContent = m.text; // never innerHTML — no injection

  row.append(user, text);
  return row;
}

function trimTo(max) {
  while (chat.children.length > max) chat.firstElementChild?.remove();
}

function handle(wire) {
  switch (wire.type) {
    case "snapshot":
      chat.replaceChildren(...wire.messages.map(renderMessage));
      break;
    case "message":
      chat.append(renderMessage(wire.message));
      trimTo(30);
      break;
  }
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);

  ws.onmessage = (ev) => {
    try {
      handle(JSON.parse(ev.data));
    } catch {
      // malformed frame — ignore, next snapshot resyncs
    }
  };
  ws.onclose = () => {
    // OBS browser sources survive reconnects; retry with backoff.
    setTimeout(connect, 2000);
  };
}

const chat = document.getElementById("chat");
connect();
