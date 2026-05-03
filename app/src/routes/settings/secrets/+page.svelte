<script lang="ts">
  const secrets = [
    { skill: 'Web Scraper', secrets: [
      { key: 'PROXY_URL', configured: true, desc: 'Proxy server URL for web scraping' },
      { key: 'SCRAPER_API_KEY', configured: false, desc: 'API key for premium scraping service' },
    ]},
    { skill: 'Email Drafter', secrets: [
      { key: 'SMTP_HOST', configured: true, desc: 'SMTP server hostname' },
      { key: 'SMTP_PASSWORD', configured: true, desc: 'SMTP password or app-specific password' },
    ]},
    { skill: 'Image Generator', secrets: [
      { key: 'REPLICATE_API_KEY', configured: false, desc: 'Replicate API key for image generation' },
    ]},
  ];
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Secrets</h2>
  <p class="text-xs text-base-content/70">Manage API keys and credentials used by your skills.</p>
</div>

{#each secrets as group}
  <div class="mb-6">
    <h3 class="text-sm font-semibold mb-2">{group.skill}</h3>
    <div class="flex flex-col gap-1.5">
      {#each group.secrets as secret}
        <div class="flex items-center gap-3 p-3 rounded-lg border border-base-content/5 bg-base-100">
          <div class="flex-1">
            <div class="flex items-center gap-2 mb-0.5">
              <span class="text-sm font-mono font-medium">{secret.key}</span>
              {#if secret.configured}
                <span class="px-1.5 py-0.5 rounded text-sm bg-success/10 text-success font-medium">Set</span>
              {:else}
                <span class="px-1.5 py-0.5 rounded text-sm bg-warning/10 text-warning font-medium">Not set</span>
              {/if}
            </div>
            <div class="text-xs text-base-content/50">{secret.desc}</div>
          </div>
          <input type="password" placeholder="••••••••" class="w-40 py-1.5 px-2.5 rounded-md border border-base-content/10 bg-base-100 text-sm font-mono outline-none focus:border-base-content/30" value={secret.configured ? '••••••••' : ''} />
          <button class="px-3 py-1 rounded-md border border-base-content/10 text-sm cursor-pointer bg-transparent hover:bg-base-200 transition-colors">Save</button>
        </div>
      {/each}
    </div>
  </div>
{/each}
