/**
 * The owner's OK for what an app from the marketplace asks to do beyond its
 * own page: see his files, talk to another website, ask another of his
 * employees, use one of his plugins (the server's `app_grants`). Asked when he opens the app
 * (`launchApp`), in plain words; changed later in the app's settings.
 */

import { writable } from "svelte/store";
import * as api from "$lib/api/nebo";
import type { AppPermission } from "$lib/api/nebo";

/** What the open consent sheet shows, and how it tells the opener it closed. */
export interface ConsentAsk {
  agentId: string;
  name: string;
  permissions: AppPermission[];
  done: () => void;
}

/** The ask the sheet (`AppConsentModal`) shows; null when none. */
export const consentAsk = writable<ConsentAsk | null>(null);

/**
 * Ask the owner about what `agentId` asks for and he has not answered yet.
 * Resolves when he has answered (or put it off): the app opens either way,
 * holding only what he allowed. An app his own employees built, or one
 * that asks for nothing, opens at once.
 */
export async function askAppPermissions(
  agentId: string,
  name: string,
): Promise<void> {
  let asking: AppPermission[] = [];
  try {
    const page = await api.getAppPermissions(agentId);
    asking = page.own
      ? []
      : page.permissions.filter((p) => p.state === "asking");
  } catch {
    return;
  }
  if (!asking.length) return;
  await new Promise<void>((done) =>
    consentAsk.set({ agentId, name, permissions: asking, done }),
  );
}

/** The i18n key and values that say one request in plain words. */
export function permissionWords(
  p: AppPermission,
  name: string,
  asking: boolean,
): { key: string; values: Record<string, string> } {
  const lead = asking ? "appPermissions.wants" : "appPermissions.can";
  const kind =
    p.kind === "files"
      ? "Files"
      : p.kind === "web"
        ? "Web"
        : p.kind === "plugin"
          ? "Plugin"
          : "Helper";
  // A plugin request always names its plugin.
  const any = p.target === "" && kind !== "Plugin" ? "Any" : "";
  return { key: `${lead}${kind}${any}`, values: { name, target: p.target } };
}
