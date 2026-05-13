<script lang="ts">
  import { N } from '$lib/tokens.js';
  let { tab = 'Orchestrate', children } = $props();

  const tabs = ['Chat', 'Workspace', 'Orchestrate', 'Agents'];
</script>

<div class="flex flex-col h-screen">
  <header class="h-14 border-b border-base-300 bg-base-100 flex items-center px-4 shrink-0">
    <div class="flex items-center gap-1.5 font-semibold text-sm tracking-tight mr-4">
      <div class="w-5 h-5 rounded bg-primary text-primary-content flex items-center justify-center font-mono text-sm font-bold">N</div>
      Nebo
    </div>
    <nav class="flex items-center h-full gap-1">
      {#each tabs as t}
        <span class="px-3 h-full flex items-center text-sm font-medium cursor-pointer transition-colors {t === tab
          ? 'border-b-3 border-primary text-base-content'
          : 'border-b-3 border-transparent text-base-content/70 hover:text-base-content'}">
          {t}
        </span>
      {/each}
    </nav>
    <div class="flex-1"></div>
    <div class="flex items-center h-8 w-48 rounded-field px-3 gap-1.5 text-sm cursor-pointer border border-base-300 bg-base-100">
      <span class="font-mono text-sm py-px px-1 rounded-sm bg-base-200">&#x2318;K</span>
      <span class="text-base-content/70">Search or run an agent…</span>
    </div>
  </header>

  <div class="flex-1 flex min-h-0">
    {@render children()}
  </div>
</div>
