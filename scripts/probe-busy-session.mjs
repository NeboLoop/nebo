// Live probe for one-turn-per-session: two chat messages on one session, the
// second while the first is inside a 40 s shell command. Expect the second to
// get an immediate status line and its own chat_complete while the first keeps
// running, and the first's transcript to carry the queued message.
const server = process.env.TEST_SERVER || 'localhost:27895';
const session = `probe:busy:${Date.now()}`;
const ws = new WebSocket(`ws://${server}/ws`);
const t0 = Date.now();
const log = (m) => console.log(`${String(Date.now() - t0).padStart(6)}ms ${m}`);
let completes = 0;
let statusSeen = false;
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  const d = msg.data || {};
  if (d.session_id && d.session_id !== session && d.chatId !== session) return;
  if (msg.type === 'chat_stream' && d.content) {
    log(`stream: ${d.content.slice(0, 120).replace(/\n/g, ' ')}`);
  } else if (msg.type === 'chat_complete') {
    completes += 1;
    log(`chat_complete #${completes} stop_reason=${d.stop_reason || ''}`);
    if (completes === 2) {
      log(`RESULT status_seen_as_typed_status=${statusSeen}`);
      ws.close();
      process.exit(statusSeen ? 0 : 1);
    }
  } else if (msg.type === 'chat_error') {
    // The busy answer arrives on the status channel with its typed reason,
    // so the app can keep the spinner and show a note instead of a reply.
    if (d.stop_reason === 'queued_into_running_turn' && String(d.error).includes('Still working on it')) statusSeen = true;
    log(`status: reason=${d.stop_reason || ''} ${String(d.error).slice(0, 100)}`);
  }
};
ws.onopen = () => {
  const send = (prompt) => ws.send(JSON.stringify({ type: 'chat', data: { session_id: session, prompt, user_id: 'probe', channel: 'web' } }));
  log('send #1 (40 s shell command)');
  send('Use the os shell to run exactly this command and nothing else first: sleep 40. When it finishes, reply with the single word DONE.');
  setTimeout(() => { log('send #2 (status question on the same session)'); send('How is it going?'); }, 8000);
};
setTimeout(() => { log('TIMEOUT'); process.exit(2); }, 180000);
