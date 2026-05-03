<script lang="ts">
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import { auth } from '$lib/stores/auth';
	import { Settings, HelpCircle, CreditCard, LogOut } from 'lucide-svelte';

	interface Props {
		userName?: string;
	}

	let { userName = '' }: Props = $props();

	let open = $state(false);

	const initial = $derived(userName ? userName.charAt(0).toUpperCase() : '?');

	function toggle(e: MouseEvent) {
		e.stopPropagation();
		open = !open;
	}

	function close() {
		open = false;
	}

	function navigate(path: string) {
		close();
		goto(path);
	}

	function handleLogout() {
		close();
		auth.logout();
	}
</script>

<svelte:window onclick={close} />

<div class="relative">
	<button
		class="flex items-center gap-2.5 w-full px-2.5 py-2 rounded-lg cursor-pointer hover:bg-base-300 transition-colors"
		onclick={toggle}
	>
		<div class="w-[26px] h-[26px] rounded-full grid place-items-center text-[11px] font-semibold bg-primary/10 text-primary shrink-0">
			{initial}
		</div>
		<span class="text-[13.5px] text-base-content truncate">{userName || 'Account'}</span>
	</button>

	{#if open}
		<!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
		<div class="fixed inset-0 z-[29]" onclick={(e) => { e.stopPropagation(); close(); }}></div>
		<div class="absolute bottom-[calc(100%+4px)] left-0 w-[220px] bg-base-100 border border-base-300 rounded-xl shadow-lg z-30 p-1.5">
			<button
				class="flex items-center gap-2.5 w-full px-2.5 py-[7px] rounded-lg text-[13px] text-base-content hover:bg-base-200 transition-colors"
				onclick={() => navigate('/settings/account')}
			>
				<Settings class="w-[15px] h-[15px] text-base-content/60" />
				{$t('nav.settings')}
				<span class="ml-auto text-[11px] text-base-content/40 font-mono">⇧⌘,</span>
			</button>
			<button
				class="flex items-center gap-2.5 w-full px-2.5 py-[7px] rounded-lg text-[13px] text-base-content hover:bg-base-200 transition-colors"
				onclick={() => navigate('/upgrade')}
			>
				<CreditCard class="w-[15px] h-[15px] text-base-content/60" />
				Plans & Upgrade
			</button>
			<button
				class="flex items-center gap-2.5 w-full px-2.5 py-[7px] rounded-lg text-[13px] text-base-content hover:bg-base-200 transition-colors"
				onclick={() => window.open('https://neboloop.com/help', '_blank')}
			>
				<HelpCircle class="w-[15px] h-[15px] text-base-content/60" />
				Help
			</button>
			<div class="h-px bg-base-300 my-1"></div>
			<button
				class="flex items-center gap-2.5 w-full px-2.5 py-[7px] rounded-lg text-[13px] text-error hover:bg-base-200 transition-colors"
				onclick={handleLogout}
			>
				<LogOut class="w-[15px] h-[15px]" />
				Log out
			</button>
		</div>
	{/if}
</div>
