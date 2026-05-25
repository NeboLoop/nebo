# VM Sandbox — SME Reference

Comprehensive Subject Matter Expert document covering Nebo's lightweight VM
sandbox system: architecture, wire protocol, session management, file transfer,
platform backends, rootfs/image build pipeline, and integration with the tool
system.

**Status:** Implementation in progress | **Last updated:** 2026-05-20

---

## Table of Contents

1. [Purpose & Design Philosophy](#1-purpose--design-philosophy)
2. [Architecture Overview](#2-architecture-overview)
3. [What Runs Where](#3-what-runs-where)
4. [Crate Structure](#4-crate-structure)
5. [Wire Protocol](#5-wire-protocol)
6. [RPC Methods](#6-rpc-methods)
7. [Event Streaming](#7-event-streaming)
8. [Session Management](#8-session-management)
9. [File Transfer (Copy-Out)](#9-file-transfer-copy-out)
10. [VM Manager](#10-vm-manager)
11. [Platform Backends](#11-platform-backends)
12. [VM Image Pipeline](#12-vm-image-pipeline)
13. [Security Model](#13-security-model)
14. [Tool Integration](#14-tool-integration)
15. [Network Configuration](#15-network-configuration)
16. [Relationship to Existing Security](#16-relationship-to-existing-security)
17. [Relationship to Rivet (Cloud Compute)](#17-relationship-to-rivet-cloud-compute)
18. [Design Decisions](#18-design-decisions)
19. [File Manifest](#19-file-manifest)

---

## 1. Purpose & Design Philosophy

### What the VM Is

The VM sandbox is a **capability extension** — a lightweight Linux virtual
machine that runs on the user's desktop, providing an isolated development
environment the agent can use when the host machine doesn't have what it needs.

**Use cases:**
- User is on macOS, agent needs to build/test Linux-specific code
- Agent needs Python 3.12 but user has 3.9
- Agent needs to run `docker compose up` without Docker Desktop installed
- Agent needs gcc/make/cmake for a C build but user doesn't have Xcode CLT
- Agent wants a clean environment that doesn't pollute the user's global packages
- Agent needs to test a web server without conflicting with local ports

### What the VM Is NOT

The VM is **not a security sandbox for Nebo's tools**. Nebo already has a
multi-layer security model (safeguards, sandbox-runtime, origin wall, policy,
path scoping) that protects the host. The VM does not wrap or intercept existing
tool calls.

**Things that stay on the host:**
- Shell commands (already protected by safeguards + sandbox-runtime)
- File operations (already protected by path guards + Seatbelt/bubblewrap)
- Desktop automation (needs real screen, mouse, keyboard)
- Organizer (needs real Mail.app, Calendar.app)
- Browser automation (needs real Chrome via CDP)
- System settings (needs real OS APIs)
- Plugins and skills (tightly coupled to host filesystem and ports)
- Notifications, TTS, clipboard, keychain, Spotlight

**Why plugins/skills don't run in the VM:**
Skills and plugins are tightly coupled to the host. A plugin that starts a web
server on localhost:8080 needs the host's localhost. A skill that writes output
to /tmp/result.json needs the host's /tmp. A plugin that reads the user's
project files needs the host's filesystem. Moving them into the VM would require
rebuilding all of this plumbing (port forwarding, filesystem sharing, env var
proxying) just to get back to where sandbox-runtime already is.

### Design Principles

1. **Opt-in, not default** — The agent explicitly requests a VM environment.
   Shell commands run on the host by default because that's where the user's
   project, tools, and context live.
2. **Host pulls, VM never pushes** — The VM cannot write to the host filesystem.
   File transfer is always host-initiated via RPC (copy-out).
3. **Two images, independent lifecycles** — The rootfs (Alpine + runtimes) is
   large and rarely changes. The sandbox image (daemon + settings) is small and
   ships with every Nebo release.
4. **Unified wire protocol** — Length-prefixed JSON over stdio/vsock. Same
   format regardless of platform (macOS Virtualization.framework, Linux QEMU).
5. **Rust all the way** — Both host and guest are Rust. The guest daemon is a
   static musl binary for minimal image size.

---

## 2. Architecture Overview

```
┌──────────────────────────────────────────────────────────────────┐
│  Host (Nebo Desktop / CLI)                                       │
│                                                                  │
│  Agent Runner                                                    │
│    ↓ tool_call: vm(action: "exec", command: "go build ./...")    │
│                                                                  │
│  Tool Registry                                                   │
│    ↓ dispatch to VmTool                                          │
│                                                                  │
│  VmManager                                                       │
│    ├─ VmConfig (memory, cpus, disk, allowed_domains)             │
│    ├─ Sessions (HashMap<id, VmSession>)                          │
│    └─ VmClient (RPC over stdio/vsock)                            │
│         ↓ write: [4B len][JSON request]                          │
│         ↑ read:  [4B len][JSON response/event]                   │
│                                                                  │
│  VM Service (platform-specific)                                  │
│    ├─ macOS: Swift helper → Virtualization.framework             │
│    └─ Linux: QEMU process → KVM acceleration                    │
│                                                                  │
│         ↕ stdio pipe (dev) / vsock (production)                  │
│                                                                  │
├──────────────────────────────────────────────────────────────────┤
│  Guest VM (lightweight Linux)                                    │
│                                                                  │
│  /init                                                           │
│    ├─ Mounts /proc, /sys, /dev, /tmp                             │
│    ├─ Mounts nebo-vm image at /mnt/nebo-vm                       │
│    └─ exec /mnt/nebo-vm/nebo-vm-daemon                           │
│                                                                  │
│  nebo-vm-daemon                                                  │
│    ├─ Wire protocol reader (stdin)                               │
│    ├─ Request dispatcher (spawn, kill, readFile, writeFile, ...) │
│    ├─ Process manager (per-session working dirs, stdio capture)  │
│    ├─ Event streamer (stdout/stderr/exit → host)                 │
│    └─ File transfer (copyOut → base64-encoded response)          │
│                                                                  │
│  Filesystem:                                                     │
│    /sessions/<id>/     ← per-session working directory           │
│    /tmp/               ← scratch space                           │
│    /mnt/nebo-vm/       ← daemon image (read-only mount)          │
│    /usr/bin/node, python3, git, curl, bash...  ← from rootfs     │
└──────────────────────────────────────────────────────────────────┘
```

---

## 3. What Runs Where

```
                    ┌─────────────────────────────────┐
                    │          HOST (user's machine)    │
                    │                                   │
                    │  Shell commands ← safeguards      │
                    │  File operations ← path guards    │
                    │  Desktop automation ← screen lock │
                    │  Browser ← CDP isolation          │
                    │  Organizer ← native APIs          │
                    │  Plugins ← sandbox-runtime        │
                    │  Skills ← sandbox-runtime         │
                    │  Settings, clipboard, TTS, etc.   │
                    │                                   │
                    │  ┌─────────────────────────────┐  │
                    │  │    VM (opt-in, on demand)    │  │
                    │  │                              │  │
                    │  │  Builds (go, rust, c, java)  │  │
                    │  │  Clean npm/pip installs      │  │
                    │  │  Docker containers           │  │
                    │  │  Linux-specific testing       │  │
                    │  │  Isolated dev environments   │  │
                    │  │  Server processes for dev    │  │
                    │  │                              │  │
                    │  │  → Results copied OUT to     │  │
                    │  │    host via RPC              │  │
                    │  └─────────────────────────────┘  │
                    └───────────────────────────────────┘
```

**Decision matrix:**

| Question | Answer | Runs on |
|---|---|---|
| Does it need the user's actual screen/keyboard? | Yes | Host |
| Does it talk to native OS apps (Mail, Calendar)? | Yes | Host |
| Does it need the user's project files in real time? | Yes | Host |
| Does it bind a port the user needs to access? | Yes | Host |
| Is it a skill/plugin with host filesystem coupling? | Yes | Host |
| Does the user lack the required runtime/toolchain? | Yes | **VM** |
| Does it need a clean Linux environment? | Yes | **VM** |
| Could it pollute global packages/config? | Yes | **VM** |
| Is it a throwaway build/test/compile job? | Yes | **VM** |

---

## 4. Crate Structure

### `crates/vm/` (nebo-vm) — Host-side VM management

| File | Purpose |
|---|---|
| `lib.rs` | Module exports, architecture docs |
| `error.rs` | `VmError` enum, `VmResult<T>` type alias |
| `rpc.rs` | Wire protocol (read/write), message types, `VmClient` |
| `manager.rs` | `VmManager` — lifecycle, sessions, event routing |
| `session.rs` | `VmSession` — per-execution state, stdout/stderr accumulation |
| `transfer.rs` | `FileTransfer` — copy-out from VM to host |
| `platform_macos.rs` | macOS: Swift helper for Virtualization.framework |
| `platform_linux.rs` | Linux: QEMU/KVM process management |
| `vm_helper.swift` | Swift source embedded via `include_str!()`, compiled on first use |

### `crates/vm-daemon/` (nebo-vm-daemon) — Runs inside the VM

| File | Purpose |
|---|---|
| `main.rs` | Entry point, stdio-based RPC loop |
| `wire.rs` | Wire protocol (same format as host side) |
| `handler.rs` | Request dispatcher, file operation handlers |
| `process.rs` | Process spawning, stdout/stderr streaming, signal forwarding |

### `vm/` — Build pipeline

| File | Purpose |
|---|---|
| `Dockerfile.rootfs` | Alpine rootfs with Node, Python, git, build tools |
| `init` | Boot script: mounts nebo-vm image, execs daemon |
| `build-sandbox-img.sh` | Cross-compiles daemon, packages into FAT32 image |
| `settings.json` | Network allowlist, filesystem policy |
| `nebo-vm-rootfs.tar.gz` | Base rootfs (committed or CI-built) |

---

## 5. Wire Protocol

### Framing

```
┌──────────────────┬──────────────────────┐
│ 4 bytes          │ N bytes              │
│ u32 big-endian   │ UTF-8 JSON payload   │
│ (payload length) │                      │
└──────────────────┴──────────────────────┘
```

- **Max message size:** 10 MB (10 × 1024 × 1024 bytes)
- **Encoding:** UTF-8 JSON
- **Transport:** stdio (dev/testing) or vsock (production)

### Host-side write (Rust)

```rust
pub async fn write_message<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    msg: &impl Serialize,
) -> VmResult<()> {
    let payload = serde_json::to_vec(msg)?;
    let len = (payload.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}
```

### Host-side read (Rust)

```rust
pub async fn read_message<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> VmResult<serde_json::Value> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE {
        return Err(VmError::MessageTooLarge { size: len, max: MAX_MESSAGE_SIZE });
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    Ok(serde_json::from_slice(&payload)?)
}
```

### Message discrimination

Messages are discriminated by structure, not a top-level `type` field:

| Category | Key fields | Direction |
|---|---|---|
| Request | `method`, `id`, `params` | Host → Guest |
| Response | `id`, `success`, `result`/`error` | Guest → Host |
| Event | `type`, `id`, `data`/`exit_code`/... | Guest → Host (push) |

---

## 6. RPC Methods

### Host → Guest

| Method | Params | Result | Purpose |
|---|---|---|---|
| `spawn` | `SpawnParams` | `{ process_id }` | Start a process |
| `kill` | `{ id, signal }` | `{}` | Send signal to process |
| `writeStdin` | `{ id, data }` | `{}` | Write to process stdin |
| `isProcessRunning` | `{ id }` | `{ running }` | Check liveness |
| `readFile` | `{ path }` | `{ content }` | Read file from VM |
| `writeFile` | `{ path, content, append }` | `{}` | Write file in VM |
| `listDir` | `{ path }` | `{ entries[] }` | List directory |
| `copyOut` | `{ src_paths[] }` | `{ files[], errors[] }` | Bulk file transfer to host |
| `deleteSessionDirs` | `{ names[] }` | `{ deleted[], errors[] }` | Clean up sessions |

### SpawnParams

```json
{
    "id": "session-uuid",
    "name": "go-build",
    "command": "go",
    "args": ["build", "./..."],
    "cwd": "/sessions/<id>/project",
    "env": { "GOPATH": "/sessions/<id>/go" },
    "timeout_secs": 120,
    "allowed_domains": ["proxy.golang.org", "sum.golang.org"],
    "one_shot": true
}
```

---

## 7. Event Streaming

The guest pushes events to the host without a corresponding request. Events are
routed to the correct session by matching the `id` field to a session's
`process_id`.

| Event type | Fields | Purpose |
|---|---|---|
| `ready` | `{}` | Daemon booted, ready for requests |
| `stdout` | `{ id, data }` | Process stdout chunk |
| `stderr` | `{ id, data }` | Process stderr chunk |
| `exit` | `{ id, exit_code, signal }` | Process finished |
| `error` | `{ id, message, fatal }` | Process error |
| `networkStatus` | `{ status }` | Network state change |

```
Host                              Guest
  │                                  │
  │─── spawn(id, cmd, args) ────────→│
  │                                  │
  │←── { success, process_id } ──────│
  │                                  │
  │←── event: stdout(id, "line1\n") ─│
  │←── event: stderr(id, "warn\n") ──│
  │←── event: stdout(id, "line2\n") ─│
  │←── event: exit(id, code=0) ──────│
  │                                  │
  │─── copyOut(["/sessions/id/..."]) →│
  │←── { files: [{path, base64}] } ──│
```

---

## 8. Session Management

Each execution gets an isolated session:

```
VmSession {
    id: "uuid",
    name: "go-build",
    state: Created → Running → Exited { code: 0 },
    work_dir: "/sessions/<uuid>/",
    allowed_domains: ["proxy.golang.org"],
    stdout: "accumulated output...",
    stderr: "accumulated errors...",
    exit_code: Some(0),
    process_id: Some("uuid"),
}
```

**Lifecycle:**

```
create_session("go-build", domains)
    → VmSession { state: Created, work_dir: /sessions/<id>/ }

exec(session_id, "go build ./...", args, env, timeout)
    → spawn RPC → wait for exit event
    → returns (stdout, stderr, exit_code)

copy_out(session_id, ["/sessions/<id>/bin/myapp"])
    → guest reads files, base64 encodes, returns via RPC
    → host writes to approved destination

destroy_session(session_id)
    → kills process if running
    → deleteSessionDirs RPC
    → removes from sessions map
```

---

## 9. File Transfer (Copy-Out)

### Security Model

The host **always pulls**. The VM cannot push files to the host filesystem.
This is the critical security property — even if code running in the VM is
compromised, it cannot write to `~/.bashrc`, `~/.ssh/authorized_keys`, or
any other host path.

### Flow

```
Agent: "save the build output to ~/projects/myapp/bin/"
   │
   ├─ Host validates destination against safeguards
   │  (protected paths, allowed_paths scope — same checks as file_tool)
   │
   ├─ Host sends copyOut RPC to guest:
   │    { src_paths: ["/sessions/<id>/bin/myapp"] }
   │
   ├─ Guest reads files, base64-encodes content:
   │    { files: [{ path, content_base64, size_bytes }] }
   │
   ├─ Host decodes and writes to approved destination:
   │    ~/projects/myapp/bin/myapp
   │
   └─ Returns success with copied file list
```

### Guest-side restrictions

Even within the VM, the daemon restricts file operations:
- `readFile` / `writeFile` / `listDir` / `copyOut` only allow `/sessions/` and `/tmp/`
- Cannot read `/etc/shadow`, system config, or the daemon binary itself
- Cannot list or copy from `/proc`, `/sys`, `/dev`

---

## 10. VM Manager

```rust
pub struct VmManager {
    config: VmConfig,                                    // Memory, CPUs, disk, domains
    state: Arc<RwLock<VmState>>,                        // Stopped/Starting/Running/Failed
    client: Arc<VmClient>,                              // RPC connection
    sessions: Arc<RwLock<HashMap<String, VmSession>>>,  // Active sessions
    event_rx: Mutex<Option<UnboundedReceiver<Event>>>,  // From guest
}
```

### VmConfig defaults

| Parameter | Default | Notes |
|---|---|---|
| `memory_mb` | 2048 | Configurable per user |
| `cpu_count` | 2 | Configurable per user |
| `disk_size_gb` | 10 | Session data volume |
| `boot_timeout_secs` | 30 | Time to wait for "ready" event |
| `allowed_domains` | pypi, npm, github, crates.io | Extensible per session |

### State machine

```
Stopped ──start()──→ Starting ──ready event──→ Running
   ↑                                              │
   └──────────stop()──────── Stopping ←───────────┘
                                │
                            Failed(reason)
```

---

## 11. Platform Backends

### macOS — Apple Virtualization.framework

Uses the **same pattern as the PIM helper** (`crates/tools/src/organizer/native.rs`):

1. Swift source (`vm_helper.swift`) embedded at compile time via `include_str!()`
2. Compiled on first use via `swiftc -O -framework Virtualization`
3. Cached at `~/.nebo/bin/nebo-vm-helper` with FNV-1a hash-based invalidation
4. Invoked as subprocess: `nebo-vm-helper start --memory-mb 2048 --cpus 2 --image /path`
5. Stdio piped for RPC (Swift bridges host stdin/stdout ↔ VM serial port ↔ guest daemon)

**Requirements:** macOS 13+, Xcode or Command Line Tools (for `swiftc`)

**VM features used:**
- `VZVirtualMachineConfiguration` — CPU, memory, platform
- `VZEFIBootLoader` — standard Linux EFI boot
- `VZVirtioBlockDeviceConfiguration` — disk image attachment
- `VZNATNetworkDeviceAttachment` — NAT networking (VM gets internet via host)
- `VZVirtioConsoleDeviceSerialPortConfiguration` — serial port for RPC
- `VZVirtioEntropyDeviceConfiguration` — guest randomness
- `VZVirtioTraditionalMemoryBalloonDeviceConfiguration` — dynamic memory

### Linux — QEMU/KVM

Direct QEMU invocation with KVM acceleration:

```
qemu-system-x86_64 \
    -enable-kvm \
    -m 2048M -smp 2 \
    -drive file=rootfs.img,format=raw,if=virtio \
    -serial stdio \
    -nographic \
    -netdev user,id=net0 \
    -device virtio-net-pci,netdev=net0
```

**Requirements:** QEMU installed, `/dev/kvm` available

### Windows — Reserved

Future Hyper-V/HCS support. Not yet implemented.

---

## 12. VM Image Pipeline

### Two-image architecture

```
rootfs (big, rarely changes)          nebo-vm image (small, every release)
┌──────────────────────────┐         ┌─────────────────────────────────┐
│ Alpine Linux 3.20 base   │         │ nebo-vm-daemon (Rust, musl)     │
│ Node.js, npm             │         │ settings.json (allowlists)      │
│ Python 3, pip            │         │                                 │
│ git, curl, bash          │         │ ~5-10 MB FAT32 image            │
│ build-base (gcc, make)   │         │ Bundled with every Nebo release │
│ /init (boot script)      │         │ Updated independently of rootfs │
│                          │         └─────────────────────────────────┘
│ ~150-200 MB compressed   │
│ Downloaded once           │
│ Updated for runtime       │
│ upgrades only            │
└──────────────────────────┘
```

### Build commands

```bash
# Cross-compile the daemon (static musl binary)
make vm-daemon
# → target/aarch64-unknown-linux-musl/release/nebo-vm-daemon

# Package into nebo-vm.arm64.img (FAT32 image)
make vm-image
# → vm/build/nebo-vm.arm64.img

# Build rootfs (Docker → raw ext4 → zstd compress → SHA-256)
make vm-rootfs
# → vm/build/rootfs.img.zst  (compressed, for CDN upload)
# → vm/build/rootfs.img      (uncompressed, for local dev)
# → vm/build/rootfs.sha256   (SHA-256 hash)

# Publish rootfs to CDN (CI step)
make vm-rootfs-publish
# → uploads to cdn.neboloop.com/vm/{arch}/{sha}/rootfs.img.zst
```

### Rootfs distribution (CDN bundle management)

Matches the Cowork pattern — rootfs is NOT bundled in the app. Downloaded on
first VM use and cached locally with SHA verification.

```
Bundle directory: ~/.nebo/vm/bundles/
  rootfs.img              ← Linux root filesystem (downloaded per version SHA)
  .rootfs.img.origin      ← SHA version tracker (which SHA this rootfs belongs to)
  rootfs.img.zst          ← Compressed cache for faster reinstalls
  sessiondata.img         ← Persistent user data (survives rootfs updates)
  .auto_reinstall_attempted ← Marker (prevents infinite reinstall loops)

Download URL pattern:
  https://cdn.neboloop.com/vm/{arch}/{sha}/rootfs.img.zst

Resolution order (priority):
  1. Local rootfs.img + origin SHA matches → use directly
  2. Local .zst cache → decompress + SHA verify
  3. CDN download → save .zst cache → decompress + SHA verify

Self-healing:
  - SHA mismatch → re-download from CDN
  - Boot failure → delete rootfs + origin (preserve sessiondata), re-download
  - Reinstall marker prevents infinite loops (max 1 auto-retry per session)
```

Code: `crates/vm/src/bundle.rs` — `Bundle` struct with `ensure_rootfs()`,
`attempt_reinstall()`, `clear_reinstall_marker()`.

The `ROOTFS_SHA` constant in `crates/tools/src/vm_tool.rs` is updated each
time a new rootfs is published. Empty string = dev mode (use local build).

### Boot sequence

```
Hypervisor boots Linux kernel from rootfs
    ↓
/init runs:
    1. mount -t proc proc /proc
    2. mount -t sysfs sys /sys
    3. mount -t devtmpfs dev /dev
    4. mount -t tmpfs tmpfs /tmp
    5. Wait for nebo-vm device (/dev/vdb)
    6. mount -o ro /dev/vdb /mnt/nebo-vm
    7. Load settings from /mnt/nebo-vm/settings.json
    8. exec /mnt/nebo-vm/nebo-vm-daemon
    ↓
Daemon sends "ready" event to host
    ↓
VmManager transitions to Running state
```

### Why two images?

If the daemon were baked into the rootfs, every daemon update would require
rebuilding and re-downloading 150-200 MB. The separate nebo-vm image is
~5-10 MB and ships inside the Nebo app bundle. Users get daemon updates with
every Nebo release without touching the rootfs.

The rootfs only needs updating when runtimes change (new Node LTS, new Python
version, new system packages).

---

## 13. Security Model

### VM is NOT the security boundary

Nebo's security comes from its existing multi-layer model:

```
Layer 1: Hard safeguards (safeguard.rs)
    Blocks: sudo, su, rm -rf /, dd to /dev, fork bombs
    Protects: system dirs, SSH keys, Nebo DB, credentials
    Cannot be overridden by any setting

Layer 2: sandbox-runtime (external crate)
    macOS: Seatbelt (mandatory access control)
    Linux: bubblewrap + seccomp BPF
    Filesystem jails, network domain filtering
    Dangerous file protection (.bashrc, .gitconfig, .git/hooks)

Layer 3: Origin wall (origin.rs)
    Skills/Apps/Comm cannot call shell by default
    Per-origin deny lists

Layer 4: Path scoping (safeguard.rs)
    Per-agent allowed_paths restrictions
    Write/edit/delete restricted to declared directories

Layer 5: Policy & approval (policy.rs)
    Destructive operations require user confirmation
    Allowlists for safe commands
```

### VM security properties

The VM adds isolation for code running **inside** it, not for Nebo's tools:

| Property | Mechanism |
|---|---|
| VM cannot write to host filesystem | Host pulls via RPC; no shared writable mounts |
| VM daemon restricts paths | Only /sessions/ and /tmp/ accessible |
| VM has limited network | settings.json allowlist (same concept as sandbox-runtime) |
| VM processes are ephemeral | Sessions destroyed after use |
| VM crash doesn't affect host | Separate process, kill_on_drop |

---

## 14. Tool Integration

The VM is exposed as an explicit tool the agent can invoke, not as a wrapper
around existing tools.

### Proposed tool interface

```json
{
    "name": "vm",
    "description": "Run commands in an isolated Linux VM environment",
    "schema": {
        "type": "object",
        "properties": {
            "action": {
                "enum": ["exec", "read", "write", "list", "copy_out", "status", "stop"],
                "description": "VM operation"
            },
            "command": { "type": "string", "description": "Shell command (for exec)" },
            "args": { "type": "array", "description": "Command arguments" },
            "path": { "type": "string", "description": "File path in VM" },
            "content": { "type": "string", "description": "File content (for write)" },
            "src_paths": { "type": "array", "description": "VM paths to copy out" },
            "dest_dir": { "type": "string", "description": "Host destination for copy_out" },
            "timeout": { "type": "integer", "description": "Timeout in seconds" }
        },
        "required": ["action"]
    }
}
```

### Example agent flow

```
User: "Build this Go project and give me the binary"

Agent thinks: User is on macOS, project needs Go 1.22.
              Host may not have Go installed. Use VM.

Agent calls: vm(action: "exec", command: "go build -o /sessions/.../myapp ./...")
  → VM spawns process, streams stdout/stderr back
  → Agent sees: "Build complete"

Agent calls: vm(action: "copy_out",
                src_paths: ["/sessions/.../myapp"],
                dest_dir: "~/projects/myapp/bin/")
  → Host validates ~/projects/myapp/bin/ against safeguards
  → Guest reads binary, base64 encodes, returns via RPC
  → Host writes to ~/projects/myapp/bin/myapp
  → Agent sees: "Copied 1 file (2.3 MB)"

Agent: "Done! Binary is at ~/projects/myapp/bin/myapp"
```

### When the agent should use the VM

The agent learns through its system prompt and skill instructions when to use
the VM. It is NOT automatic routing — the agent makes the decision based on
context:

```
"You have access to a vm() tool that provides an isolated Linux environment
with Node.js, Python, Go, git, and build tools. Use it when:
- The user's machine may not have the required toolchain
- You need a clean environment (fresh npm/pip install)
- The task is a build/compile/test job
- You need Linux-specific behavior on a macOS host

Do NOT use it for:
- Reading/writing the user's project files (use system tool)
- Running quick shell commands the host can handle
- Anything that needs the user's screen, browser, or apps"
```

---

## 15. Network Configuration

### settings.json

```json
{
    "network": {
        "allowedDomains": [
            "registry.npmjs.org", "npmjs.com",
            "yarnpkg.com", "registry.yarnpkg.com",
            "pypi.org", "files.pythonhosted.org",
            "github.com",
            "crates.io", "index.crates.io", "static.crates.io",
            "neboloop.com", "api.neboloop.com"
        ],
        "deniedDomains": [],
        "allowLocalBinding": true,
        "allowAllUnixSockets": false
    },
    "filesystem": {
        "denyRead": [],
        "allowWrite": ["/sessions", "/tmp"],
        "denyWrite": ["/usr", "/bin", "/sbin", "/etc"]
    }
}
```

### Per-session domain overrides

Each session can add domains to the allowlist via `SpawnParams.allowed_domains`.
A skill that needs `api.openai.com` declares it in its capability metadata, and
the VM session is created with that domain added.

---

## 16. Relationship to Existing Security

The VM and Nebo's existing security model are complementary, not overlapping:

```
                      Nebo's Security Stack
                      =====================

  ┌─────────────────────────────────────────────────┐
  │  safeguard.rs                                    │
  │  Hard blocks: sudo, rm -rf /, system paths       │  ← Protects HOST
  ├─────────────────────────────────────────────────┤
  │  sandbox-runtime (Seatbelt / bubblewrap)         │
  │  Filesystem jails, network filtering, seccomp    │  ← Protects HOST
  ├─────────────────────────────────────────────────┤
  │  Origin wall + Policy + Path scoping             │
  │  Per-agent restrictions, user approval           │  ← Protects HOST
  ├─────────────────────────────────────────────────┤
  │  VM sandbox (nebo-vm)                            │
  │  Isolated Linux environment for builds/dev       │  ← Extends CAPABILITY
  │  Host-pull-only file transfer                    │
  │  Ephemeral sessions                              │
  └─────────────────────────────────────────────────┘
```

The first three layers protect the host from the agent. The VM extends what the
agent can do without requiring the user to install toolchains.

---

## 17. Relationship to Rivet (Cloud Compute)

Nebo's compute story has three tiers:

| Tier | Where | When | Latency | Cost |
|---|---|---|---|---|
| Host | User's machine | Default | Instant | Free |
| VM | User's machine | Agent needs missing toolchain | ~5s boot | Free |
| Rivet | Cloud (Firecracker) | Laptop closed, long-running | ~1s wake | Credits |

The VM and Rivet share the same concept (isolated Linux environment) but serve
different purposes:

- **VM** — local, immediate, for interactive dev work
- **Rivet** — cloud, persistent, for offline/headless execution

They could potentially share the same rootfs image and daemon binary. The
wire protocol is compatible. A session started locally could theoretically be
migrated to Rivet and back, though this is not planned for v1.

---

## 18. Design Decisions

### Why length-prefixed JSON instead of gRPC/protobuf?

- Zero additional dependencies (serde_json already in the workspace)
- Human-readable on the wire (easier debugging)
- Flexible schema evolution (add fields without breaking)
- Same format used by Cowork (Anthropic's implementation), proven at scale

### Why stdio instead of vsock initially?

- Works on all platforms without hypervisor-specific setup
- Easy to test (pipe to/from a local process)
- Same wire protocol, just swap the transport later
- vsock requires platform-specific guest kernel support

### Why FAT32 for the nebo-vm image?

- Universal: readable on macOS, Linux, and Windows without drivers
- Simple: no journaling, no permissions, no special tools
- Good enough: we only need 2-3 files on it

### Why separate rootfs and nebo-vm images?

- Rootfs is 150-200 MB, nebo-vm image is 5-10 MB
- Rootfs changes rarely (runtime upgrades), nebo-vm changes every release
- Users shouldn't re-download 200 MB for a daemon bug fix
- Same pattern proven by Anthropic's Cowork (rootfs.img + smol-bin.img)

### Why Rust for the guest daemon instead of Go?

- Nebo's entire stack is Rust — one language, one toolchain
- Static musl binaries are trivial with Rust cross-compilation
- Shares types and patterns with the host crate
- Smaller binary size than equivalent Go (no runtime overhead)

### Why not use the VM for all tool execution?

- Most tools need real OS access (screen, browser, mail, calendar)
- Skills and plugins are tightly coupled to host filesystem and ports
- Nebo's existing security (safeguards + sandbox-runtime) is sufficient
- Adding VM overhead to every shell command would be noticeably slower
- The VM is a capability extension, not a security replacement

---

## 19. File Manifest

```
crates/vm/
├── Cargo.toml                          # nebo-vm workspace crate
├── src/
│   ├── lib.rs                          # Module exports, architecture docs
│   ├── bundle.rs                       # CDN bundle management (download, cache, SHA verify)
│   ├── error.rs                        # VmError enum, VmResult type
│   ├── rpc.rs                          # Wire protocol, VmClient, message types
│   ├── manager.rs                      # VmManager — lifecycle, sessions, events
│   ├── session.rs                      # VmSession — per-execution state
│   ├── transfer.rs                     # FileTransfer — copy-out operations
│   ├── platform_macos.rs               # macOS Virtualization.framework backend
│   ├── platform_linux.rs               # Linux QEMU/KVM backend
│   └── vm_helper.swift                 # Virtualization.framework CLI wrapper

crates/vm-daemon/
├── Cargo.toml                          # nebo-vm-daemon, static musl binary
├── src/
│   ├── main.rs                         # Entry point, stdio RPC
│   ├── wire.rs                         # Wire protocol (same format as host)
│   ├── handler.rs                      # Request dispatcher, file handlers
│   └── process.rs                      # Process spawning, stdout/stderr streaming

crates/tools/src/
│   └── vm_tool.rs                      # vm() DynTool — agent-facing interface

vm/
├── Dockerfile.rootfs                   # Alpine rootfs build (Node, Python, git)
├── init                                # VM boot script (mounts image, execs daemon)
├── build-sandbox-img.sh                # Cross-compile + package nebo-vm.{arch}.img
├── settings.json                       # Network allowlist, filesystem policy
└── build/                              # Build outputs (gitignored)
    ├── rootfs.img                      # Raw ext4 disk image (for local dev)
    ├── rootfs.img.zst                  # Zstd compressed (for CDN upload)
    ├── rootfs.sha256                   # SHA-256 hash (set in ROOTFS_SHA const)
    └── nebo-vm.arm64.img              # Sidecar image (bundled in app)
```
