<script lang="ts">
  import { onMount } from 'svelte';
  import Gift from 'lucide-svelte/icons/gift';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import Info from 'lucide-svelte/icons/info';
  import * as api from '$lib/api/nebo';
  import Spinner from '$lib/components/ui/Spinner.svelte';

  let referralCode = $state('');
  let referralLink = $state('');
  let referralCopied = $state(false);
  let referralLinkCopied = $state(false);
  let showGiftInfo = $state(false);
  let loaded = $state(false);

  interface ReferralResponse {
    referral_code: string;
    referral_link: string;
  }

  onMount(async () => {
    try {
      const resp = await api.neboAIReferralCode() as ReferralResponse;
      referralCode = resp.referral_code;
      referralLink = resp.referral_link;
    } catch { /* not connected */ }
    loaded = true;
  });

  function copyCode() {
    navigator.clipboard.writeText(referralCode);
    referralCopied = true;
    setTimeout(() => referralCopied = false, 2000);
  }

  function copyLink() {
    navigator.clipboard.writeText(referralLink);
    referralLinkCopied = true;
    setTimeout(() => referralLinkCopied = false, 2000);
  }
</script>

<section>
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Give Nebo</h3>
    <button
      type="button"
      onclick={() => (showGiftInfo = true)}
      class="flex items-center gap-1 text-xs text-base-content/50 hover:text-base-content/70 transition-colors cursor-pointer bg-transparent border-none"
    >
      <Info class="w-3.5 h-3.5" />
      <span>How it works</span>
    </button>
  </div>
  <div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
    <div class="flex items-center gap-3 mb-1">
      <Gift class="w-5 h-5 text-primary" />
      <p class="text-sm font-medium text-base-content">Give a friend a bonus 1M tokens</p>
    </div>
    <p class="text-xs text-base-content/50 mb-4 ml-8">They get 3M tokens on signup plus a bonus 1M from you — 4M total to start. You get 3M when they try it.</p>
    {#if referralLink}
      <div class="flex flex-col gap-2">
        <div class="flex items-center gap-2">
          <span class="flex-1 text-sm text-base-content bg-base-300/60 rounded-xl px-4 py-2.5 truncate">
            {referralLink}
          </span>
          <button
            type="button"
            onclick={copyLink}
            class="h-10 px-4 rounded-xl bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all shrink-0 flex items-center gap-1.5 cursor-pointer border-none"
            title="Copy link"
          >
            {#if referralLinkCopied}
              <Check class="w-4 h-4" />
              Copied
            {:else}
              <Copy class="w-4 h-4" />
              Copy
            {/if}
          </button>
        </div>
        <div class="flex items-center gap-2">
          <span class="text-xs text-base-content/70">Your code:</span>
          <span class="text-xs font-mono font-bold text-base-content tracking-wider">{referralCode}</span>
          <button
            type="button"
            onclick={copyCode}
            class="text-xs text-primary hover:brightness-110 transition-all flex items-center gap-1 cursor-pointer bg-transparent border-none"
            title="Copy code"
          >
            {#if referralCopied}
              <Check class="w-3 h-3" />
              Copied
            {:else}
              <Copy class="w-3 h-3" />
              Copy
            {/if}
          </button>
        </div>
      </div>
    {:else if loaded}
      <p class="text-xs text-base-content/50 ml-8">Connect your NeboAI account to get your gift link.</p>
    {:else}
      <div class="flex items-center gap-2 ml-8">
        <Spinner size={14} />
        <span class="text-xs text-base-content/50">Loading your gift link...</span>
      </div>
    {/if}
  </div>
</section>

<!-- How Gift Works Modal -->
{#if showGiftInfo}
  <div class="fixed inset-0 z-[80] flex items-center justify-center" role="dialog" aria-modal="true">
    <button type="button" class="absolute inset-0 bg-black/60 backdrop-blur-sm cursor-default border-none" onclick={() => (showGiftInfo = false)} aria-label="Close"></button>
    <div class="relative rounded-2xl bg-base-100 w-full max-w-sm shadow-xl">
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-base font-semibold">How Giving Nebo Works</h3>
        <button type="button" onclick={() => (showGiftInfo = false)} class="text-base-content/60 text-xl hover:text-base-content cursor-pointer bg-transparent border-none" aria-label="Close">
          &times;
        </button>
      </div>

      <div class="px-5 py-5 space-y-5">
        <div class="space-y-4">
          <div class="flex gap-3">
            <div class="w-7 h-7 rounded-full bg-primary/10 flex items-center justify-center shrink-0">
              <span class="text-xs font-bold text-primary">1</span>
            </div>
            <div>
              <p class="text-sm font-medium text-base-content">Share your link</p>
              <p class="text-xs text-base-content/50">Send your personal link to someone you want to have Nebo.</p>
            </div>
          </div>
          <div class="flex gap-3">
            <div class="w-7 h-7 rounded-full bg-primary/10 flex items-center justify-center shrink-0">
              <span class="text-xs font-bold text-primary">2</span>
            </div>
            <div>
              <p class="text-sm font-medium text-base-content">They start with 4M tokens</p>
              <p class="text-xs text-base-content/50">Everyone gets 3M on signup. Your gift adds a bonus 1M — so they start with 4 million tokens.</p>
            </div>
          </div>
          <div class="flex gap-3">
            <div class="w-7 h-7 rounded-full bg-primary/10 flex items-center justify-center shrink-0">
              <span class="text-xs font-bold text-primary">3</span>
            </div>
            <div>
              <p class="text-sm font-medium text-base-content">You get 3M tokens</p>
              <p class="text-xs text-base-content/50">Once they try Nebo, you receive 3 million tokens as a thank you.</p>
            </div>
          </div>
        </div>

        <div class="rounded-xl bg-base-200/50 border border-base-content/10 p-4">
          <p class="text-sm font-medium text-base-content mb-2">Gift Milestones</p>
          <div class="space-y-1.5">
            {#each [
              { count: 3, tier: 'Guide', reward: '+50M tokens' },
              { count: 5, tier: 'Builder', reward: '+100M tokens' },
              { count: 10, tier: 'Pathfinder', reward: '+250M tokens' },
              { count: 25, tier: 'Benefactor', reward: '+500M tokens' },
              { count: 50, tier: 'Patron', reward: '+1B tokens' },
              { count: 100, tier: "Founder's Circle", reward: '+2B tokens' }
            ] as milestone}
              <div class="flex items-center justify-between text-xs">
                <span class="text-base-content/70">{milestone.count} gifts &rarr; <span class="font-medium text-base-content">{milestone.tier}</span></span>
                <span class="text-primary font-medium tabular-nums">{milestone.reward}</span>
              </div>
            {/each}
          </div>
        </div>

        <p class="text-xs text-base-content/50">
          The more people you bring along, the more tokens you earn. Each milestone unlocks additional perks on your NeboAI profile. All bonus tokens expire 90 days after they're granted.
          <a href="https://getnebo.com/legal/gifting-terms" target="_blank" rel="noopener noreferrer" class="text-primary hover:brightness-110 transition-all">Gifting Terms</a>
        </p>
      </div>
    </div>
  </div>
{/if}
