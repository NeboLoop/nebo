<script lang="ts">
	import { onMount } from 'svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { getAgentProfile, updateAgentProfile } from '$lib/api/nebo';
	import NeboIcon from '$lib/components/icons/NeboIcon.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let saveError = $state(false);

	let agentName = $state('Nebo');
	let emoji = $state('');
	let creature = $state('');
	let role = $state('');
	let vibe = $state('');
	let avatar = $state('');

	onMount(async () => {
		try {
			const data = await getAgentProfile();
			if (data) {
				agentName = data.name || 'Nebo';
				emoji = data.emoji || '';
				creature = data.creature || '';
				role = data.role || '';
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
				role,
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
	const displayRole = $derived(role || '');
	const displayVibe = $derived(vibe || '');
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Identity</h2>
	<p class="text-base text-base-content/80">Your agent's name, avatar, and persona</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading identity...</span>
	</div>
{:else}
	<form
		onsubmit={(e) => {
			e.preventDefault();
			saveIdentity();
		}}
		class="space-y-6"
	>
		<!-- Avatar -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Avatar</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex items-start gap-5">
					<label class="identity-avatar-trigger w-20 h-20 rounded-2xl bg-base-content/5 flex items-center justify-center overflow-hidden shrink-0 border border-base-content/10 cursor-pointer relative group">
						{#if avatar}
							<img src={avatar} alt="Avatar" class="w-full h-full object-cover" />
						{:else}
							<NeboIcon class="w-14 h-14" />
						{/if}
						<div class="identity-avatar-overlay absolute inset-0 rounded-2xl bg-black/40 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity">
							<span class="text-white text-base font-medium">Change</span>
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
						<p class="text-base text-base-content/80">{displayCreature}</p>
						{#if displayRole}
							<p class="text-base text-base-content/80 mt-0.5">{displayRole}</p>
						{/if}
						{#if displayVibe}
							<p class="text-base text-base-content/80 italic mt-1">"{displayVibe}"</p>
						{/if}
					</div>
				</div>
				{#if avatar}
					<div class="flex justify-start mt-3">
						<button type="button" class="text-base text-base-content/80 hover:text-error transition-colors" onclick={clearAvatar}>
							Remove custom avatar
						</button>
					</div>
				{:else}
					<p class="text-base text-base-content/80 mt-3">Click the avatar to upload a custom image</p>
				{/if}
			</div>
		</section>

		<!-- Identity -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Identity</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				<div class="grid sm:grid-cols-2 gap-4">
					<div>
						<label class="text-base font-medium text-base-content/80" for="agent-name">
							What should your agent be called?
						</label>
						<input
							id="agent-name"
							type="text"
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							placeholder="Nebo"
							bind:value={agentName}
						/>
					</div>
					<div>
						<label class="text-base font-medium text-base-content/80" for="agent-emoji">
							Emoji
						</label>
						<input
							id="agent-emoji"
							type="text"
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							placeholder="Used in chat bubbles and notifications"
							bind:value={emoji}
						/>
					</div>
				</div>

				<div>
					<label class="text-base font-medium text-base-content/80" for="agent-role">
						What's your relationship dynamic?
					</label>
					<input
						id="agent-role"
						type="text"
						class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
						placeholder="Friend, Mentor, Coach, COO..."
						bind:value={role}
					/>
				</div>
			</div>
		</section>

		<!-- Persona -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Persona</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				<div>
					<label class="text-base font-medium text-base-content/80" for="agent-creature">
						What archetype does it embody?
					</label>
					<input
						id="agent-creature"
						type="text"
						class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
						placeholder="Helpful sidekick, Sarcastic librarian, Rogue diplomat..."
						bind:value={creature}
					/>
				</div>

				<div>
					<label class="text-base font-medium text-base-content/80" for="agent-vibe">
						What's the vibe?
					</label>
					<textarea
						id="agent-vibe"
						class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base focus:outline-none focus:border-primary/50 transition-colors resize-none"
						rows="2"
						placeholder="chill but opinionated, dry humor"
						bind:value={vibe}
					></textarea>
				</div>
			</div>
		</section>

		<!-- Save -->
		{#if saveMessage}
			<Alert type={saveError ? 'error' : 'success'} title={saveError ? 'Error' : 'Saved'}>
				{saveMessage}
			</Alert>
		{/if}

		<div class="flex justify-end">
			<button
				type="submit"
				disabled={isSaving}
				class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
			>
				{#if isSaving}
					<Spinner size={16} />
					Saving...
				{:else}
					Save Identity
				{/if}
			</button>
		</div>
	</form>
{/if}
