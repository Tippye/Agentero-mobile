export type MobileSettingsSection =
	| "appearance"
	| "reading"
	| "agent"
	| "translate"
	| "easyscholar"
	| "layout"
	| "sync"
	| "background"
	| "about";

const OPEN_EVENT = "agentero:mobile-settings";

const SECTIONS: readonly MobileSettingsSection[] = [
	"appearance",
	"reading",
	"agent",
	"translate",
	"easyscholar",
	"layout",
	"sync",
	"background",
	"about",
];

function isSection(value: unknown): value is MobileSettingsSection {
	return (
		typeof value === "string" &&
		SECTIONS.includes(value as MobileSettingsSection)
	);
}

/** Open the in-shell mobile settings page without creating a desktop window. */
export function openMobileSettings(
	section: MobileSettingsSection = "appearance",
): void {
	if (typeof window === "undefined") return;
	window.dispatchEvent(new CustomEvent(OPEN_EVENT, { detail: section }));
}

export function listenMobileSettings(
	onOpen: (section: MobileSettingsSection) => void,
): () => void {
	if (typeof window === "undefined") return () => {};
	const listener = (event: Event) => {
		const section = (event as CustomEvent<unknown>).detail;
		if (isSection(section)) onOpen(section);
	};
	window.addEventListener(OPEN_EVENT, listener);
	return () => window.removeEventListener(OPEN_EVENT, listener);
}
