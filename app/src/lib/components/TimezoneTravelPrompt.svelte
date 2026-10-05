<!--
  The owner's time zone and language belong to their account. On load this
  reports what the device detects (kept only while none is known); when the
  device is somewhere else than the zone in force, it asks once whether to
  switch — never silently, because schedules run on that clock. A "no" is
  remembered for that zone, so travel does not nag.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { t, locale } from 'svelte-i18n';
  import { storage } from '$lib/storage';

  /** The device zone the owner chose to keep their own clock in. */
  const DECLINED_KEY = 'nebo_tz_declined';

  let offer = $state<{ device: string; current: string } | null>(null);
  let busy = $state(false);

  /** The place a zone is named for: "America/Argentina/Buenos_Aires" → "Buenos Aires". */
  function place(zone: string): string {
    return (zone.split('/').pop() ?? zone).replaceAll('_', ' ');
  }

  /** The zone in force, from the profile an update answers with. */
  function zoneOf(profile: unknown): string {
    if (profile && typeof profile === 'object' && 'timezone' in profile) {
      const zone = (profile as { timezone?: unknown }).timezone;
      return typeof zone === 'string' ? zone : '';
    }
    return '';
  }

  onMount(async () => {
    const device = Intl.DateTimeFormat().resolvedOptions().timeZone ?? '';
    if (!device) return;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.userUpdateProfile({
        deviceTimezone: device,
        deviceLanguage: get(locale) ?? navigator.language
      });
      const current = zoneOf(resp.profile);
      if (current && current !== device && storage.get(DECLINED_KEY) !== device) {
        offer = { device, current };
      }
    } catch {
      // Reporting is best-effort: the next load reports again.
    }
  });

  async function switchZone() {
    if (!offer) return;
    busy = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.userUpdateProfile({ timezone: offer.device });
      offer = null;
    } catch {
      // Left on screen: the owner can try again.
    }
    busy = false;
  }

  function keep() {
    if (!offer) return;
    storage.set(DECLINED_KEY, offer.device);
    offer = null;
  }
</script>

{#if offer}
  <div
    class="fixed bottom-4 inset-x-4 z-[80] mx-auto max-w-lg flex flex-wrap items-center gap-2 rounded-lg border border-base-300 bg-base-100 shadow-lg px-3 py-2.5"
    role="status"
  >
    <span class="flex-1 min-w-0 text-sm">{$t('timezoneTravel.question', { values: { city: place(offer.device) } })}</span>
    <button type="button" class="btn btn-sm btn-primary" onclick={switchZone} disabled={busy}>{$t('timezoneTravel.switch')}</button>
    <button type="button" class="btn btn-sm btn-ghost" onclick={keep} disabled={busy}>
      {$t('timezoneTravel.keep', { values: { city: place(offer.current) } })}
    </button>
  </div>
{/if}
