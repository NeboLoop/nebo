<script lang="ts">
	import { onMount } from 'svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import { CheckCircle, AlertCircle, Trash2, Lock, Zap } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import { t } from 'svelte-i18n';

	interface SecretInfo {
		key: string;
		label: string;
		hint: string;
		required: boolean;
		configured: boolean;
	}

	interface SkillWithSecrets {
		name: string;
		description: string;
		enabled: boolean;
		secrets: SecretInfo[];
	}

	let isLoading = $state(true);
	let error = $state('');
	let skills = $state<SkillWithSecrets[]>([]);
	let settingSecret = $state<string | null>(null);
	let secretInputs = $state<Record<string, string>>({});
	let successMsg = $state('');

	onMount(async () => {
		await loadSecrets();
	});

	async function loadSecrets() {
		isLoading = true;
		error = '';
		try {
			const resp = await api.listExtensions();
			const allSkills = resp.skills || [];

			// Filter to skills that declare secrets
			const withSecrets: SkillWithSecrets[] = [];
			for (const skill of allSkills) {
				const s = skill as any;
				if (s.secrets && Array.isArray(s.secrets) && s.secrets.length > 0) {
					withSecrets.push({
						name: skill.name,
						description: skill.description,
						enabled: skill.enabled,
						secrets: s.secrets,
					});
				}
			}
			skills = withSecrets;
		} catch (err: any) {
			error = err?.message || $t('settingsSecrets.loadFailed');
		} finally {
			isLoading = false;
		}
	}

	async function saveSecret(skillName: string, key: string) {
		const inputKey = `${skillName}:${key}`;
		const value = secretInputs[inputKey];
		if (!value) return;
		settingSecret = inputKey;
		successMsg = '';
		try {
			await api.setSkillSecret(skillName, key, value);
			secretInputs[inputKey] = '';
			successMsg = $t('settingsSecrets.secretSaved', { values: { key, skill: skillName } });
			await loadSecrets();
			setTimeout(() => successMsg = '', 3000);
		} catch (err: any) {
			error = err?.message || $t('settingsSecrets.saveFailed');
		} finally {
			settingSecret = null;
		}
	}

	async function removeSecret(skillName: string, key: string) {
		const inputKey = `${skillName}:${key}`;
		settingSecret = inputKey;
		try {
			await api.deleteSkillSecret(skillName, key);
			await loadSecrets();
		} catch (err: any) {
			error = err?.message || $t('settingsSecrets.removeFailed');
		} finally {
			settingSecret = null;
		}
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsSecrets.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsSecrets.description')}</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('common.loading')}</span>
	</div>
{:else}
	<div class="space-y-6">
		{#if error}
			<Alert type="error" title={$t('common.error')}>{error}</Alert>
		{/if}

		{#if successMsg}
			<Alert type="success" title={$t('common.saved')}>{successMsg}</Alert>
		{/if}

		{#if skills.length === 0}
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="py-8 text-center">
					<Lock class="w-10 h-10 mx-auto mb-3 text-base-content/40" />
					<p class="text-base font-medium text-base-content/80 mb-1">{$t('settingsSecrets.noSecrets')}</p>
					<p class="text-sm text-base-content/60 mb-4">{$t('settingsSecrets.noSecretsDesc')}</p>
					<a
						href="/marketplace/skills"
						class="inline-block h-10 px-6 leading-10 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
					>
						{$t('settingsSecrets.browseSkills')}
					</a>
				</div>
			</div>
		{:else}
			{#each skills as skill (skill.name)}
				<section>
					<div class="flex items-center gap-2 mb-3">
						<Zap class="w-4 h-4 text-primary" />
						<h3 class="text-base font-semibold text-base-content">{skill.name}</h3>
						{#if !skill.enabled}
							<span class="text-xs text-base-content/40">{$t('common.disabled')}</span>
						{/if}
					</div>
					<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-3">
						{#each skill.secrets as secret (secret.key)}
							<div class="flex items-start gap-3 py-2">
								<div class="flex-1 min-w-0">
									<div class="flex items-center gap-2 mb-0.5">
										<span class="text-base font-medium text-base-content">{secret.label || secret.key}</span>
										{#if secret.required}
											<span class="text-xs text-error/80">{$t('common.required')}</span>
										{/if}
										{#if secret.configured}
											<CheckCircle class="w-3.5 h-3.5 text-success" />
										{:else}
											<AlertCircle class="w-3.5 h-3.5 text-warning" />
										{/if}
									</div>
									{#if secret.hint}
										<p class="text-sm text-base-content/50">{secret.hint}</p>
									{/if}

									{#if secret.configured}
										<div class="flex items-center gap-2 mt-2">
											<span class="text-sm text-success/80">{$t('common.configured')}</span>
											<button
												type="button"
												class="text-sm text-base-content/40 hover:text-error transition-colors"
												onclick={() => removeSecret(skill.name, secret.key)}
												disabled={settingSecret === `${skill.name}:${secret.key}`}
											>
												<Trash2 class="w-3.5 h-3.5" />
											</button>
										</div>
									{:else}
										<div class="flex gap-2 mt-2">
											<input
												type="password"
												placeholder={secret.key}
												bind:value={secretInputs[`${skill.name}:${secret.key}`]}
												class="flex-1 h-9 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
											/>
											<button
												type="button"
												class="h-9 px-4 rounded-xl bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all disabled:opacity-50"
												onclick={() => saveSecret(skill.name, secret.key)}
												disabled={settingSecret === `${skill.name}:${secret.key}` || !secretInputs[`${skill.name}:${secret.key}`]}
											>
												{settingSecret === `${skill.name}:${secret.key}` ? '...' : $t('common.save')}
											</button>
										</div>
									{/if}
								</div>
							</div>
						{/each}
					</div>
				</section>
			{/each}
		{/if}
	</div>
{/if}
