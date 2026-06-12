# 🌉 MCP Server Bridge (V17)

A high-performance **Rust bridge** that exposes stdio-based [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) servers over the network using **HTTP + SSE** (Server-Sent Events).

Designed specifically to make the **Chrome DevTools MCP server** accessible to remote AI agents and IDEs, bypassing local Docker requirements on the client-side.

---

## 🚀 Version 17 Core Features

- **Stdin EOF Race Fix**: `chrome-devtools-mcp` v1.1.0+ exits immediately when its stdin reaches EOF ([#2117](https://github.com/ChromeDevTools/chrome-devtools-mcp/issues/2117)). The writer task previously dropped `child_stdin` when the internal mpsc channel returned `None` during session teardown, sending a spurious EOF and killing the child. Fixed: the writer now holds stdin open and waits for the explicit cancel signal before breaking.
- **Proactive Zombie Session Cleanup**: When the child process exits unexpectedly (stdout EOF), the reader task now cancels the session token immediately. Previously the dead session lingered in the store until the SSE client timed out, causing the MCP client to hang silently. Fixed: the watcher task fires instantly and removes the zombie entry, so the client sees the stream end and can reconnect.
- **Pinned `chrome-devtools-mcp` version**: `Dockerfile.devtool` now pins to `1.2.0` instead of `@latest` to prevent silent breakage from upstream transport changes.

### Previously Introduced (V16)

- **Guaranteed Orphan Cleanup**: On session teardown the bridge executes `docker exec <container> pkill -f chrome-devtools-mcp` to ensure any detached Node MCP process is terminated.
- **No More "Node Army"**: Prevents accumulation of idle `chrome-devtools-mcp` processes even if clients crash during tool execution.
- **Fast Recovery**: Cleanup is lightweight (no container restart), so reconnects are immediate and container memory stabilizes.

### Previously Introduced (V11)

- **Deterministic Session Cleanup**: Fixes an SSE lifecycle edge case where client disconnects could leave orphan `chrome-devtools-mcp` Node processes running. Sessions are now guaranteed to drop when the SSE stream ends, ensuring the child process is killed.
- **No More "Node Army"**: Prevents accumulation of idle MCP processes when clients reconnect repeatedly.

### Previously Introduced (V10)

- **SSE Multiplexing**: Spawns unique, isolated `docker exec` sessions per client.
- **Auto-Cleanup**: When a session ends, the bridge force-cleans any orphan `chrome-devtools-mcp` processes inside the container using `docker exec pkill -f chrome-devtools-mcp`, preventing the classic "node army" problem.
- **Hyper-Robust Session Handling**: Supports session IDs via `X-Session-ID` headers, query parameters (`?session_id=...`), or the standard MCP `endpoint` event.
- **Cross-Platform Readiness**: Full support for Windows (Host) to Docker (Linux) communication.
- **Permissive CORS**: All origins, methods, and headers allowed — works from `localhost`, LAN IPs, and remote machines without CORS preflight failures.
- **Chrome Host-Header Fix**: Automatically resolves `host.docker.internal` (and any hostname) to its IP at startup. Chrome's DevTools Protocol HTTP server rejects non-IP `Host` headers with HTTP 500 — this fix ensures tool execution always works.
- **Smart 410 Gone**: Stale session POSTs return `410 Gone` + `X-MCP-Reconnect: true` instead of a confusing `404`, so clients know to re-establish the SSE stream.

---

## 🛠 Prerequisites

### 1. Configure Chrome (On Windows/Host)

Chrome's remote debugger is strict. You **must** launch it with these flags for Docker to reach it:

```cmd
start chrome.exe --remote-debugging-port=9322 --remote-allow-origins=* --remote-debugging-address=0.0.0.0
```

> [!IMPORTANT]
> Close all existing Chrome windows before running this command, or the new flags will be ignored.

### 2. Networking

- **Docker to Host**: The bridge uses `host.docker.internal`, which is automatically resolved to an IP at startup.
- **Client to Bridge**: Both `localhost:3000` (local machine) and `<LAN-IP>:3000` (remote machines) work.

---

## 📦 Deployment

### Via Docker Compose (Recommended)

```bash
docker compose down
docker image rm mcp-server-bridge  # Clear cache
docker compose up -d --build mcp-bridge
```

### ⚠️ Important: Memory Limits

Due to memory accumulation over time in the underlying `chrome-devtools-mcp` Node process, it is highly recommended to enforce memory limits within your `docker-compose.yml`.

Example snippet:

```yaml
  mcp-server:
    image: chrome-devtools-mcp
    deploy:
      resources:
        limits:
          memory: 2048M  # Caps memory to 2GB to prevent background Node leak from crashing host
```

The Rust bridge itself is extremely lightweight and can be safely capped at `256M`.

### Environment Variables

| Variable | Default | Purpose |
| --- | --- | --- |
| `MCP_ARGS` | `""` | Arguments passed to the inner process. Example: `--browser-url=http://host.docker.internal:9322` |
| `MCP_CONTAINER` | `chrome-mcp-server` | The target container for `docker exec`. |
| `PORT` | `3000` | Bridge listening port. |

---

## 🤖 Client Configuration

Depending on your client app, use one of the following JSON blocks in your `mcp_config.json`:

### Local (same machine as the bridge)

```json
{
  "mcpServers": {
    "chrome-devtools": {
      "type": "sse",
      "url": "http://localhost:3000/mcp"
    }
  }
}
```

### Remote (different machine on LAN)

```json
{
  "mcpServers": {
    "chrome-devtools": {
      "type": "sse",
      "url": "http://<windows-host-ip>:3000/mcp"
    }
  }
}
```

### Strict Clients (e.g. Cursor / Antigravity IDE)

If you get a `"serverURL missing"` error, use the `serverURL` key:

```json
{
  "mcpServers": {
    "chrome-devtools": {
      "type": "sse",
      "serverURL": "http://<windows-host-ip>:3000/mcp"
    }
  }
}
```

---

## 🔍 Verification & Debugging

### Step 1: Health Check

```bash
curl http://localhost:3000/health
# Response: {"status":"ok"}
```

### Step 2: Stream Test

```bash
curl -v -N -H "Accept: text/event-stream" http://localhost:3000/mcp
```

**Success:** You should see `event: endpoint` with an absolute URL containing a `session_id`.

### Step 3: Check Startup Logs

```bash
docker compose logs -f mcp-bridge
```

### Step 3.5: Check if Chrome running directly devtool container

```bash
docker exec chrome-mcp-server node -e "const dns=require('dns');dns.lookup('host.docker.internal',(_,ip)=>{require('http').get('http://'+ip+':9322/json/version',r=>{let d='';r.on('data',c=>d+=c);r.on('end',()=>console.log(d))}).on('error',e=>console.log('ERROR:',e.message))})"
```

```bash
docker compose logs -f chrome-mcp-server
```

**Confirm V17 is running:**

```text
@@@ MCP BRIDGE VERSION 17 - STDIN-EOF FIX FOR chrome-devtools-mcp v1.1.0+ @@@
Resolved hostname to IP  original=http://host.docker.internal:9322  resolved=http://192.168.65.254:9322
mcp-server-bridge listening  addr=0.0.0.0:3000  mcp_args=["--browser-url=http://192.168.65.254:9322"]
```

> [!NOTE]
> You will always see one `POST rejected: missing session_id` warn per client connection. This is **normal** — it is the client probing for an existing session before opening SSE. It is not an error.

### Step 4: Tool Execution Fails?

If tools are discovered but calls fail:

- Verify Chrome is running with `--remote-debugging-port=9322 --remote-debugging-address=0.0.0.0`
- Check that `mcp_args` in the startup log shows a resolved IP, not a hostname
- Run the debug DNS test inside the MCP container:

  ```bash
  docker exec chrome-mcp-server node -e "require('http').get('http://<resolved-ip>:9322/json/version', r => { let d=''; r.on('data', c=>d+=c); r.on('end', ()=>console.log(d)) })"
  ```

---

## 🏗 Build from Source

```bash
# Inside mcp-server-bridge directory
cargo build --release
cp target/release/mcp-server-bridge bin/mcp-server-bridge
```
