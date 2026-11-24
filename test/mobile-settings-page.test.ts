// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MobileSettingsSection } from "@/lib/mobile/settings";
import type { AppSettings } from "@/lib/settings/types";

type Captured = Record<string, unknown>;

const io = vi.hoisted(() => ({
	save: vi.fn(),
	error: vi.fn(),
	version: vi.fn(),
	openExternal: vi.fn(),
	snapshot: null as AppSettings | null,
	appearance: null as Captured | null,
	agent: null as Captured | null,
	translate: null as Captured | null,
	layout: null as Captured | null,
	sync: null as Captured | null,
	download: null as Captured | null,
	background: null as Captured | null,
	t: (key: string) => key,
}));

vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: io.t }) }));
vi.mock("@/lib/settings", () => ({
	loadSettings: () => io.snapshot,
	saveSettingsAsync: io.save,
	subscribeSettings: () => () => {},
}));
vi.mock("@/lib/core/notify", () => ({
	notifyError: io.error,
	errorMessage: (error: unknown) => String(error),
}));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: io.version }));
vi.mock("@/lib/core/open-external", () => ({
	openExternalUrl: io.openExternal,
}));
vi.mock("@/components/settings/panes/appearance-pane", () => ({
	AppearancePane: (props: Captured) => {
		io.appearance = props;
		return null;
	},
}));
vi.mock("@/components/settings/panes/translate-pane", () => ({
	TranslatePane: (props: Captured) => {
		io.translate = props;
		return null;
	},
}));
vi.mock("@/components/settings/panes/layout-pane", () => ({
	LayoutPane: (props: Captured) => {
		io.layout = props;
		return null;
	},
}));
vi.mock("@/components/settings/panes/sync-pane", () => ({
	SyncPane: (props: Captured) => {
		io.sync = props;
		return null;
	},
}));
vi.mock("@/components/mobile/mobile-agent-pane", () => ({
	MobileAgentPane: (props: Captured) => {
		io.agent = props;
		return null;
	},
}));
vi.mock("@/components/mobile/mobile-download-settings", () => ({
	MobileDownloadSettings: (props: Captured) => {
		io.download = props;
		return null;
	},
}));
vi.mock("@/components/mobile/mobile-background-settings", () => ({
	MobileBackgroundSettingsPanel: (props: Captured) => {
		io.background = props;
		return null;
	},
}));

import { MobileSettingsPage } from "@/components/mobile/mobile-settings-page";
import { TooltipProvider } from "@/components/ui/tooltip";
import { maskSecret } from "@/lib/core/secret-mask";
import {
	listenMobileSettings,
	openMobileSettings,
} from "@/lib/mobile/settings";
import { DEFAULT_SETTINGS } from "@/lib/settings/defaults";

const NAV_LABELS: [string, MobileSettingsSection][] = [
	["settings:nav.appearance", "appearance"],
	["settings:nav.reading", "reading"],
	["settings:nav.agent", "agent"],
	["settings:nav.translate", "translate"],
	["settings:general.easyScholar.section", "easyscholar"],
	["settings:nav.layout", "layout"],
	["settings:nav.sync", "sync"],
	["mobile:background.title", "background"],
	["settings:nav.about", "about"],
];

let root: ReturnType<typeof createRoot>;
let container: HTMLDivElement;
let section: MobileSettingsSection;
let onSectionChange: ReturnType<typeof vi.fn>;
let onClose: ReturnType<typeof vi.fn>;
const onSynced = vi.fn();

function button(label: string): HTMLButtonElement {
	const found = [
		...document.querySelectorAll<HTMLButtonElement>("button"),
	].find(
		(element) =>
			element.getAttribute("aria-label") === label ||
			element.textContent === label,
	);
	if (!found) throw new Error(`Missing button: ${label}`);
	return found;
}

async function renderPage(open = true) {
	await act(async () => {
		root.render(
			// The app root supplies the provider (main.tsx); the page itself
			// renders tooltip-bearing settings blocks inside it.
			createElement(
				TooltipProvider,
				null,
				createElement(MobileSettingsPage, {
					open,
					section,
					onSectionChange,
					onClose,
					vaultPath: "/vault",
					onSynced,
				}),
			),
		);
	});
	await act(async () => {});
}

async function click(label: string) {
	await act(async () => button(label).click());
	await act(async () => {});
}

async function edit(element: HTMLInputElement, value: string) {
	await act(async () => {
		Object.getOwnPropertyDescriptor(
			HTMLInputElement.prototype,
			"value",
		)?.set?.call(element, value);
		element.dispatchEvent(new Event("input", { bubbles: true }));
	});
}

async function navigate(label: string, next: MobileSettingsSection) {
	await click(label);
	expect(onSectionChange).toHaveBeenCalledWith(next);
	expect(section).toBe(next);
	await renderPage();
}

beforeEach(() => {
	Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
	vi.clearAllMocks();
	io.snapshot = structuredClone(DEFAULT_SETTINGS);
	// The Host stores the plaintext secret but answers with its redaction.
	io.save.mockImplementation(async (next: AppSettings) => {
		io.snapshot = { ...next, easyScholarKey: maskSecret(next.easyScholarKey) };
		return io.snapshot;
	});
	io.version.mockResolvedValue("9.9.9");
	section = "appearance";
	onSectionChange = vi.fn((next: MobileSettingsSection) => {
		section = next;
	});
	onClose = vi.fn();
	container = document.createElement("div");
	document.body.append(container);
	root = createRoot(container);
});

afterEach(async () => {
	await act(async () => root.unmount());
	container.remove();
});

describe("mobile settings event bridge", () => {
	it("opens the requested section and ignores unknown details", () => {
		const seen: MobileSettingsSection[] = [];
		const unbind = listenMobileSettings((next) => seen.push(next));
		openMobileSettings("layout");
		expect(seen).toEqual(["layout"]);
		openMobileSettings();
		expect(seen).toEqual(["layout", "appearance"]);
		for (const detail of ["account", "sync ", 42, { section: "about" }]) {
			window.dispatchEvent(
				new CustomEvent("agentero:mobile-settings", { detail }),
			);
		}
		expect(seen).toEqual(["layout", "appearance"]);
		unbind();
		openMobileSettings("sync");
		expect(seen).toEqual(["layout", "appearance"]);
	});
});

describe("mobile settings page", () => {
	it("renders nothing while closed", async () => {
		await renderPage(false);
		expect(container.childElementCount).toBe(0);
	});

	it("renders the title, back action and one nav entry per section", async () => {
		await renderPage();
		const host = container.querySelector("section");
		expect(host?.getAttribute("aria-label")).toBe("settings:title");
		expect(container.textContent).toContain("settings:title");
		expect(
			container.querySelector("header")?.className.includes("min-h-16"),
		).toBe(true);
		const navButtons = [...container.querySelectorAll("nav button")];
		expect(navButtons).toHaveLength(NAV_LABELS.length);
		expect(navButtons.map((item) => item.textContent)).toEqual(
			NAV_LABELS.map(([label]) => label),
		);
		expect(
			container.querySelector('nav button[aria-current="page"]')?.textContent,
		).toBe("settings:nav.appearance");
		expect(io.appearance).not.toBeNull();
		expect(typeof io.appearance?.patch).toBe("function");
		await click("common:close");
		expect(onClose).toHaveBeenCalledOnce();
	});

	it("switches panes through onSectionChange and forwards mobile-only props", async () => {
		await renderPage();
		await navigate("settings:nav.reading", "reading");
		await navigate("settings:nav.agent", "agent");
		expect(io.agent?.settings).toBe(io.snapshot);
		expect(typeof io.agent?.patch).toBe("function");
		await navigate("settings:nav.translate", "translate");
		expect(io.translate?.apiOnly).toBe(true);
		expect(io.translate?.settings).toBe(io.snapshot);
		await navigate("settings:general.easyScholar.section", "easyscholar");
		expect(container.textContent).toContain("general.easyScholar.section");
		expect(
			container.querySelector<HTMLInputElement>("#easy-scholar-key")?.value,
		).toBe(io.snapshot?.easyScholarKey);
		await navigate("settings:nav.layout", "layout");
		expect(io.layout?.apiOnly).toBe(true);
		expect(io.layout?.hostContext).toEqual({ kind: "local" });
		await navigate("settings:nav.sync", "sync");
		expect(io.sync).toMatchObject({ vaultPath: "/vault", mobile: true });
		expect(io.sync?.onSynced).toBe(onSynced);
		expect(io.download).toMatchObject({ vaultPath: "/vault" });
		await navigate("mobile:background.title", "background");
		expect(io.background?.active).toBe(true);
		expect(io.background?.onStateChange).toBeUndefined();
	});

	it("keeps the saved EasyScholar secret masked and preserves unrelated settings", async () => {
		io.snapshot = {
			...structuredClone(DEFAULT_SETTINGS),
			easyScholarKey: "******",
			locale: "en",
			batchImportConcurrency: 2,
		};
		const before = structuredClone(io.snapshot);
		section = "easyscholar";
		await renderPage();
		const input = container.querySelector("#easy-scholar-key");
		if (!(input instanceof HTMLInputElement))
			throw new Error("Missing EasyScholar key input");
		expect(input.type).toBe("password");
		expect(input.value).toBe("******");
		await edit(input, "user-supplied-key");
		await click("general.easyScholar.confirm");
		expect(io.save).toHaveBeenCalledExactlyOnceWith({
			...before,
			easyScholarKey: "user-supplied-key",
		});
		// The committed key comes back redacted from the Host, never as plaintext.
		expect(input.value).toBe(maskSecret("user-supplied-key"));
	});

	it("opens the EasyScholar console through the guarded opener", async () => {
		section = "easyscholar";
		await renderPage();
		await click("general.easyScholar.openKeyPage");
		expect(io.openExternal).toHaveBeenCalledExactlyOnceWith(
			"https://www.easyscholar.cc/console/user/open",
		);
	});

	it("shows a compact about page with the app version and GitHub link", async () => {
		await renderPage();
		await navigate("settings:nav.about", "about");
		expect(io.version).toHaveBeenCalled();
		expect(container.textContent).toContain("settings:about.title");
		expect(container.textContent).toContain("Agentero");
		expect(container.textContent).toContain("v9.9.9");
		expect(container.textContent).toContain("settings:about.starGithub");
		const icon = container.querySelector("img");
		expect(icon?.getAttribute("src")).toBeTruthy();
		await click("settings:about.starGithub");
		expect(io.openExternal).toHaveBeenCalledWith(
			"https://github.com/poco-ai/Agentero",
		);
		expect(container.textContent).not.toContain("settings:about.cli");
	});

	it("persists merged settings and reports save failures", async () => {
		await renderPage();
		const appearance = io.appearance as {
			theme: string;
			patch: (partial: Partial<AppSettings>) => void;
		};
		expect(appearance.theme).toBe(io.snapshot?.theme);
		const before = structuredClone(io.snapshot as AppSettings);
		await act(async () => {
			appearance.patch({ theme: "dark" });
		});
		await act(async () => {});
		expect(io.save).toHaveBeenCalledTimes(1);
		const saved = io.save.mock.calls[0]?.[0] as AppSettings;
		expect(saved.theme).toBe("dark");
		expect(saved.locale).toBe(before.locale);
		expect(saved.translate).toEqual(before.translate);
		expect(io.error).not.toHaveBeenCalled();
		io.save.mockRejectedValueOnce(new Error("settings_set failed"));
		await act(async () => {
			appearance.patch({ theme: "light" });
		});
		await act(async () => {});
		expect(io.error).toHaveBeenCalledWith("Error: settings_set failed");
	});
});
