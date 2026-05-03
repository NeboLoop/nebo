<script lang="ts">
  const plan = { name: 'Pro', price: '$29/mo' };
  const usage = {
    sessions: { used: 847, limit: 2000 },
    weekly: { used: 142, limit: 500 },
    budget: { free: 12.40, gift: 0, credits: 5.00 },
  };

  const sessionPct = Math.round((usage.sessions.used / usage.sessions.limit) * 100);
  const weeklyPct = Math.round((usage.weekly.used / usage.weekly.limit) * 100);
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Usage</h2>
  <p class="text-xs text-base-content/70">Monitor your plan usage and billing balance.</p>
</div>

<!-- Plan -->
<div class="p-4 rounded-xl border border-success/30 bg-success/5 mb-6">
  <div class="flex items-center justify-between">
    <div>
      <div class="text-sm font-semibold">{plan.name} Plan</div>
      <div class="text-xs text-base-content/50">{plan.price}</div>
    </div>
    <a href="/settings/billing" class="px-3 py-1.5 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors">Change Plan</a>
  </div>
</div>

<!-- Session usage -->
<div class="mb-5">
  <div class="flex items-center justify-between mb-1.5">
    <span class="text-sm font-semibold">Sessions this month</span>
    <span class="text-sm font-mono">{usage.sessions.used.toLocaleString()} / {usage.sessions.limit.toLocaleString()}</span>
  </div>
  <div class="w-full h-2 rounded-full bg-base-200 overflow-hidden">
    <div class="h-full rounded-full {sessionPct > 80 ? 'bg-warning' : 'bg-primary'} transition-all" style="width: {sessionPct}%"></div>
  </div>
  <div class="text-xs text-base-content/50 mt-1">{sessionPct}% used</div>
</div>

<!-- Weekly usage -->
<div class="mb-6">
  <div class="flex items-center justify-between mb-1.5">
    <span class="text-sm font-semibold">Weekly requests</span>
    <span class="text-sm font-mono">{usage.weekly.used} / {usage.weekly.limit}</span>
  </div>
  <div class="w-full h-2 rounded-full bg-base-200 overflow-hidden">
    <div class="h-full rounded-full bg-primary transition-all" style="width: {weeklyPct}%"></div>
  </div>
  <div class="text-xs text-base-content/50 mt-1">{weeklyPct}% used</div>
</div>

<!-- Budget -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3">Balance</h3>
  <div class="flex gap-2.5">
    <div class="flex-1 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="text-xs text-base-content/50 mb-0.5">Free pool</div>
      <div class="text-lg font-mono font-bold">${usage.budget.free.toFixed(2)}</div>
    </div>
    <div class="flex-1 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="text-xs text-base-content/50 mb-0.5">Gift pool</div>
      <div class="text-lg font-mono font-bold">${usage.budget.gift.toFixed(2)}</div>
    </div>
    <div class="flex-1 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="text-xs text-base-content/50 mb-0.5">Credits</div>
      <div class="text-lg font-mono font-bold">${usage.budget.credits.toFixed(2)}</div>
    </div>
  </div>
</div>

<button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm cursor-pointer bg-transparent hover:bg-base-200 transition-colors">Refresh Usage</button>
