<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';

  // Payload arrives as query params (no event plumbing needed):
  // /notify?title=..&body=..&agent=..&kind=meeting|reminder|alert|message&time=..&accent=violet
  const q = $page.url.searchParams;
  const title = q.get('title') ?? 'Reminder';
  const body = q.get('body') ?? '';
  const agent = q.get('agent') ?? 'Nebo';
  const kind = (q.get('kind') ?? 'reminder') as keyof typeof KIND;
  const time = q.get('time') ?? '';
  const accent = q.get('accent') ?? 'violet';

  const KIND = {
    meeting: { icon: '📅', label: 'Upcoming meeting' },
    reminder: { icon: '⏰', label: 'Reminder' },
    alert: { icon: '⚠️', label: 'Alert' },
    message: { icon: '💬', label: 'Message' },
  } as const;
  const meta = KIND[kind] ?? KIND.reminder;

  // Static class strings so Tailwind's content scan picks them up.
  const ACCENT: Record<string, string> = {
    violet: 'bg-violet-500',
    blue: 'bg-blue-500',
    emerald: 'bg-emerald-500',
    amber: 'bg-amber-500',
    rose: 'bg-rose-500',
    slate: 'bg-slate-500',
  };
  const accentClass = ACCENT[accent] ?? ACCENT.violet;
  const initial = (agent.trim()[0] ?? 'N').toUpperCase();

  const TIMEOUT_MS = 8000;
  let paused = $state(false);
  let closing = $state(false);
  let timer: ReturnType<typeof setTimeout>;

  function arm() {
    timer = setTimeout(close, TIMEOUT_MS);
  }
  function close() {
    if (closing) return;
    closing = true;
    setTimeout(async () => {
      try {
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        await getCurrentWindow().close();
      } catch {
        /* browser preview — nothing to close */
      }
    }, 240);
  }
  async function open() {
    try {
      const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
      const main = await WebviewWindow.getByLabel('main');
      await main?.unminimize();
      await main?.show();
      await main?.setFocus();
    } catch {
      /* browser preview */
    }
    close();
  }

  onMount(() => {
    arm();
    return () => clearTimeout(timer);
  });
</script>

<div class="flex min-h-screen items-start justify-center p-3">
  <div
    role="alertdialog"
    aria-label={title}
    tabindex="-1"
    onmouseenter={() => {
      paused = true;
      clearTimeout(timer);
    }}
    onmouseleave={() => {
      paused = false;
      arm();
    }}
    class="w-[380px] overflow-hidden rounded-2xl border border-base-content/10 bg-base-100/95 shadow-2xl backdrop-blur-xl
           {closing
      ? 'animate-[hud-out_.24s_ease-in_forwards]'
      : 'animate-[hud-in_.34s_cubic-bezier(.16,1,.3,1)]'}"
  >
    <div class="p-4">
      <div class="mb-3 flex items-center gap-2.5">
        <div class="relative shrink-0">
          <div class="grid h-9 w-9 place-items-center rounded-full text-sm font-semibold text-white {accentClass}">
            {initial}
          </div>
          <div class="absolute -bottom-1 -right-1 grid h-4 w-4 place-items-center rounded-full bg-base-100 text-[10px] shadow">
            {meta.icon}
          </div>
        </div>
        <div class="min-w-0 flex-1">
          <div class="truncate text-xs font-semibold text-base-content">{agent}</div>
          <div class="truncate text-[11px] uppercase tracking-wide text-base-content/45">{meta.label}</div>
        </div>
        {#if time}
          <div class="shrink-0 text-xs font-medium tabular-nums text-base-content/50">{time}</div>
        {/if}
        <button class="btn btn-circle btn-ghost btn-xs text-base-content/40" onclick={close} aria-label="Dismiss">✕</button>
      </div>

      <div class="text-[15px] font-semibold leading-snug text-base-content">{title}</div>
      {#if body}
        <div class="mt-1 line-clamp-2 text-sm text-base-content/65">{body}</div>
      {/if}

      <div class="mt-3.5 flex gap-2">
        <button class="btn btn-primary btn-sm flex-1 rounded-xl" onclick={open}>Open</button>
      </div>
    </div>

    <!-- auto-dismiss bar: drains over TIMEOUT_MS, pauses on hover -->
    <div class="h-1 w-full bg-base-content/5">
      <div
        class="h-full w-full bg-primary/70 {closing ? '' : 'animate-[hud-drain_8s_linear_forwards]'} {paused
          ? '[animation-play-state:paused]'
          : ''}"
      ></div>
    </div>
  </div>
</div>
