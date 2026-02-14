<script lang="ts">
	import Modal from '$lib/components/ui/Modal.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Save, Loader2 } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { ExtensionSkill } from '$lib/api/nebo';

	interface Props {
		show: boolean;
		skill?: ExtensionSkill | null;
		onclose: () => void;
		onsaved: () => void;
	}

	let { show = $bindable(false), skill = null, onclose, onsaved }: Props = $props();

	let content = $state('');
	let slug = $state('');
	let saving = $state(false);
	let loading = $state(false);
	let error = $state('');

	let mode = $derived(skill ? 'edit' : 'create');
	let title = $derived(mode === 'create' ? 'Create Skill' : `Edit: ${skill?.name}`);

	const defaultTemplate = `---
name: my-skill
description: A brief description of what this skill does
version: "1.0.0"
tags:
  - general
tools: []
priority: 10
---

# My Skill

Instructions for the agent when this skill is activated...
`;

	$effect(() => {
		if (show) {
			error = '';
			if (mode === 'create') {
				content = defaultTemplate;
				slug = '';
			} else if (skill) {
				loadContent(skill.name);
			}
		}
	});

	async function loadContent(name: string) {
		loading = true;
		try {
			const resp = await api.getSkillContent(name);
			content = resp.content;
		} catch (e: any) {
			error = e.message || 'Failed to load skill content';
		} finally {
			loading = false;
		}
	}

	async function handleSave() {
		saving = true;
		error = '';
		try {
			if (mode === 'create') {
				await api.createSkill({ content, slug: slug || undefined });
			} else if (skill) {
				await api.updateSkill({ content }, skill.name);
			}
			show = false;
			onsaved();
		} catch (e: any) {
			error = e.message || 'Failed to save skill';
		} finally {
			saving = false;
		}
	}
</script>

<Modal bind:show {title} size="lg" onclose={() => { show = false; onclose(); }}>
	{#if loading}
		<div class="py-12 text-center text-base-content/60">
			<span class="loading loading-spinner loading-md"></span>
			<p class="mt-2">Loading skill content...</p>
		</div>
	{:else}
		{#if error}
			<div class="alert alert-error mb-4">
				<span>{error}</span>
			</div>
		{/if}

		{#if mode === 'create'}
			<div class="form-control mb-4">
				<label class="label" for="skill-slug">
					<span class="label-text">Slug (directory name)</span>
				</label>
				<input
					id="skill-slug"
					type="text"
					class="input input-bordered w-full"
					placeholder="my-skill"
					bind:value={slug}
				/>
				<label class="label" for="skill-slug">
					<span class="label-text-alt text-base-content/50">Leave empty to auto-derive from skill name</span>
				</label>
			</div>
		{/if}

		<div class="form-control mb-4">
			<label class="label" for="skill-content">
				<span class="label-text">SKILL.md Content</span>
			</label>
			<textarea
				id="skill-content"
				class="textarea textarea-bordered w-full font-mono text-sm leading-relaxed"
				rows="20"
				bind:value={content}
				placeholder="---\nname: my-skill\n..."
			></textarea>
		</div>
	{/if}

	{#snippet footer()}
		<div class="flex justify-end gap-2">
			<Button type="ghost" onclick={() => { show = false; onclose(); }}>
				Cancel
			</Button>
			<Button type="primary" onclick={handleSave} disabled={saving || loading || !content.trim()}>
				{#if saving}
					<Loader2 class="w-4 h-4 mr-2 animate-spin" />
					Saving...
				{:else}
					<Save class="w-4 h-4 mr-2" />
					{mode === 'create' ? 'Create' : 'Save'}
				{/if}
			</Button>
		</div>
	{/snippet}
</Modal>
