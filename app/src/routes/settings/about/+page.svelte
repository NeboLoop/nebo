<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">About</h2>
  <p class="text-xs text-base-content/70">Application information and resources.</p>
</div>

<div class="p-4 rounded-xl border border-base-content/5 bg-base-100 mb-6">
  <div class="flex items-center gap-3 mb-4">
    <div class="w-12 h-12 rounded-xl bg-base-content text-base-100 grid place-items-center font-mono text-lg font-semibold">N</div>
    <div>
      <div class="text-base font-bold">Nebo</div>
      <div class="text-sm">AI Agent Orchestration Platform</div>
    </div>
  </div>

  <div class="flex flex-col gap-2 text-sm">
    <div class="flex justify-between py-1.5 border-b border-base-content/5">
      <span class="text-base-content/70">Version</span>
      <span class="font-mono">2.4.1</span>
    </div>
    <div class="flex justify-between py-1.5 border-b border-base-content/5">
      <span class="text-base-content/70">Build</span>
      <span class="font-mono">2026.04.28-abc1234</span>
    </div>
    <div class="flex justify-between py-1.5 border-b border-base-content/5">
      <span class="text-base-content/70">Platform</span>
      <span class="font-mono">macOS arm64</span>
    </div>
    <div class="flex justify-between py-1.5">
      <span class="text-base-content/70">Svelte</span>
      <span class="font-mono">5.55.2</span>
    </div>
  </div>
</div>

<!-- Resources -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3">Resources</h3>
  <div class="flex flex-col gap-1.5">
    {#each ['Documentation', 'Release Notes', 'Community Discord', 'Report an Issue', 'Privacy Policy', 'Terms of Service'] as link}
      <div class="flex items-center justify-between p-3 rounded-lg border border-base-content/5 bg-base-100 cursor-pointer hover:border-base-content/15 transition-colors">
        <span class="text-sm font-medium">{link}</span>
        <span class="text-base-content">→</span>
      </div>
    {/each}
  </div>
</div>

<button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors">Check for Updates</button>
