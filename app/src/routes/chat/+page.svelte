<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';

  let messages = $state<{ role: string; text: string }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      // Try loading companion chat messages
      const resp = await api.getCompanionChat();
      if (resp?.messages?.length) {
        messages = (resp.messages as unknown as Record<string, unknown>[]).map((m) => ({
          role: m.role as string,
          text: m.content as string,
        }));
      }
    } catch {
      // Keep mock messages
    }
  });
</script>

<svelte:head><title>Chat - Nebo</title></svelte:head>

<div class="flex h-screen bg-base-100 text-base-content text-sm">
  <Sidebar activePage="chat" activeChat="c1" />
  <div class="flex-1 flex flex-col min-w-0 min-h-0">
    <div class="h-12 px-5 border-b border-base-content/10 flex items-center gap-3.5 shrink-0">
      <span class="text-sm font-semibold">Summarize Q3 board deck</span>
      <div class="ml-auto h-7 w-[200px] rounded-md border border-base-content/10 bg-base-100 flex items-center px-2.5 gap-2 text-sm">
        <span class="font-mono">⌘K</span><span>Search or run…</span>
      </div>
    </div>

    <div class="flex-1 flex flex-col min-h-0">
      <div class="flex-1 overflow-auto px-6 py-6 flex flex-col gap-5">
        {#each messages as msg}
          <div class="flex gap-3 max-w-[720px] {msg.role === 'user' ? 'self-end flex-row-reverse' : ''}">
            <div class="w-7 h-7 rounded-md grid place-items-center font-mono text-sm font-semibold shrink-0 {msg.role === 'user' ? 'bg-[var(--agent-violet-bg)] text-[var(--agent-violet-ink)]' : 'bg-base-200'}">
              {msg.role === 'user' ? 'A' : 'N'}
            </div>
            <div class="px-3.5 py-2.5 rounded-xl text-sm leading-relaxed max-w-[560px] {msg.role === 'user' ? 'bg-primary text-primary-content rounded-br-sm' : 'bg-base-200 text-base-content rounded-bl-sm'}">
              {#each msg.text.split('\n') as line}
                {#if line.startsWith('**') && line.endsWith('**')}
                  <strong>{line.replace(/\*\*/g, '')}</strong><br/>
                {:else if line.startsWith('|')}
                  <span class="font-mono text-xs">{line}</span><br/>
                {:else if line.startsWith('- ')}
                  <span>• {line.slice(2)}</span><br/>
                {:else if line.match(/^\d+\./)}
                  <span>{line}</span><br/>
                {:else}
                  {line}<br/>
                {/if}
              {/each}
            </div>
          </div>
        {/each}
      </div>

      <div class="px-6 py-3 border-t border-base-content/10 flex items-center gap-2.5">
        <button class="w-8 h-8 rounded-lg grid place-items-center hover:text-base-content cursor-pointer">+</button>
        <textarea rows="1" placeholder="Type a message…"
          class="flex-1 py-2.5 px-3.5 rounded-xl border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30 resize-none placeholder:text-base-content"></textarea>
        <button class="w-[34px] h-[34px] rounded-[10px] grid place-items-center bg-base-300 text-base-content cursor-pointer">↑</button>
      </div>
    </div>
  </div>
</div>
