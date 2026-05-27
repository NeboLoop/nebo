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

function findPids() {
	try {
		if (isWindows) {
			const out = execSync(`netstat -ano | findstr :${port}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString();
			const pids = new Set();
			for (const line of out.split('\n')) {
				const parts = line.trim().split(/\s+/);
				const pid = parts[parts.length - 1];
				if (/^\d+$/.test(pid) && pid !== '0') pids.add(pid);
			}
			return [...pids];
		}
		const out = execSync(`lsof -ti :${port}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString();
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
