// quiver-chat widget — dumb renderer. All behavior lives in the engine;
// this page only draws what the WebSocket feed sends.

interface RenderedMessage {
  id: string;
  user_login: string;
  display_name: string;
  color: string | null;
  text: string;
}

type Wire =
  | { type: "snapshot"; messages: RenderedMessage[] }
  | { type: "message"; message: RenderedMessage };

const chat = document.getElementById("chat") as HTMLDivElement;

function renderMessage(m: RenderedMessage): HTMLDivElement {
  const row = document.createElement("div");
  row.className = "msg";
  row.dataset.id = m.id;

  const user = document.createElement("span");
  user.className = "user";
  user.textContent = m.display_name;
  if (m.color) user.style.color = m.color;

  const text = document.createElement("span");
  text.className = "text";
  text.textContent = m.text; // textContent, never innerHTML — no injection

  row.append(user, text);
  return row;
}

function trimTo(max: number): void {
  while (chat.children.length > max) chat.firstElementChild?.remove();
}

function handle(wire: Wire): void {
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

function connect(): void {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);

  ws.onmessage = (ev) => {
    try {
      handle(JSON.parse(ev.data) as Wire);
    } catch {
      // malformed frame — ignore, next snapshot resyncs
    }
  };
  ws.onclose = () => {
    // OBS browser sources survive reconnects; retry with backoff.
    setTimeout(connect, 2000);
  };
}

connect();
