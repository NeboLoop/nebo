<script lang="ts">
  import { goto } from '$app/navigation';
  import Bell from 'lucide-svelte/icons/bell';
  import X from 'lucide-svelte/icons/x';
  import CheckCheck from 'lucide-svelte/icons/check-check';
  import { notifications, unreadCount, markAsRead, markAllRead, removeNotification, type Notification } from '$lib/stores/notifications.js';

  let open = $state(false);

  const typeColors: Record<string, string> = {
    agent: 'bg-success',
    system: 'bg-info',
    warning: 'bg-warning',
    error: 'bg-error',
  };

  function handleClick(notif: Notification) {
    markAsRead(notif.id);
    if (notif.link) {
      open = false;
      goto(notif.link);
    }
  }
</script>

<div class="relative">
  <button
    onclick={() => (open = !open)}
    class="relative p-2 rounded-lg hover:bg-base-content/5 transition-colors cursor-pointer bg-transparent border-none"
    aria-label="Notifications"
  >
    <Bell class="w-4.5 h-4.5 text-base-content/70" />
    {#if $unreadCount > 0}
      <span class="absolute top-1 right-1 w-4 h-4 rounded-full bg-error text-error-content text-[0.625rem] font-bold flex items-center justify-center">
        {$unreadCount > 9 ? '9+' : $unreadCount}
      </span>
    {/if}
  </button>

  {#if open}
    <div class="fixed inset-0 z-40" onclick={() => (open = false)} role="presentation"></div>
    <div class="absolute right-0 top-full mt-1 w-80 bg-base-100 rounded-xl border border-base-300 shadow-xl z-50 overflow-hidden">
      <!-- Header -->
      <div class="flex items-center justify-between px-4 py-3 border-b border-base-content/10">
        <span class="text-sm font-bold">Notifications</span>
        {#if $unreadCount > 0}
          <button
            onclick={markAllRead}
            class="flex items-center gap-1 text-sm text-primary hover:brightness-110 transition-all cursor-pointer bg-transparent border-none"
          >
            <CheckCheck class="w-3.5 h-3.5" /> Mark all read
          </button>
        {/if}
      </div>

      <!-- List -->
      <div class="max-h-80 overflow-y-auto">
        {#if $notifications.length === 0}
          <div class="py-8 text-center text-xs text-base-content/50">No notifications</div>
        {:else}
          {#each $notifications as notif (notif.id)}
            <div
              class="flex items-start gap-3 px-4 py-3 border-b border-base-content/5 last:border-b-0 transition-colors cursor-pointer {notif.read ? '' : 'bg-primary/5'}"
              onclick={() => handleClick(notif)}
              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); handleClick(notif); } }}
              role="button"
              tabindex="0"
            >
              <div class="w-2 h-2 rounded-full mt-1.5 shrink-0 {typeColors[notif.type] || 'bg-info'}"></div>
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-sm font-semibold truncate {notif.read ? 'text-base-content/70' : 'text-base-content'}">{notif.title}</span>
                  <span class="text-sm text-base-content/40 shrink-0">{notif.time}</span>
                </div>
                <p class="text-xs text-base-content/50 truncate mt-0.5">{notif.message}</p>
              </div>
              <button
                onclick={(e) => { e.stopPropagation(); removeNotification(notif.id); }}
                class="p-1 rounded hover:bg-base-content/10 transition-colors cursor-pointer bg-transparent border-none shrink-0"
              >
                <X class="w-3 h-3 text-base-content/30" />
              </button>
            </div>
          {/each}
        {/if}
      </div>
    </div>
  {/if}
</div>
