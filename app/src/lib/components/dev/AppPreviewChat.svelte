<!--
  AppPreviewChat — Inline conversational preview for app structured UI.
  Shows how an app's UI blocks appear within agent messages.
  Apps are conversation: Nebo manages apps, the human optionally interacts.
-->

<script lang="ts">
	import { tick } from 'svelte';
	import { Bot, Send, Loader2, RotateCcw } from 'lucide-svelte';
	import UIBlock from '$lib/components/appui/UIBlock.svelte';
	import { getUIView, sendUIEvent } from '$lib/api/nebo';
	import type { UIView } from '$lib/api/nebo';
	import { generateUUID } from '$lib/utils';

	interface Props {
		appId: string;
	}

	let { appId }: Props = $props();

	interface PreviewMessage {
		id: string;
		role: 'agent' | 'user';
		text?: string;
		view?: UIView;
		toast?: string;
		error?: string;
	}

	let messages = $state<PreviewMessage[]>([]);
	let loading = $state(false);
	let inputValue = $state('');
	let messagesContainer: HTMLDivElement;

	// Reload view when app changes
	$effect(() => {
		if (appId) {
			loadAppView();
		}
	});

	// Auto-scroll on new messages
	$effect(() => {
		if (messages.length > 0 && messagesContainer) {
			tick().then(() => {
				if (messagesContainer) {
					messagesContainer.scrollTo({ top: messagesContainer.scrollHeight, behavior: 'smooth' });
				}
			});
		}
	});

	async function loadAppView() {
		loading = true;
		messages = [];
		try {
			const view = await getUIView(appId);
			messages = [{
				id: generateUUID(),
				role: 'agent',
				text: view.title || 'App loaded',
				view
			}];
		} catch (err: any) {
			messages = [{
				id: generateUUID(),
				role: 'agent',
				error: err.message || 'Failed to load app view'
			}];
		} finally {
			loading = false;
		}
	}

	async function handleBlockEvent(blockId: string, action: string, value: string) {
		const currentView = messages.findLast(m => m.view)?.view;
		if (!currentView || !appId) return;

		try {
			const resp = await sendUIEvent({
				view_id: currentView.view_id,
				block_id: blockId,
				action,
				value
			}, appId);

			const msg: PreviewMessage = {
				id: generateUUID(),
				role: 'agent'
			};

			if (resp.toast) msg.text = resp.toast;
			if (resp.error) msg.error = resp.error;
			if (resp.view) msg.view = resp.view;

			// Only add a message if there's something to show
			if (msg.text || msg.error || msg.view) {
				messages = [...messages, msg];
			}
		} catch (err: any) {
			messages = [...messages, {
				id: generateUUID(),
				role: 'agent',
				error: err.message || 'Event failed'
			}];
		}
	}

	function sendTestMessage() {
		if (!inputValue.trim()) return;
		const text = inputValue.trim();
		inputValue = '';

		// Add user message
		messages = [...messages, {
			id: generateUUID(),
			role: 'user',
			text
		}];

		// Mock agent response: reload the app's current view
		if (appId) {
			loading = true;
			getUIView(appId)
				.then(view => {
					messages = [...messages, {
						id: generateUUID(),
						role: 'agent',
						text: `Here's the current view:`,
						view
					}];
				})
				.catch(err => {
					messages = [...messages, {
						id: generateUUID(),
						role: 'agent',
						error: err.message || 'Failed to get view'
					}];
				})
				.finally(() => {
					loading = false;
				});
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			sendTestMessage();
		}
	}

	function resetPreview() {
		messages = [];
		if (appId) loadAppView();
	}
</script>

<div class="flex flex-col h-full">
	<!-- Header -->
	<div class="flex items-center gap-2 px-3 py-2 border-b border-base-300 bg-base-100 shrink-0">
		<span class="text-xs text-base-content/40 flex-1">Inline Preview</span>

		<button
			type="button"
			class="btn btn-xs btn-ghost"
			onclick={resetPreview}
			title="Reload view"
		>
			<RotateCcw class="w-3.5 h-3.5" />
		</button>
	</div>

	<!-- Messages Area -->
	<div
		bind:this={messagesContainer}
		class="flex-1 min-h-0 overflow-y-auto overscroll-contain p-4 space-y-4"
	>
		{#if !appId}
			<div class="flex flex-col items-center justify-center h-full text-base-content/50 gap-2">
				<Bot class="w-8 h-8" />
				<p class="text-sm font-medium">No App Selected</p>
				<p class="text-xs">Sideload an app in Settings &gt; Developer to preview</p>
			</div>
		{:else if loading && messages.length === 0}
			<div class="flex flex-col items-center justify-center h-full gap-2">
				<Loader2 class="w-6 h-6 text-base-content/40 animate-spin" />
				<p class="text-xs text-base-content/50">Loading app view...</p>
			</div>
		{:else if messages.length === 0}
			<div class="flex flex-col items-center justify-center h-full text-base-content/50 gap-2">
				<Bot class="w-8 h-8" />
				<p class="text-sm">No view available</p>
			</div>
		{:else}
			{#each messages as msg (msg.id)}
				{#if msg.role === 'user'}
					<!-- User bubble (right-aligned) -->
					<div class="flex justify-end">
						<div class="rounded-2xl rounded-br-md px-4 py-2.5 bg-primary text-primary-content max-w-[85%]">
							<p class="text-sm">{msg.text}</p>
						</div>
					</div>
				{:else}
					<!-- Agent bubble (left-aligned with avatar) -->
					<div class="flex gap-3">
						<div class="w-7 h-7 rounded-lg shrink-0 self-end mb-1 grid place-items-center bg-base-300 text-base-content/60 text-xs font-semibold">
							N
						</div>
						<div class="flex flex-col gap-2 max-w-full min-w-0 flex-1">
							{#if msg.error}
								<div class="rounded-2xl rounded-bl-md px-4 py-2.5 bg-error/10 text-error text-sm">
									{msg.error}
								</div>
							{/if}

							{#if msg.text}
								<div class="rounded-2xl rounded-bl-md px-4 py-2.5 bg-base-200 text-sm">
									{msg.text}
								</div>
							{/if}

							{#if msg.view?.blocks?.length}
								<div class="rounded-2xl rounded-bl-md px-4 py-3 bg-base-200 space-y-3 w-full">
									{#each msg.view.blocks as block (block.block_id)}
										<UIBlock {block} onEvent={handleBlockEvent} />
									{/each}
								</div>
							{/if}
						</div>
					</div>
				{/if}
			{/each}

			{#if loading}
				<div class="flex gap-3">
					<div class="w-7 h-7 rounded-lg shrink-0 self-end mb-1 grid place-items-center bg-base-300 text-base-content/60 text-xs font-semibold">
						N
					</div>
					<div class="rounded-2xl rounded-bl-md px-4 py-2.5 bg-base-200">
						<Loader2 class="w-4 h-4 animate-spin text-base-content/40" />
					</div>
				</div>
			{/if}
		{/if}
	</div>

	<!-- Input Area -->
	<div class="shrink-0 border-t border-base-300 p-3">
		<div class="flex items-center gap-2">
			<input
				type="text"
				bind:value={inputValue}
				onkeydown={handleKeydown}
				placeholder="Type a test message..."
				class="input input-bordered input-sm flex-1"
				disabled={!appId || loading}
			/>
			<button
				type="button"
				class="btn btn-sm btn-primary"
				onclick={sendTestMessage}
				disabled={!inputValue.trim() || !appId || loading}
			>
				<Send class="w-4 h-4" />
			</button>
		</div>
	</div>
</div>
