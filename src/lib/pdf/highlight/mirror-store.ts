/**
 * Live highlight mirror between the original PDF pane and its translated
 * companion. The original pane owns the real annotation plugin/store; the
 * translation pane has no annotation plugin, so it renders the same highlight
 * models through the translated-text overlay instead of re-importing them into
 * a second PDF document.
 *
 * This is display state only: it contains no annotation mutation path and is
 * safe to drop whenever either pane has no paper key.
 */
import { useSyncExternalStore } from "react";
import type { PdfHighlight } from "./types";

type Entry = {
	highlights: readonly PdfHighlight[];
	listeners: Set<() => void>;
};

const entries = new Map<string, Entry>();
const EMPTY: readonly PdfHighlight[] = [];

function entry(key: string): Entry {
	let value = entries.get(key);
	if (!value) {
		value = { highlights: EMPTY, listeners: new Set() };
		entries.set(key, value);
	}
	return value;
}

export function publishHighlightMirror(
	key: string | null,
	highlights: readonly PdfHighlight[],
): void {
	if (!key) return;
	const value = entry(key);
	if (value.highlights === highlights) return;
	value.highlights = highlights;
	for (const listener of value.listeners) listener();
}

export function useHighlightMirror(
	key: string | null,
): readonly PdfHighlight[] {
	return useSyncExternalStore(
		(listener) => {
			if (!key) return () => undefined;
			const value = entry(key);
			value.listeners.add(listener);
			return () => value.listeners.delete(listener);
		},
		() => (key ? entry(key).highlights : EMPTY),
		() => EMPTY,
	);
}
