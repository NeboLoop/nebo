#!/usr/bin/env node
// Kill any process listening on the given port (defaults to 5173).
//
// Wired as `predev` in package.json: when `cargo tauri dev` exits without
// reaping its `vite dev` child, the next `pnpm dev` invocation hangs with
// `Error: Port 5173 is already in use` because vite is configured
// `strictPort: true` (a Tauri requirement — the webview's devUrl is baked
// into the build). Rather than relax strictPort, we clear the port before
// vite tries to claim it. No-op when nothing's there.
//
// Cross-platform: lsof on POSIX, netstat + taskkill on Windows.

import { execSync } from 'node:child_process';

const port = Number(process.argv[2] || 5173);
const isWindows = process.platform === 'win32';

// Only the process LISTENING on the port holds it. A process merely
// connected to it is never killed: `cargo tauri dev` connects to :5173 while
// it waits for vite, and killing every PID on a `:5173` line killed Tauri
// itself on every Windows `make dev`.
function findPids() {
	try {
		if (isWindows) {
			const out = execSync('netstat -ano -p tcp', { stdio: ['ignore', 'pipe', 'ignore'] }).toString();
			const pids = new Set();
			for (const line of out.split('\n')) {
				// Proto  Local Address  Foreign Address  State  PID
				const [proto, local, , state, pid] = line.trim().split(/\s+/);
				if (proto === 'TCP' && state === 'LISTENING' && local?.endsWith(`:${port}`) && /^\d+$/.test(pid ?? '') && pid !== '0') {
					pids.add(pid);
				}
			}
			return [...pids];
		}
		const out = execSync(`lsof -ti tcp:${port} -sTCP:LISTEN`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString();
		return out.trim().split('\n').filter(Boolean);
	} catch {
		// No process on the port — that's the happy path.
		return [];
	}
}

function killPid(pid) {
	try {
		if (isWindows) {
			execSync(`taskkill /PID ${pid} /F`, { stdio: 'ignore' });
		} else {
			process.kill(Number(pid), 'SIGKILL');
		}
		return true;
	} catch {
		return false;
	}
}

const pids = findPids();
if (pids.length === 0) {
	// Nothing to clean up. Silent — this is the common case.
	process.exit(0);
}

const killed = pids.filter(killPid);
if (killed.length > 0) {
	console.log(`clear-port: freed :${port} (killed ${killed.join(', ')})`);
}
