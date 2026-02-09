<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { Fingerprint, Save } from 'lucide-svelte';
	import { getAgentProfile, updateAgentProfile } from '$lib/api/nebo';
	import NeboIcon from '$lib/components/icons/NeboIcon.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let saveError = $state(false);

	let agentName = $state('Nebo');
	let emoji = $state('');
	let creature = $state('');
	let vibe = $state('');
	let avatar = $state('');

	onMount(async () => {
		try {
			const data = await getAgentProfile();
			if (data) {
				agentName = data.name || 'Nebo';
				emoji = data.emoji || '';
				creature = data.creature || '';
				vibe = data.vibe || '';
				avatar = data.avatar || '';
			}
		} catch (error) {
			console.error('Failed to load profile:', error);
		} finally {
			isLoading = false;
		}
	});

	async function saveIdentity() {
		isSaving = true;
		saveMessage = '';
		saveError = false;
		try {
			await updateAgentProfile({
				name: agentName,
				emoji,
				creature,
				vibe,
				avatar
			});
			saveMessage = 'Identity saved';
			saveError = false;
			setTimeout(() => (saveMessage = ''), 3000);
		} catch (error) {
			console.error('Failed to save identity:', error);
			saveMessage = 'Failed to save';
			saveError = true;
		} finally {
			isSaving = false;
		}
	}

	function handleAvatarUpload(e: Event) {
		const input = e.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;

		if (file.size > 512 * 1024) {
			saveMessage = 'Avatar too large (max 512KB)';
			saveError = true;
			setTimeout(() => (saveMessage = ''), 4000);
			return;
		}

		const reader = new FileReader();
		reader.onload = () => {
			avatar = reader.result as string;
		};
		reader.readAsDataURL(file);
	}

	function clearAvatar() {
		avatar = '';
	}

	const displayName = $derived(agentName || 'Nebo');
	const displayCreature = $derived(creature || 'AI Agent');
	const displayVibe = $derived(vibe || '');
</script>

{#if isLoading}
	<Card>
		<div class="flex items-center justify-center gap-3 py-8">
			<Spinner size={20} />
			<span class="text-sm text-base-content/60">Loading identity...</span>
		</div>
	</Card>
{:else}
	<form
		onsubmit={(e) => {
			e.preventDefault();
			saveIdentity();
		}}
	>
		<!-- Character Card Preview -->
		<div class="rounded-2xl border border-base-300 bg-gradient-to-br from-base-200/50 to-base-100 p-6 mb-6">
			<div class="flex items-start gap-5">
				<!-- Avatar — click to change -->
				<label class="identity-avatar-trigger w-20 h-20 rounded-2xl bg-base-200 flex items-center justify-center overflow-hidden shrink-0 border border-base-300 cursor-pointer relative group">
					{#if avatar}
						<img src={avatar} alt="Avatar" class="w-full h-full object-cover" />
					{:else}
						<NeboIcon class="w-14 h-14" />
					{/if}
					<div class="identity-avatar-overlay absolute inset-0 rounded-2xl bg-black/40 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity">
						<span class="text-white text-xs font-medium">Change</span>
					</div>
					<input
						type="file"
						class="hidden"
						accept="image/png,image/jpeg,image/gif,image/webp"
						onchange={handleAvatarUpload}
					/>
				</label>

				<div class="flex-1 min-w-0">
					<h2 class="text-xl font-bold text-base-content truncate">
						{#if emoji && !avatar}<span class="mr-1.5">{emoji}</span>{/if}{displayName}
					</h2>
					<p class="text-sm text-base-content/60">{displayCreature}</p>
					{#if displayVibe}
						<p class="text-sm text-base-content/40 italic mt-1">"{displayVibe}"</p>
					{/if}
				</div>
			</div>
			{#if avatar}
				<div class="flex justify-start mt-3">
					<button type="button" class="text-xs text-base-content/40 hover:text-error transition-colors" onclick={clearAvatar}>
						Remove custom avatar
					</button>
				</div>
			{:else}
				<p class="text-xs text-base-content/30 mt-3">Click the avatar to upload a custom image</p>
			{/if}
		</div>

		<Card>
			<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider mb-4">Identity</h3>

			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="agent-name">
						Name
					</label>
					<input
						id="agent-name"
						type="text"
						class="input input-bordered input-sm w-full max-w-xs"
						placeholder="Nebo"
						bind:value={agentName}
					/>
					<p class="text-xs text-base-content/30 mt-1">What your agent calls itself</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="agent-creature">
						Creature
					</label>
					<textarea
						id="agent-creature"
						class="textarea textarea-bordered textarea-sm w-full resize-none"
						rows="1"
						placeholder="Helpful sidekick, Sarcastic librarian, Rogue diplomat..."
						bind:value={creature}
					></textarea>
					<p class="text-xs text-base-content/30 mt-1">The archetype your agent embodies</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="agent-vibe">
						Vibe
					</label>
					<textarea
						id="agent-vibe"
						class="textarea textarea-bordered textarea-sm w-full resize-none"
						rows="2"
						placeholder="chill but opinionated, dry humor"
						bind:value={vibe}
					></textarea>
					<p class="text-xs text-base-content/30 mt-1">Your agent's energy in a few words</p>
				</div>

				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="agent-emoji">
						Emoji
					</label>
					<input
						id="agent-emoji"
						type="text"
						class="input input-bordered input-sm w-20"
						placeholder="🤖"
						bind:value={emoji}
					/>
					<p class="text-xs text-base-content/30 mt-1">Shows up in chat bubbles and notifications</p>
				</div>
			</div>

		</Card>

		{#if saveMessage}
			<div class="mt-4">
				<Alert type={saveError ? 'error' : 'success'} title={saveError ? 'Error' : 'Saved'}>
					{saveMessage}
				</Alert>
			</div>
		{/if}

		<div class="flex justify-end mt-4">
			<Button type="primary" htmlType="submit" disabled={isSaving}>
				{#if isSaving}
					<Spinner size={16} />
					Saving...
				{:else}
					Save Identity
				{/if}
			</Button>
		</div>
	</form>
{/if}
