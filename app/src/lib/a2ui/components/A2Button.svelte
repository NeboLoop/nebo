<script lang="ts">
  let { label = '', variant = 'default', size = 'sm', action, onaction }: {
    label?: string;
    variant?: string;
    size?: string;
    action?: { name: string; payload?: Record<string, unknown> };
    onaction?: (name: string, payload?: Record<string, unknown>) => void;
  } = $props();

  const variantMap: Record<string, string> = {
    default: 'btn btn-sm bg-base-100 border border-base-300 hover:bg-base-200',
    primary: 'btn btn-sm btn-primary',
    ghost: 'btn btn-sm btn-ghost',
    accent: 'btn btn-sm bg-base-content text-base-100 hover:opacity-90 border-none',
  };

  const sizeMap: Record<string, string> = {
    xs: 'btn-xs',
    sm: 'btn-sm',
    md: '',
  };

  function handleClick() {
    if (action && onaction) {
      onaction(action.name, action.payload);
    }
  }
</script>

<button
  class="{variantMap[variant] ?? variantMap.default} {sizeMap[size] ?? ''} cursor-pointer"
  onclick={handleClick}
>{label}</button>
