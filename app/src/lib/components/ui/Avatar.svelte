<!--
  Avatar Component
  User profile images with fallback initials
-->

<script lang="ts">
	interface Props {
		src?: string;
		alt?: string;
		initials?: string;
		size?: 'xs' | 'sm' | 'md' | 'lg' | 'xl';
		status?: 'online' | 'offline';
		rounded?: boolean;
		ring?: boolean;
		ringColor?: 'primary' | 'secondary' | 'accent' | 'success' | 'warning' | 'error' | 'info';
		class?: string;
	}

	let {
		src,
		alt = '',
		initials = '',
		size = 'md',
		status,
		rounded = true,
		ring = false,
		ringColor = 'primary',
		class: className = ''
	}: Props = $props();

	let imageError = $state(false);

	const sizeClasses: Record<string, string> = {
		xs: 'w-6',
		sm: 'w-8',
		md: 'w-10',
		lg: 'w-12',
		xl: 'w-16'
	};

	const textSizeClasses: Record<string, string> = {
		xs: 'text-xs',
		sm: 'text-sm',
		md: 'text-base',
		lg: 'text-lg',
		xl: 'text-xl'
	};

	const ringClasses: Record<string, string> = {
		primary: 'ring-primary',
		secondary: 'ring-secondary',
		accent: 'ring-accent',
		success: 'ring-success',
		warning: 'ring-warning',
		error: 'ring-error',
		info: 'ring-info'
	};

	let avatarWrapperClass = $derived(
		`avatar ${status ? status : ''} ${className}`.trim()
	);

	let avatarClass = $derived(
		`${sizeClasses[size]} ${rounded ? 'rounded-full' : 'rounded-lg'} ${ring ? `ring ring-offset-base-100 ring-offset-2 ${ringClasses[ringColor]}` : ''}`.trim()
	);

	let placeholderClass = $derived(
		`bg-neutral text-neutral-content ${sizeClasses[size]} ${rounded ? 'rounded-full' : 'rounded-lg'} ${ring ? `ring ring-offset-base-100 ring-offset-2 ${ringClasses[ringColor]}` : ''}`.trim()
	);
</script>

<div class={avatarWrapperClass}>
	{#if src && !imageError}
		<div class={avatarClass}>
			<img
				{src}
				{alt}
				onerror={() => imageError = true}
			/>
		</div>
	{:else}
		<div class="avatar placeholder">
			<div class={placeholderClass}>
				<span class={textSizeClasses[size]}>
					{#if initials}
						{initials}
					{:else}
						<i class="fas fa-user"></i>
					{/if}
				</span>
			</div>
		</div>
	{/if}
</div>
