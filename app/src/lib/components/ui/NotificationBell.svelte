<!--
  Notification Bell Component
  Shows unread count badge and dropdown with notifications
-->

<script lang="ts">
	import { Bell, Check, CheckCheck, Trash2, ExternalLink } from 'lucide-svelte';
	import { goto } from '$app/navigation';
	import { notification, notifications, unreadNotificationCount, hasUnreadNotifications } from '$lib/stores/notification';
	import { onMount, onDestroy } from 'svelte';

	interface Props {
		class?: string;
	}

	let { class: extraClass = '' }: Props = $props();

	let isOpen = $state(false);
	let dropdownRef = $state<HTMLDivElement | null>(null);

	onMount(() => {
		notification.startPolling();
	});

	onDestroy(() => {
		notification.stopPolling();
	});

	function toggleDropdown() {
		isOpen = !isOpen;
		if (isOpen && $notifications.length === 0) {
			notification.loadNotifications();
		}
	}

	function closeDropdown() {
		isOpen = false;
	}

	function handleClickOutside(e: MouseEvent) {
		if (dropdownRef && !dropdownRef.contains(e.target as Node)) {
			closeDropdown();
		}
	}

	async function handleMarkAsRead(id: string) {
		await notification.markAsRead(id);
	}

	async function handleMarkAllAsRead() {
		await notification.markAllAsRead();
	}

	async function handleDelete(id: string) {
		await notification.deleteNotification(id);
	}

	function handleNotificationClick(n: { actionUrl?: string; id: string; readAt?: string }) {
		if (!n.readAt) {
			notification.markAsRead(n.id);
		}
		if (n.actionUrl) {
			// Use goto for internal URLs, window.location for external
			if (n.actionUrl.startsWith('/')) {
				goto(n.actionUrl);
			} else {
				window.location.href = n.actionUrl;
			}
		}
		closeDropdown();
	}

	function formatTime(dateStr: string): string {
		const date = new Date(dateStr);
		const now = new Date();
		const diffMs = now.getTime() - date.getTime();
		const diffMins = Math.floor(diffMs / 60000);
		const diffHours = Math.floor(diffMs / 3600000);
		const diffDays = Math.floor(diffMs / 86400000);

		if (diffMins < 1) return 'Just now';
		if (diffMins < 60) return `${diffMins}m ago`;
		if (diffHours < 24) return `${diffHours}h ago`;
		if (diffDays < 7) return `${diffDays}d ago`;
		return date.toLocaleDateString();
	}

	const iconMap: Record<string, string> = {
		invite: '👥',
		billing: '💳',
		system: '🔔',
		team: '👤',
		success: '✅',
		warning: '⚠️',
		error: '❌'
	};

	$effect(() => {
		if (isOpen) {
			document.addEventListener('click', handleClickOutside);
		} else {
			document.removeEventListener('click', handleClickOutside);
		}
		return () => document.removeEventListener('click', handleClickOutside);
	});
</script>

<div class="relative {extraClass}" bind:this={dropdownRef}>
	<!-- Bell Button -->
	<button
		type="button"
		class="btn btn-ghost btn-circle relative"
		onclick={toggleDropdown}
		aria-label="Notifications"
		aria-expanded={isOpen}
		aria-haspopup="true"
	>
		<Bell class="w-5 h-5" />
		{#if $hasUnreadNotifications}
			<span
				class="absolute -top-1 -right-1 min-w-5 h-5 px-1 rounded-full bg-error text-error-content text-xs font-bold flex items-center justify-center"
			>
				{$unreadNotificationCount > 99 ? '99+' : $unreadNotificationCount}
			</span>
		{/if}
	</button>

	<!-- Dropdown -->
	{#if isOpen}
		<div
			class="absolute right-0 mt-2 w-80 sm:w-96 bg-base-100 border border-base-300 rounded-lg shadow-xl z-50"
			role="menu"
		>
			<!-- Header -->
			<div class="flex items-center justify-between px-4 py-3 border-b border-base-300">
				<h3 class="font-semibold">Notifications</h3>
				{#if $hasUnreadNotifications}
					<button
						type="button"
						class="btn btn-ghost btn-xs gap-1"
						onclick={handleMarkAllAsRead}
					>
						<CheckCheck class="w-3 h-3" />
						Mark all read
					</button>
				{/if}
			</div>

			<!-- Notification List -->
			<div class="max-h-96 overflow-y-auto">
				{#if $notifications.length === 0}
					<div class="px-4 py-8 text-center text-base-content/60">
						<Bell class="w-8 h-8 mx-auto mb-2 opacity-40" />
						<p>No notifications yet</p>
					</div>
				{:else}
					{#each $notifications as n}
						<div
							class="group relative px-4 py-3 border-b border-base-200 hover:bg-base-200/50 transition-colors cursor-pointer {!n.readAt ? 'bg-base-200/30' : ''}"
							onclick={() => handleNotificationClick(n)}
							onkeydown={(e) => e.key === 'Enter' && handleNotificationClick(n)}
							role="menuitem"
							tabindex="0"
						>
							<div class="flex gap-3">
								<!-- Icon -->
								<div class="text-xl flex-shrink-0">
									{iconMap[n.type] || iconMap[n.icon || 'system']}
								</div>

								<!-- Content -->
								<div class="flex-1 min-w-0">
									<div class="flex items-start justify-between gap-2">
										<p class="font-medium text-sm" class:font-bold={!n.readAt}>
											{n.title}
										</p>
										{#if !n.readAt}
											<span class="w-2 h-2 rounded-full bg-primary flex-shrink-0 mt-1.5"></span>
										{/if}
									</div>
									{#if n.body}
										<p class="text-sm text-base-content/70 line-clamp-2 mt-0.5">
											{n.body}
										</p>
									{/if}
									<p class="text-xs text-base-content/50 mt-1">
										{formatTime(n.createdAt)}
									</p>
								</div>
							</div>

							<!-- Actions (show on hover) -->
							<div class="absolute right-2 top-2 hidden group-hover:flex gap-1">
								{#if !n.readAt}
									<button
										type="button"
										class="btn btn-ghost btn-xs btn-circle"
										onclick={(e) => { e.stopPropagation(); handleMarkAsRead(n.id); }}
										title="Mark as read"
									>
										<Check class="w-3 h-3" />
									</button>
								{/if}
								<button
									type="button"
									class="btn btn-ghost btn-xs btn-circle text-error"
									onclick={(e) => { e.stopPropagation(); handleDelete(n.id); }}
									title="Delete"
								>
									<Trash2 class="w-3 h-3" />
								</button>
							</div>
						</div>
					{/each}
				{/if}
			</div>

			<!-- Footer -->
			{#if $notifications.length > 0}
				<div class="px-4 py-2 border-t border-base-300">
					<a
						href="/notifications"
						class="btn btn-ghost btn-sm btn-block gap-1"
						onclick={closeDropdown}
					>
						View all notifications
						<ExternalLink class="w-3 h-3" />
					</a>
				</div>
			{/if}
		</div>
	{/if}
</div>
