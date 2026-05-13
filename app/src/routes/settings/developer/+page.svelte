<script lang="ts">
  import { devMode } from '$lib/stores/devmode.js';

  let appPath = $state('');

  const sideloadedApps = [
    { name: 'My Custom Tool', path: '~/projects/custom-tool', status: 'running' as const },
    { name: 'Test Plugin', path: '~/projects/test-plugin', status: 'stopped' as const },
  ];
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Developer</h2>
  <p class="text-xs text-base-content/70">Developer tools and advanced settings.</p>
</div>

<!-- Dev mode toggle -->
<div class="p-4 rounded-xl border border-base-content/10 bg-base-100 mb-2">
  <div class="flex items-center justify-between">
    <div>
      <div class="text-sm font-semibold">Developer Mode</div>
      <div class="text-xs text-base-content/50">Show advanced settings like Providers, Routing, and Secrets.</div>
    </div>
    <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={$devMode} onchange={() => $devMode = !$devMode} />
  </div>
</div>

<p class="text-sm text-base-content/40 mb-6">By default, Nebo routes all requests through the Nebo-1 / Janus gateway. Enable developer mode to configure your own providers and routing rules.</p>

{#if $devMode}
  <!-- Sideload app -->
  <div class="mb-6">
    <h3 class="text-base font-semibold mb-3">Sideload App</h3>
    <div class="flex gap-2">
      <input type="text" bind:value={appPath} placeholder="/path/to/your/app" class="flex-1 py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm font-mono outline-none focus:border-base-content/30" />
      <button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors" disabled={!appPath.trim()}>Load</button>
    </div>
  </div>

  <!-- Loaded apps -->
  <div class="mb-6">
    <h3 class="text-base font-semibold mb-3">Sideloaded Apps</h3>
    <div class="flex flex-col gap-1.5">
      {#each sideloadedApps as app}
        <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
          <div class="flex-1">
            <div class="flex items-center gap-2 mb-0.5">
              <span class="text-sm font-semibold">{app.name}</span>
              <span class="px-1.5 py-0.5 rounded text-sm font-mono bg-accent/10 text-accent">dev</span>
              <span class="px-1.5 py-0.5 rounded text-sm font-mono {app.status === 'running' ? 'bg-success/10 text-success' : 'bg-base-200'}">{app.status}</span>
            </div>
            <div class="text-sm font-mono text-base-content/50">{app.path}</div>
          </div>
          <button class="px-3 py-1 rounded-md border border-base-content/10 text-sm cursor-pointer bg-transparent hover:bg-base-200 transition-colors">Relaunch</button>
          <button class="px-3 py-1 rounded-md border border-error/20 text-sm text-error cursor-pointer bg-transparent hover:bg-error/5 transition-colors">Unload</button>
        </div>
      {/each}
    </div>
  </div>

  <!-- How it works -->
  <div class="p-4 rounded-lg bg-base-200/50 text-sm leading-relaxed">
    <div class="font-semibold mb-2">How it works</div>
    <ul class="list-disc list-inside flex flex-col gap-1 text-base-content/70">
      <li>Point to a folder containing your app manifest</li>
      <li>Nebo discovers and loads the app in development mode</li>
      <li>Changes to your app are hot-reloaded automatically</li>
      <li>Sideloaded apps run in a sandboxed environment</li>
    </ul>
  </div>
{/if}
