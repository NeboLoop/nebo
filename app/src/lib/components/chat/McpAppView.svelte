<!--
  McpAppView — a tool result shown as its server's interactive view (MCP
  Apps), inline in the tool's message. The view runs in the sandbox proxy
  on Nebo's view origin (never Nebo's own), under the policy the engine
  builds from what the view declared; the bridge ($lib/mcpApps/view)
  answers it. Camera, microphone and location only after the owner allows
  them on the decision card. A view whose server is gone (or a client not
  on this computer) shows the "no longer available" line and the text
  result.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import { t, locale } from 'svelte-i18n';
  import DecisionCard from '$lib/components/DecisionCard.svelte';
  import { emphasized } from '$lib/chat/decision';
  import { theme } from '$lib/stores/theme';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { serverOf } from '$lib/mcpApps/servers';
  import { engineRequest } from '$lib/mcpApps/engine';
  import {
    ViewBridge,
    allowAttribute,
    askedFeatures,
    grantedPermissions,
    sandboxUrl,
    type HostContext,
    type McpAppPayload
  } from '$lib/mcpApps/view';

  let {
    app,
    chatId,
    toolId,
    text = ''
  }: {
    app: McpAppPayload;
    chatId: string;
    /** The call id the result is stored under. */
    toolId: string;
    /** The call's text result, shown when the view can't be. */
    text?: string;
  } = $props();

  const MAX_HEIGHT = 720;
  const viewOrigin = getWebSocketClient().getViewOrigin();
  const asked = $derived(askedFeatures(app.permissions));

  let server = $state<{ name: string; connected: boolean } | null | undefined>(undefined);
  $effect(() => {
    serverOf(app.serverId).then((s) => (server = s));
  });

  // The owner's answer on the decision card; undecided (null) while the
  // view asks for a feature he hasn't answered.
  let answer = $state<boolean | null>(null);
  const allowed = $derived(asked.length ? answer : false);
  let frame = $state<HTMLIFrameElement | null>(null);
  let height = $state(160);
  let width = $state(0);
  let sent = false;

  const available = $derived(!!viewOrigin && server?.connected === true);
  const granted = $derived(grantedPermissions(app.permissions, allowed === true));
  const allowAttr = $derived(allowAttribute(granted));
  const src = $derived(viewOrigin ? sandboxUrl(viewOrigin, app, window.location.origin) : '');

  function context(): HostContext {
    return {
      toolInfo: { tool: { name: app.tool } },
      theme: $theme === 'dark' ? 'dark' : 'light',
      locale: $locale ?? 'en',
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      platform: 'desktop',
      displayMode: 'inline',
      availableDisplayModes: ['inline'],
      containerDimensions: { width, maxHeight: MAX_HEIGHT }
    };
  }

  const bridge = new ViewBridge(
    {
      post: (msg) => frame?.contentWindow?.postMessage(msg, viewOrigin),
      engine: (method, params) => engineRequest({ chatId, messageId: toolId }, method, params),
      openLink: (url) => window.open(url, '_blank', 'noopener,noreferrer'),
      resize: ({ height: h }) => {
        if (typeof h === 'number' && h > 0) height = Math.min(Math.ceil(h), MAX_HEIGHT);
      },
      context,
      hostVersion: '1'
    },
    {
      get toolInput() {
        return app.toolInput ?? {};
      },
      get toolResult() {
        return app.toolResult ?? {};
      }
    }
  );

  function onMessage(event: MessageEvent) {
    if (!frame || event.source !== frame.contentWindow || event.origin !== viewOrigin) return;
    const msg = event.data;
    if (msg?.method === 'ui/notifications/sandbox-proxy-ready') {
      // Once: a proxy that loads again (the view navigated it) gets nothing.
      if (sent) return;
      sent = true;
      frame.contentWindow?.postMessage(
        {
          jsonrpc: '2.0',
          method: 'ui/notifications/sandbox-resource-ready',
          params: { html: app.html, csp: app.csp, permissions: granted }
        },
        viewOrigin
      );
      return;
    }
    void bridge.receive(msg);
  }

  $effect(() => {
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  });

  // Theme and language follow Nebo's while the view is open.
  $effect(() => {
    const ctx = { theme: ($theme === 'dark' ? 'dark' : 'light') as 'light' | 'dark', locale: $locale ?? 'en' };
    bridge.contextChanged(ctx);
  });

  onDestroy(() => {
    void bridge.teardown('closed');
  });

  function features(): string {
    const names = asked.map((f) => $t(`mcpApp.feature.${f}`));
    return new Intl.ListFormat($locale ?? 'en', { type: 'conjunction' }).format(names);
  }
</script>

{#if server !== undefined}
  {#if available}
    <div class="mcp-app" data-testid="mcp-app">
      <p class="mcp-app-label">{$t('mcpApp.label', { values: { server: server?.name ?? '' } })}</p>
      {#if allowed === null}
        <DecisionCard
          line={emphasized($t, 'mcpApp.wantsFeatures', { server: server?.name ?? '', features: features() })}
          reason={$t('mcpApp.featuresReason')}
          choices={[
            { id: 'allow', label: $t('decision.allowOnce'), tone: 'filled' },
            { id: 'decline', label: $t('decision.decline'), tone: 'outlined' }
          ]}
          onchoose={(id) => (answer = id === 'allow')}
        />
      {:else}
        <div class="mcp-app-frame" class:mcp-app-bordered={app.prefersBorder !== false} bind:clientWidth={width}>
          <iframe
            bind:this={frame}
            title={$t('mcpApp.label', { values: { server: server?.name ?? '' } })}
            {src}
            sandbox="allow-scripts allow-same-origin allow-forms"
            allow={allowAttr}
            {height}
          ></iframe>
        </div>
      {/if}
    </div>
  {:else}
    <div class="mcp-app-gone" data-testid="mcp-app-gone">
      <p class="mcp-app-gone-line">{$t('mcpApp.unavailable')}</p>
      {#if text}<p class="mcp-app-gone-text" dir="auto">{text}</p>{/if}
    </div>
  {/if}
{/if}
