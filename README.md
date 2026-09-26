# Agent

A minimal desktop chat UI for OpenCode V2, built with Slint and Rust.

Install **OpenCode V2** and Node.js 22+, then configure your model/provider in OpenCode. From this project's directory:

```sh
cd bridge && npm ci
cd .. && cargo run
```

The app starts with the directory where you launch it. Use **New project** to register other folders through the system folder picker. New chats use the last selected project, and you can change the project in an empty chat before sending its first message. Each chat then keeps its project's working directory. The app connects to OpenCode's shared background service through `@opencode/client`; the service owns the session history, agent tools, models, and permissions. Install OpenCode V2 separately: an OpenCode V1 CLI is not compatible.

Use **New chat** to start a session. Middle-click a sidebar chat to hide it without deleting its OpenCode history. The model selector shows enabled OpenCode models; the adjacent selector shows the model's available variants (including `default`). Permissions require an explicit **Allow** (once) or **Deny**. For a pending form, enter an answer as a JSON object keyed by the displayed field names, such as `{"name":"value"}`.

Sidebar metadata is stored under `opencode-agent-desktop` in local app data (`%LOCALAPPDATA%` on Windows, `$XDG_DATA_HOME` or `~/.local/share` elsewhere). Old `pi-agent-desktop` files are left untouched but not shown. The desktop app requires `bridge/node_modules` from `npm ci` at runtime.
