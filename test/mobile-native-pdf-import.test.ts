// @vitest-environment happy-dom
import { act, type ComponentType, createElement as h, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { PaperMetadata } from "@/lib/paper/types";

const io = vi.hoisted(() => ({
	tauri: true,
	vault: "/private/Vault-A",
	refresh: vi.fn(async () => {}),
	error: vi.fn(),
	native: vi.fn(),
	local: vi.fn(),
	mkdir: vi.fn(async () => {}),
	write: vi.fn(async () => {}),
	remove: vi.fn(async () => {}),
	readerMounts: 0,
	backEnabled: false,
	settingsPage: null as ComponentType<Record<string, unknown>> | null,
	settings: null as Record<string, unknown> | null,
	runtimeMounts: 0,
	runtimeUnmounts: 0,
	paper: {
		id: "existing",
		path: "papers/existing",
		title: "Existing paper",
		authors: [],
		tags: [],
	} as PaperMetadata,
}));
vi.mock("@tauri-apps/api/core", async (original) => ({
	...(await original<object>()),
	isTauri: () => io.tauri,
}));
// Native transport and filesystem are test boundaries, including for the retained preview importer.
vi.mock("@/lib/core/tauri", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/event", async (original) => ({
	...(await original<typeof import("@tauri-apps/api/event")>()),
	listen: vi.fn(async () => () => {}),
}));
vi.mock("@/lib/core/bindings", async (original) => {
	const actual = await original<typeof import("@/lib/core/bindings")>();
	return {
		...actual,
		commands: {
			...actual.commands,
			paperImportNativePdfs: (...args: unknown[]) => io.native(...args),
			paperImportOpenedPdf: async () => ({
				status: "ok",
				data: { ok: true, data: null },
			}),
			paperImportLocalPdf: (...args: unknown[]) => io.local(...args),
		},
	};
});
vi.mock("@/lib/vault/fs", async (original) => ({
	...(await original<typeof import("@/lib/vault/fs")>()),
	createVaultDirectory: (...args: unknown[]) => io.mkdir(...args),
	writeVaultBytes: (...args: unknown[]) => io.write(...args),
	removeVaultPath: (...args: unknown[]) => io.remove(...args),
}));
vi.mock("@/lib/core/notify", () => ({
	notifyError: (...args: unknown[]) => io.error(...args),
	errorMessage: (error: unknown) => String(error),
}));
// The real settings page asks for the app version when it opens.
vi.mock("@tauri-apps/api/app", () => ({
	getVersion: async () => "9.9.9",
}));
vi.mock("react-i18next", () => ({
	initReactI18next: { type: "3rdParty", init() {} },
	useTranslation: () => ({ t: (key: string) => key }),
}));
// The real settings page is enabled per-test (App-level integration below);
// everywhere else it is a null boundary so import flows stay isolated.
vi.mock("@/components/mobile/mobile-settings-page", () => ({
	MobileSettingsPage: (props: Record<string, unknown>) => {
		io.settings = props;
		return io.settingsPage ? h(io.settingsPage, props) : null;
	},
}));
vi.mock("@/lib/mobile/pdf-processing", () => ({
	// Observe where MobileApp owns the runtime. Runtime behavior is tested separately.
	useMobilePdfProcessingRuntime: () => {
		useEffect(() => {
			io.runtimeMounts++;
			return () => {
				io.runtimeUnmounts++;
			};
		}, []);
	},
}));
vi.mock("@/components/mobile/hooks/use-mobile-vault", () => ({
	useMobileVault: () => ({ vaultPath: io.vault, loading: false, retry() {} }),
}));
vi.mock("@/components/mobile/hooks/use-mobile-papers", () => ({
	useMobilePapers: () => ({
		papers: [io.paper],
		loading: false,
		refresh: io.refresh,
	}),
}));
vi.mock("@/components/mobile/hooks/use-bridge-status", () => ({
	useBridgeStatus: () => ({
		status: { paired: false, connected: false },
		pairPending: null,
		setStatus() {},
		disconnect() {},
		resume() {},
		resetAvailable: false,
	}),
}));
vi.mock("@/components/mobile/hooks/use-mobile-agents", () => ({
	useMobileAgents: () => ({
		agents: [],
		loading: false,
		selectedAgent: null,
		selectedAgentId: null,
		selectAgent() {},
	}),
}));
vi.mock("@/components/mobile/hooks/use-mobile-paper-sync", () => ({
	useMobilePaperSync: () => ({
		states: {},
		setPinned() {},
		fetchMissing: async () => false,
		progressText: null,
	}),
}));
vi.mock("@/components/mobile/hooks/use-pair-offer-links", () => ({
	usePairOfferLinks() {},
}));
vi.mock("@/components/mobile/hooks/use-mobile-route", () => ({
	useMobileRoute: () => ({ markInteraction() {} }),
}));
vi.mock("@/components/mobile/hooks/use-mobile-system-back", () => ({
	useMobileSystemBack: ({ enabled }: { enabled: boolean }) => {
		io.backEnabled = enabled;
		return true;
	},
}));
vi.mock("@/components/mobile/hooks/use-mobile-workspace-width", () => ({
	useMobileWorkspaceWidth: () => ({
		ref: { current: null },
		widthClass: "medium",
	}),
}));
// Unrelated persistent document/Agent renderers are heavy boundaries; import buttons,
// library rows, navigation, pane composition and MobileApp itself remain real.
vi.mock("@/components/mobile/mobile-reader-page", () => ({
	MobileReaderPage: () => {
		useEffect(() => {
			io.readerMounts++;
		}, []);
		return h(
			"article",
			{ "data-reader-document": true },
			h("textarea", { "aria-label": "retained-notes" }),
		);
	},
}));
vi.mock("@/components/agent/agent-panel", () => ({
	AgentPanel: () => h("textarea", { "aria-label": "retained-agent" }),
}));
vi.mock("@/components/mobile/mobile-agent-page", () => ({
	MobileAgentPage: () => null,
}));
vi.mock("@/components/mobile/mobile-paper-import", () => ({
	MobilePaperImport: () => null,
}));
vi.mock("@/components/mobile/mobile-paper-info", () => ({
	MobilePaperInfo: () => null,
}));
vi.mock("@/components/mobile/mobile-plaza-page", () => ({
	MobilePlazaPage: () => null,
}));
vi.mock("@/components/mobile/mobile-pairing", () => ({
	MobilePairing: () => null,
}));
// The settings page renders the background pane on demand; keep it a boundary.
vi.mock("@/components/mobile/mobile-background-settings", () => ({
	MobileBackgroundSettingsPanel: () => null,
}));

import MobileApp from "@/components/mobile/mobile-app";
import { TooltipProvider } from "@/components/ui/tooltip";
import { pickAndImportMobilePdfs } from "@/lib/mobile/import-pdf";

const result = (papers: unknown[] = [], errors: string[] = []) => ({
	status: "ok",
	data: { ok: true, data: { papers, errors } },
});
function deferred<T>() {
	let resolve!: (value: T) => void, reject!: (error: unknown) => void;
	const promise = new Promise<T>((yes, no) => {
		resolve = yes;
		reject = no;
	});
	return { promise, resolve, reject };
}
let root: Root, host: HTMLDivElement, disposed: boolean;
async function render() {
	await act(async () => root.render(h(TooltipProvider, null, h(MobileApp))));
}
function importButton() {
	const button = host.querySelector<HTMLButtonElement>(
		'button[aria-label="library.importPdf"]',
	);
	if (!button) throw Error("Real import button missing");
	return button;
}
async function clickImport() {
	await act(async () => importButton().click());
}
async function unmount() {
	await act(async () => root.unmount());
	disposed = true;
}
beforeEach(() => {
	vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
	localStorage.clear();
	io.tauri = true;
	io.vault = "/private/Vault-A";
	io.readerMounts = 0;
	io.backEnabled = false;
	io.settingsPage = null;
	io.settings = null;
	io.runtimeMounts = 0;
	io.runtimeUnmounts = 0;
	io.refresh = vi.fn(async () => {});
	io.error.mockReset();
	io.native.mockReset().mockResolvedValue(result());
	io.local.mockReset().mockResolvedValue(result([io.paper]));
	io.mkdir.mockClear();
	io.write.mockClear();
	io.remove.mockClear();
	host = document.createElement("div");
	document.body.append(host);
	root = createRoot(host);
	disposed = false;
});
afterEach(async () => {
	if (!disposed) await unmount();
	host.remove();
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
});

it("opens the real settings page from the real Sidebar while keeping runtime ownership above reader navigation", {
	timeout: 20000,
}, async () => {
	const actual = await vi.importActual<
		typeof import("@/components/mobile/mobile-settings-page")
	>("@/components/mobile/mobile-settings-page");
	io.settingsPage = actual.MobileSettingsPage as ComponentType<
		Record<string, unknown>
	>;
	await render();
	const clickLabel = async (label: string) => {
		const button = [
			...document.querySelectorAll<HTMLButtonElement>("button"),
		].find(
			(item) =>
				item.getAttribute("aria-label") === label || item.textContent === label,
		);
		if (!button) throw Error(`Missing button ${label}`);
		await act(async () => button.click());
	};
	const settingsPage = () =>
		document.querySelector<HTMLElement>('section[aria-label="settings:title"]');
	expect(io.runtimeMounts).toBe(1);
	await clickLabel("settings.menu");
	await clickLabel("tabs.settings");
	expect(io.settings?.open).toBe(true);
	const page = settingsPage();
	if (!page) throw Error("Real settings page missing");
	expect(page.textContent).toContain("settings:title");
	const navButtons = [...page.querySelectorAll("nav button")];
	// Android adds a dedicated EasyScholar settings pane between translate and layout.
	expect(navButtons.map((item) => item.textContent)).toEqual([
		"settings:nav.appearance",
		"settings:nav.agent",
		"settings:nav.translate",
		"settings:general.easyScholar.section",
		"settings:nav.layout",
		"settings:nav.sync",
		"mobile:background.title",
		"settings:nav.about",
	]);
	expect(
		page.querySelector('nav button[aria-current="page"]')?.textContent,
	).toBe("settings:nav.appearance");
	expect(page.textContent).toContain("appearance.title");
	await clickLabel("settings:nav.translate");
	const translate = settingsPage();
	expect(translate?.textContent).toContain("translate.title");
	expect(translate?.textContent).not.toContain("appearance.title");
	expect(document.querySelector("#translate-cache-translation")).not.toBeNull();
	await clickLabel("settings:nav.layout");
	const layout = settingsPage();
	expect(layout?.textContent).toContain("layout.title");
	expect(
		document.querySelector("#layout-provider-paddle-api-key"),
	).not.toBeNull();
	expect(
		document.querySelector("#layout-provider-mineru-api-key"),
	).not.toBeNull();
	expect(
		document.querySelector("#layout-provider-openaiCompatible-api-key"),
	).toBeNull();
	await clickLabel("common:close");
	expect(settingsPage()).toBeNull();
	expect(io.settings?.open).toBe(false);
	expect(io.runtimeMounts).toBe(1);
	const paper = [...host.querySelectorAll("button")].find((button) =>
		button.textContent?.includes("Existing paper"),
	);
	if (!paper) throw Error("Real library row missing");
	await act(async () => paper.click());
	expect(io.readerMounts).toBe(1);
	await clickLabel("reader.back");
	expect(host.querySelector("[data-reader-document]")).toBeNull();
	expect(io.runtimeMounts).toBe(1);
	expect(io.runtimeUnmounts).toBe(0);
	await unmount();
	expect(io.runtimeUnmounts).toBe(1);
});

it("calls the native command with only the vault and maps catalog results without staging or WebView file reads", async () => {
	io.native.mockResolvedValueOnce(
		result([io.paper, { id: "another" }], ["one provider item failed"]),
	);
	expect(await pickAndImportMobilePdfs("/private/Vault-A")).toEqual({
		imported: 2,
		errors: ["one provider item failed"],
	});
	expect(io.native).toHaveBeenCalledExactlyOnceWith("/private/Vault-A");
	expect(io.mkdir).not.toHaveBeenCalled();
	expect(io.write).not.toHaveBeenCalled();
});

it("uses the real native import button once for two same-turn clicks, stays busy through import/refresh, and preserves clean-root Back", async () => {
	const command = deferred<ReturnType<typeof result>>(),
		refresh = deferred<void>();
	io.native.mockReturnValueOnce(command.promise);
	io.refresh.mockReturnValueOnce(refresh.promise);
	await render();
	const input = host.querySelector<HTMLInputElement>('input[type="file"]');
	if (!input) throw Error("Preview input missing");
	const browserPicker = vi.spyOn(input, "click").mockImplementation(() => {});
	await act(() => {
		importButton().click();
		importButton().click();
	});
	expect(io.native).toHaveBeenCalledExactlyOnceWith("/private/Vault-A");
	expect(browserPicker).not.toHaveBeenCalled();
	expect(importButton().disabled).toBe(true);
	expect(io.backEnabled).toBe(false);
	expect(
		document.querySelector('[role="dialog"][data-state="open"]'),
	).toBeNull();
	await act(async () => command.resolve(result([io.paper])));
	expect(io.refresh).toHaveBeenCalledOnce();
	expect(importButton().disabled).toBe(true);
	await act(async () => refresh.resolve());
	expect(importButton().disabled).toBe(false);
	expect(io.mkdir).not.toHaveBeenCalled();
	expect(io.write).not.toHaveBeenCalled();
	expect(io.error).not.toHaveBeenCalled();
});

it("treats native picker cancellation as harmless and admits a later user attempt", async () => {
	await render();
	await clickImport();
	expect(io.error).not.toHaveBeenCalled();
	expect(importButton().disabled).toBe(false);
	expect(io.backEnabled).toBe(false);
	await clickImport();
	expect(io.native).toHaveBeenCalledTimes(2);
	expect(io.error).not.toHaveBeenCalled();
});

it("reports partial errors once while preserving the currently opened document and unsent drafts", async () => {
	const command = deferred<ReturnType<typeof result>>();
	io.native.mockReturnValueOnce(command.promise);
	await render();
	await clickImport();
	const paper = [...host.querySelectorAll("button")].find((button) =>
		button.textContent?.includes("Existing paper"),
	);
	if (!paper) throw Error("Real library row missing");
	await act(async () => paper.click());
	const reader = host.querySelector("[data-reader-document]"),
		notes = host.querySelector<HTMLTextAreaElement>(
			'[aria-label="retained-notes"]',
		),
		agent = host.querySelector<HTMLTextAreaElement>(
			'[aria-label="retained-agent"]',
		);
	if (!notes || !agent) throw Error("Persistent document/Agent missing");
	notes.value = "Unsent notes";
	agent.value = "Unsent prompt";
	await act(async () =>
		command.resolve(
			result(
				[{ id: "new-imported-paper" }],
				["item A failed", "item B failed"],
			),
		),
	);
	expect(io.error).toHaveBeenCalledExactlyOnceWith(
		"item A failed\nitem B failed",
	);
	expect(io.refresh).toHaveBeenCalledOnce();
	expect(host.querySelector("[data-reader-document]")).toBe(reader);
	expect(io.readerMounts).toBe(1);
	expect(notes.value).toBe("Unsent notes");
	expect(agent.value).toBe("Unsent prompt");
	expect(io.vault).toBe("/private/Vault-A");
});

it("releases admission on a picker failure even when catalog refresh also rejects, without duplicate errors", async () => {
	io.native.mockRejectedValueOnce(Error("Picker failed"));
	io.refresh.mockRejectedValueOnce(Error("Refresh failed"));
	await render();
	await clickImport();
	expect(io.error).toHaveBeenCalledExactlyOnceWith("Picker failed");
	expect(importButton().disabled).toBe(false);
	await clickImport();
	expect(io.native).toHaveBeenCalledTimes(2);
	expect(importButton().disabled).toBe(false);
});

it("handles an independent refresh failure without leaving import busy or an unhandled rejection", async () => {
	io.refresh.mockRejectedValueOnce(Error("Catalog refresh failed"));
	await render();
	await clickImport();
	expect(io.error).toHaveBeenCalledExactlyOnceWith("Catalog refresh failed");
	expect(importButton().disabled).toBe(false);
	await clickImport();
	expect(io.native).toHaveBeenCalledTimes(2);
});

it.each([
	false,
	true,
])("drops old-vault completion and errors after a committed switch (switch back: %s)", async (switchBack) => {
	const command = deferred<ReturnType<typeof result>>();
	io.native.mockReturnValueOnce(command.promise);
	await render();
	await clickImport();
	const oldRefresh = io.refresh;
	io.vault = "/private/Vault-B";
	io.refresh = vi.fn(async () => {});
	await render();
	if (switchBack) {
		io.vault = "/private/Vault-A";
		await render();
	}
	await act(async () => command.resolve(result([], ["Stale vault error"])));
	expect(oldRefresh).not.toHaveBeenCalled();
	expect(io.refresh).not.toHaveBeenCalled();
	expect(io.error).not.toHaveBeenCalled();
	expect(importButton().disabled).toBe(false);
});

it("uses the latest committed refresh for the same vault rather than an obsolete callback", async () => {
	const command = deferred<ReturnType<typeof result>>();
	io.native.mockReturnValueOnce(command.promise);
	await render();
	await clickImport();
	const oldRefresh = io.refresh;
	io.refresh = vi.fn(async () => {});
	await render();
	await act(async () => command.resolve(result([io.paper])));
	expect(oldRefresh).not.toHaveBeenCalled();
	expect(io.refresh).toHaveBeenCalledOnce();
});

it("does not refresh or notify after unmount when a native command finishes late", async () => {
	const command = deferred<ReturnType<typeof result>>();
	io.native.mockReturnValueOnce(command.promise);
	await render();
	await clickImport();
	await unmount();
	await act(async () => command.reject(Error("Late native error")));
	expect(io.error).not.toHaveBeenCalled();
	expect(io.refresh).not.toHaveBeenCalled();
});

it("retains the browser input and existing file-arrayBuffer importer without invoking the native picker", async () => {
	io.tauri = false;
	await render();
	const input = host.querySelector<HTMLInputElement>('input[type="file"]');
	if (!input) throw Error("Preview input missing");
	const browserPicker = vi.spyOn(input, "click").mockImplementation(() => {});
	await clickImport();
	expect(browserPicker).toHaveBeenCalledOnce();
	expect(io.native).not.toHaveBeenCalled();
	const read = vi.fn(async () => new Uint8Array([37, 80, 68, 70]).buffer);
	Object.defineProperty(input, "files", {
		configurable: true,
		value: [{ name: "Browser file.pdf", arrayBuffer: read }],
	});
	await act(async () =>
		input.dispatchEvent(new Event("change", { bubbles: true })),
	);
	expect(read).toHaveBeenCalledOnce();
	expect(io.mkdir).toHaveBeenCalledOnce();
	expect(io.write).toHaveBeenCalledWith(
		expect.stringMatching(/\/0\.pdf$/),
		new Uint8Array([37, 80, 68, 70]),
	);
	expect(io.local).toHaveBeenCalledOnce();
	expect(io.native).not.toHaveBeenCalled();
	expect(importButton().disabled).toBe(false);
});
