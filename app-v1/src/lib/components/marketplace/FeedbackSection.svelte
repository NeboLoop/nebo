<script lang="ts">
	import { Bug, Lightbulb, HelpCircle } from 'lucide-svelte';
	import webapi from '$lib/api/gocliRequest';

	interface FeedbackItem {
		id: string;
		targetId: string;
		targetType: string;
		userId: string;
		userName?: string;
		user_name?: string;
		type: string;
		title: string;
		body?: string;
		status: string;
		createdAt?: string;
		created_at?: string;
	}

	let {
		feedback = [],
		targetId,
		targetType
	}: {
		feedback: FeedbackItem[];
		targetId: string;
		targetType: string;
	} = $props();

	let showForm = $state(false);
	let fbType = $state<'bug' | 'feature' | 'question'>('bug');
	let fbTitle = $state('');
	let fbBody = $state('');
	let submitting = $state(false);
	let items = $state<FeedbackItem[]>(feedback);

	$effect(() => {
		items = feedback;
	});

	const typeIcons: Record<string, typeof Bug> = {
		bug: Bug,
		feature: Lightbulb,
		question: HelpCircle
	};

	const typeLabels: Record<string, string> = {
		bug: 'Bug Report',
		feature: 'Feature Request',
		question: 'Question'
	};

	const statusColors: Record<string, string> = {
		open: 'badge-warning',
		acknowledged: 'badge-info',
		resolved: 'badge-success',
		closed: 'badge-neutral'
	};

	function timeAgo(dateStr: string) {
		if (!dateStr) return '';
		const diff = Date.now() - new Date(dateStr).getTime();
		const mins = Math.floor(diff / 60000);
		if (mins < 60) return `${mins}m ago`;
		const hours = Math.floor(mins / 60);
		if (hours < 24) return `${hours}h ago`;
		const days = Math.floor(hours / 24);
		if (days < 30) return `${days}d ago`;
		return `${Math.floor(days / 30)}mo ago`;
	}

	async function submitFeedback() {
		if (!fbTitle.trim()) return;
		submitting = true;
		try {
			await webapi.post(`/api/v1/store/products/${targetId}/feedback`, { type: fbType, title: fbTitle, body: fbBody });
			showForm = false;
			fbTitle = '';
			fbBody = '';
			// Refresh feedback list
			const res = await webapi.get(`/api/v1/store/products/${targetId}/feedback`);
			items = (res as any).feedback ?? [];
		} catch { /* ignore */ }
		submitting = false;
	}
</script>

<div class="px-5 py-5 border-b border-base-content/5">
	<div class="flex items-center justify-between mb-4">
		<h3 class="font-display text-lg font-bold">Feedback</h3>
		<button type="button" onclick={() => showForm = true} class="text-base text-primary font-medium">Submit Feedback</button>
	</div>

	{#if showForm}
		<div class="rounded-2xl bg-base-content/[0.04] p-4 mb-4">
			<div class="flex gap-2 mb-3">
				{#each (['bug', 'feature', 'question'] as const) as t}
					<button type="button" onclick={() => fbType = t} class="flex items-center gap-1.5 px-3 py-1.5 rounded-full text-sm font-medium transition-colors {fbType === t ? 'bg-primary/15 text-primary' : 'bg-base-content/5 text-base-content/60 hover:bg-base-content/10'}">
						<svelte:component this={typeIcons[t]} class="w-3 h-3" />
						{typeLabels[t]}
					</button>
				{/each}
			</div>
			<input type="text" bind:value={fbTitle} placeholder="Title" class="w-full rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-2.5 text-base placeholder:text-base-content/40 focus:outline-none focus:border-primary/40 transition-colors mb-2" />
			<textarea bind:value={fbBody} rows="3" placeholder="Description (optional)" class="w-full rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-2.5 text-base placeholder:text-base-content/40 focus:outline-none focus:border-primary/40 transition-colors resize-none"></textarea>
			<div class="flex gap-2 mt-3">
				<button type="button" onclick={() => showForm = false} class="flex-1 h-9 rounded-xl border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors">Cancel</button>
				<button type="button" onclick={submitFeedback} disabled={!fbTitle.trim() || submitting} class="flex-1 h-9 rounded-xl bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30">
					{submitting ? 'Submitting...' : 'Submit'}
				</button>
			</div>
		</div>
	{/if}

	{#if items.length > 0}
		<div class="space-y-2">
			{#each items as fb}
				<div class="rounded-xl bg-base-content/[0.04] p-3.5">
					<div class="flex items-start gap-3">
						<div class="w-7 h-7 rounded-lg bg-base-content/5 flex items-center justify-center shrink-0 mt-0.5">
							<svelte:component this={typeIcons[fb.type] ?? Bug} class="w-3.5 h-3.5 text-base-content/40" />
						</div>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2 mb-1">
								<span class="text-base font-semibold truncate">{fb.title}</span>
								<span class="badge badge-xs {statusColors[fb.status] ?? 'badge-neutral'}">{fb.status}</span>
							</div>
							{#if fb.body}
								<p class="text-sm text-base-content/60 line-clamp-2">{fb.body}</p>
							{/if}
							<div class="flex items-center gap-2 mt-1.5">
								<span class="text-sm text-base-content/40">{fb.userName ?? fb.user_name ?? 'User'}</span>
								<span class="text-sm text-base-content/40">{timeAgo(fb.createdAt ?? fb.created_at ?? '')}</span>
							</div>
						</div>
					</div>
				</div>
			{/each}
		</div>
	{:else if !showForm}
		<p class="text-base text-base-content/40 text-center py-4">No feedback yet</p>
	{/if}
</div>
