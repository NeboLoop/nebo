/**
 * Intelligence: the two choices a conversation (or an employee) makes about
 * the AI it runs on, read from and written to the same model string every
 * assignment already carries.
 *
 * - The LEVEL: always the same five, Instant · Low · Medium · High · Max
 *   (`types::packs::Effort`). A reference with no level (the pack's Auto, or
 *   its default) reads as Medium.
 * - The PACK: which models those five levels run on. Nebo AI is built in;
 *   the owner can add his own (Settings → Intelligence Packs).
 *
 * Never a model, provider or gateway id on screen: a pack reads as its name
 * (an internal one as "Custom pack"), a level as its word, and a raw model
 * that maps to neither as "Custom".
 */
import * as api from '$lib/api/nebo';

/** The Effort ladder, lowest first (`types::packs::Effort`). */
export const EFFORTS = ['instant', 'low', 'medium', 'high', 'max'] as const;
export type Effort = (typeof EFFORTS)[number];

/** The level a reference without one reads as: the middle of the ladder. */
export const DEFAULT_LEVEL: Effort = 'medium';

/** The built-in pack's id. */
export const NEBO_AI_ID = 'nebo-ai';

/** A pack as the picker shows it. */
export type PackRow = { id: string; name: string; builtIn: boolean };

/** The packs and the bot's default model string. */
export type Intelligence = { packs: PackRow[]; botDefault: string };

/**
 * What a model string runs on: a pack at a level (null: the pack's Auto or
 * default, shown as Medium), or a raw model no pack names.
 */
export type Choice = { pack: string; level: Effort | null } | { custom: true };

export function isCustom(c: Choice): c is { custom: true } {
	return 'custom' in c;
}

/** The legacy Nebo AI speeds, as levels (display only: nothing is rewritten). */
const LEGACY: Record<string, Effort | null> = {
	'nebo-1': null,
	'nebo-1-flash': 'instant',
	'nebo-1-medium': 'medium',
	'nebo-1-pro': 'high'
};

/** Pack names that are internal ids, never shown to the owner. */
export function isInternalName(name: string): boolean {
	const n = name.trim().toLowerCase();
	return n.startsWith('janus') || ['nebo-ai', 'neboai', 'nebo-1'].includes(n);
}

/** What one model string runs on. '' is the built-in pack's Auto. */
export function resolve(value: string, packs: PackRow[]): Choice {
	const v = value.trim();
	if (!v) return { pack: NEBO_AI_ID, level: null };
	if (v.startsWith('pack/')) {
		const [id, level] = v.slice(5).split('/');
		const known = packs.some((p) => p.id === id);
		const effort = (EFFORTS as readonly string[]).includes(level ?? '') ? (level as Effort) : null;
		if (!id || (level && level !== 'auto' && !effort)) return { custom: true };
		// A deleted pack runs on Nebo AI at the same level (the bot says so).
		return { pack: known ? id : NEBO_AI_ID, level: effort };
	}
	const slash = v.indexOf('/');
	const provider = slash < 0 ? '' : v.slice(0, slash);
	const model = slash < 0 ? v : v.slice(slash + 1);
	if ((provider === '' || provider === 'janus') && model in LEGACY) {
		return { pack: NEBO_AI_ID, level: LEGACY[model] };
	}
	return { custom: true };
}

/** Whether two choices run the same pack at the same level. */
export function sameChoice(a: Choice, b: Choice): boolean {
	if (isCustom(a) || isCustom(b)) return false;
	return a.pack === b.pack && a.level === b.level;
}

/**
 * The model string to write for a pick. `inherited` is what '' means where it
 * is written (a chat: the employee's choice, else the bot's default; an
 * employee: the bot's default): a pick equal to it clears the override.
 */
export function valueFor(pack: string, level: Effort | null, inherited: string, packs: PackRow[]): string {
	if (sameChoice(resolve(inherited, packs), { pack, level })) return '';
	return level ? `pack/${pack}/${level}` : `pack/${pack}`;
}

/** The level a choice shows as checked: no level reads as Medium. */
export function shownLevel(c: Choice): Effort | null {
	return isCustom(c) ? null : (c.level ?? DEFAULT_LEVEL);
}

/** The packs and the bot's default; null when they can't be read (the chip hides). */
export async function loadIntelligence(): Promise<Intelligence | null> {
	try {
		const res = await api.listPacks();
		const packs = (res.packs ?? []).map((p) => ({ id: p.id, name: p.name, builtIn: !!p.builtIn }));
		if (!packs.some((p) => p.id === NEBO_AI_ID)) packs.unshift({ id: NEBO_AI_ID, name: 'Nebo AI', builtIn: true });
		return { packs, botDefault: res.default ?? '' };
	} catch {
		return null;
	}
}


/** A translator, as `$t` is. */
type Translate = (key: string) => string;

/** A pack's name on screen: Nebo AI's own, an internal one as "Custom pack". */
export function packName(p: PackRow, t: Translate): string {
	if (p.builtIn) return t('modelPick.neboAi');
	return isInternalName(p.name) || !p.name.trim() ? t('modelPick.customPack') : p.name;
}

/**
 * The composer chip: the level ("Medium"), after the pack's name only when it
 * is not Nebo AI ("Work keys · High"); "Custom" for a raw model.
 */
export function choiceLabel(c: Choice, packs: PackRow[], t: Translate): string {
	if (isCustom(c)) return t('modelPick.custom');
	const level = t(`settingsPacks.level.${shownLevel(c)}`);
	const pack = packs.find((p) => p.id === c.pack);
	return pack && !pack.builtIn ? `${packName(pack, t)} · ${level}` : level;
}

/**
 * Any stored model string as words: a pack or a Nebo AI speed as the chip
 * names it, any other model by its own id without the provider or connection
 * in front ("anthropic@x/claude-…" → "claude-…").
 */
export function modelName(value: string, packs: PackRow[], t: Translate): string {
	const c = resolve(value, packs);
	if (!isCustom(c)) return choiceLabel(c, packs, t);
	return value.split('/').pop() || value;
}

/** How long a pick's checkmark shows before the picker closes. */
export const CLOSE_AFTER_MS = 150;

/**
 * A pick in a picker that closes on selection: the row already in use (the
 * checked one) closes without a write; any other is picked at once, so its
 * checkmark shows, and the picker closes a moment later.
 */
export function selectThenClose(
	current: Choice,
	pack: string,
	level: Effort | null,
	onpick: (pack: string, level: Effort | null) => void,
	onclose: () => void,
	delay = CLOSE_AFTER_MS
): void {
	const inUse =
		!isCustom(current) && current.pack === pack && (level === current.level || (level !== null && shownLevel(current) === level));
	if (inUse) return onclose();
	onpick(pack, level);
	setTimeout(onclose, delay);
}
