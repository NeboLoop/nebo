// Live probe for one-turn-per-session: two chat messages on one session, the
// second while the first is inside a 40 s shell command. Expect the second to
// get its own chat_complete at once with the typed queued stop and NO words
// (no chat_error, no busy line) while the first keeps running, and the
// first's transcript to carry the queued message.
// The owner's own client: it proves itself to the local API with the install
// key, as the address's first segment (scripts/install-key.sh).
const key = (process.env.NEBO_MCP_API_KEY || (await import('node:fs')).readFileSync(
  `${process.env.NEBO_HOME || `${process.env.HOME}/Library/Application Support/Nebo`}/.install-key`, 'utf8')).trim();
const server = `${process.env.TEST_SERVER || 'localhost:27895'}/k/${key}`;
const session = `probe:busy:${Date.now()}`;
const ws = new WebSocket(`ws://${server}/ws`);
const t0 = Date.now();
console.log(`session=${session}`);
const log = (m) => console.log(`${String(Date.now() - t0).padStart(6)}ms ${m}`);
let completes = 0;
let queuedSilently = false;
let announced = false;
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  const d = msg.data || {};
  if (d.session_id && d.session_id !== session && d.chatId !== session) return;
  if (msg.type === 'chat_stream' && d.content) {
    log(`stream: ${d.content.slice(0, 120).replace(/\n/g, ' ')}`);
  } else if (msg.type === 'chat_complete') {
    completes += 1;
    log(`chat_complete #${completes} stop_reason=${d.stop_reason || ''}`);
    // The message taken in ends with its typed reason and no words: the app
    // marks it pending and announces nothing.
    if (d.stop_reason === 'queued_into_running_turn' && !d.stop_notice) queuedSilently = true;
    if (completes === 2) {
      const ok = queuedSilently && !announced;
      log(`RESULT queued_silently=${queuedSilently} announced=${announced}`);
      ws.close();
      process.exit(ok ? 0 : 1);
    }
  } else if (msg.type === 'chat_error') {
    announced = true;
    log(`chat_error: reason=${d.stop_reason || ''} ${String(d.error).slice(0, 100)}`);
  }
};
ws.onopen = () => {
  const send = (prompt) => ws.send(JSON.stringify({ type: 'chat', data: { session_id: session, prompt, user_id: 'probe', channel: 'web' } }));
  log('send #1 (40 s shell command)');
  send('Use the os shell to run exactly this command and nothing else first: sleep 40. When it finishes, reply with the single word DONE.');
  setTimeout(() => { log('send #2 (status question on the same session)'); send('How is it going?'); }, 8000);
};
setTimeout(() => { log('TIMEOUT'); process.exit(2); }, 180000);
