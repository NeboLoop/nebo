<script lang="ts">
  import Check from 'lucide-svelte/icons/check';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';

  interface Props {
    show: boolean;
    agentName?: string;
    onclose?: () => void;
  }

  let {
    show = $bindable(false),
    agentName = 'Agent',
    onclose,
  }: Props = $props();

  let step = $state(0);
  let activating = $state(false);
  let activated = $state(false);

  // Mock config inputs
  const inputs = [
    { id: 'repo', label: 'Repository', placeholder: 'owner/repo', value: '' },
    { id: 'branch', label: 'Default Branch', placeholder: 'main', value: 'main' },
    { id: 'notify', label: 'Notification Channel', placeholder: '#deployments', value: '' },
  ];

  let inputValues = $state(inputs.map(i => i.value));

  // Schedule options
  let scheduleType = $state('manual');

  function handleActivate() {
    activating = true;
    setTimeout(() => {
      activating = false;
      activated = true;
    }, 1500);
  }

  function handleClose() {
    show = false;
    setTimeout(() => {
      step = 0;
      activating = false;
      activated = false;
      inputValues = inputs.map(i => i.value);
      scheduleType = 'manual';
    }, 300);
    onclose?.();
  }

  const steps = ['Configure', 'Schedule', 'Activate'];
</script>

{#if show}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="presentation" onclick={handleClose} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); handleClose(); } }}></div>

    <div class="relative w-full max-w-md rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-sm font-bold">Set up {agentName}</h3>
        <div class="flex items-center gap-2">
          {#each steps as s, i}
            <div class="flex items-center gap-1.5">
              <div class="w-5 h-5 rounded-full flex items-center justify-center text-[0.625rem] font-bold {i < step ? 'bg-success text-success-content' : i === step ? 'bg-primary text-primary-content' : 'bg-base-200 text-base-content/40'}">
                {#if i < step}<Check class="w-3 h-3" />{:else}{i + 1}{/if}
              </div>
              {#if i < steps.length - 1}
                <div class="w-4 h-0.5 rounded {i < step ? 'bg-success' : 'bg-base-200'}"></div>
              {/if}
            </div>
          {/each}
        </div>
      </div>

      <div class="px-5 py-5">
        {#if step === 0}
          <!-- Configure inputs -->
          <p class="text-xs text-base-content/50 mb-4">Configure {agentName} for your workflow.</p>
          <div class="flex flex-col gap-3">
            {#each inputs as input, i}
              <div>
                <label class="text-sm font-medium mb-1 block" for="setup-{input.id}">{input.label}</label>
                <input
                  id="setup-{input.id}"
                  type="text"
                  bind:value={inputValues[i]}
                  placeholder={input.placeholder}
                  class="w-full py-2 px-3 rounded-lg border border-base-content/10 bg-base-100 text-sm outline-none focus:border-base-content/30"
                />
              </div>
            {/each}
          </div>

        {:else if step === 1}
          <!-- Schedule -->
          <p class="text-xs text-base-content/50 mb-4">How should {agentName} run?</p>
          <div class="flex flex-col gap-2">
            {#each [
              { id: 'manual', label: 'Manual', desc: 'Only when you ask' },
              { id: 'hourly', label: 'Every hour', desc: 'Checks and runs automatically' },
              { id: 'daily', label: 'Daily', desc: 'Runs once per day at 9:00 AM' },
              { id: 'event', label: 'On events', desc: 'Triggers on webhooks or system events' },
            ] as option}
              <label class="flex items-center gap-3 p-3 rounded-xl border cursor-pointer transition-colors {scheduleType === option.id ? 'border-primary/30 bg-primary/5' : 'border-base-content/10 hover:border-base-content/20'}">
                <input type="radio" name="schedule" value={option.id} bind:group={scheduleType} class="radio radio-sm radio-primary" />
                <div>
                  <div class="text-sm font-semibold">{option.label}</div>
                  <div class="text-xs text-base-content/50">{option.desc}</div>
                </div>
              </label>
            {/each}
          </div>

        {:else if step === 2}
          <!-- Activate -->
          <div class="text-center py-4">
            {#if activated}
              <div class="w-14 h-14 rounded-full bg-success/15 flex items-center justify-center mx-auto mb-4">
                <Check class="w-7 h-7 text-success" />
              </div>
              <h3 class="text-lg font-bold mb-1">{agentName} is ready!</h3>
              <p class="text-xs text-base-content/50">Your agent is configured and active.</p>
            {:else if activating}
              <div class="w-14 h-14 rounded-full bg-primary/15 flex items-center justify-center mx-auto mb-4">
                <span class="loading loading-spinner loading-md text-primary"></span>
              </div>
              <h3 class="text-lg font-bold mb-1">Activating {agentName}...</h3>
              <p class="text-xs text-base-content/50">Setting up your agent configuration.</p>
            {:else}
              <div class="w-14 h-14 rounded-xl bg-base-200 flex items-center justify-center mx-auto mb-4 text-2xl font-bold text-base-content/30">
                {agentName.charAt(0)}
              </div>
              <h3 class="text-lg font-bold mb-1">Ready to activate</h3>
              <p class="text-xs text-base-content/50 mb-4">{agentName} will start with your configured settings.</p>
            {/if}
          </div>
        {/if}
      </div>

      <!-- Footer -->
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
        {#if activated}
          <button
            onclick={handleClose}
            class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
          >
            Done
          </button>
        {:else}
          <button
            onclick={handleClose}
            class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
          >
            Cancel
          </button>
          {#if step < 2}
            <button
              onclick={() => (step += 1)}
              class="flex items-center gap-1.5 px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
            >
              Next <ArrowRight class="w-3.5 h-3.5" />
            </button>
          {:else if !activating}
            <button
              onclick={handleActivate}
              class="px-4 py-2 rounded-lg bg-success text-success-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
            >
              Activate
            </button>
          {/if}
        {/if}
      </div>
    </div>
  </div>
{/if}
