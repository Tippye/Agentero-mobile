// @vitest-environment happy-dom
import { act, createElement, type ReactNode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BridgeClientStatus } from "@/lib/bridge/client";
import type { PaperMetadata } from "@/lib/paper/types";

const state = vi.hoisted(() => ({
	t: (key: string) => key,
	status: { paired: false, connected: false } as BridgeClientStatus,
	paper: {
		id: "example",
		path: "papers/example",
		title: "Offline paper",
		authors: [],
		tags: [],
	} as PaperMetadata,
	mounts: 0,
	remoteMounts: 0,
	dirty: null as ((dirty: boolean) => void) | null,
	error: vi.fn(),
	refresh: vi.fn(async (_rescan?: boolean) => {}),
	trash: vi.fn(async () => ({
		batchId: "batch",
		rels: ["papers/example"],
		count: 1,
	})),
	settings: null as {
		open: boolean;
		section: string;
		vaultPath: string | null;
	} | null,
}));
// Keep the real plugin bridge: `@/i18n` (pulled in by the Plaza sources) needs
// `initReactI18next`; only the translation hook is pinned to the key echo.
vi.mock("react-i18next", async (importOriginal) => ({
	...(await importOriginal<typeof import("react-i18next")>()),
	useTranslation: () => ({ t: state.t }),
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
// The settings surface has its own focused integration suite; here it is only a
// probe that records wiring and exposes the sync result to the shell.
vi.mock("@/components/mobile/mobile-settings-page", () => ({
	MobileSettingsPage: ({
		open,
		section,
		vaultPath,
		onClose,
		onSynced,
	}: {
		open: boolean;
		section: string;
		vaultPath: string | null;
		onClose: () => void;
		onSynced?: () => void | Promise<void>;
	}) => {
		state.settings = { open, section, vaultPath };
		return open
			? createElement(
					"div",
					{ "data-settings-vault": vaultPath ?? "" },
					createElement(
						"button",
						{ type: "button", onClick: () => void onSynced?.() },
						"settings-synced",
					),
					createElement(
						"button",
						{ type: "button", onClick: onClose },
						"close-settings",
					),
				)
			: null;
	},
}));
vi.mock("@/lib/mobile/pdf-processing", () => ({
	useMobilePdfProcessingRuntime: () => {},
}));
vi.mock("@/components/mobile/hooks/use-mobile-vault", () => ({
	useMobileVault: () => ({
		vaultPath: "/private/Vault",
		loading: false,
		retry: vi.fn(),
	}),
}));
vi.mock("@/components/mobile/hooks/use-mobile-papers", () => ({
	useMobilePapers: () => ({
		papers: [state.paper],
		loading: false,
		refresh: state.refresh,
	}),
}));
vi.mock("@/components/mobile/hooks/use-mobile-paper-sync", () => ({
	useMobilePaperSync: () => ({
		states: {},
		setPinned: vi.fn(),
		fetchMissing: vi.fn(async () => false),
		progressText: null,
	}),
}));
vi.mock("@/components/mobile/hooks/use-mobile-agents", () => ({
	useMobileAgents: () => ({
		agents: [],
		loading: false,
		selectedAgent: null,
		selectedAgentId: null,
		selectAgent: vi.fn(),
	}),
}));
vi.mock("@/components/mobile/hooks/use-bridge-status", () => ({
	useBridgeStatus: () => ({
		status: state.status,
		setStatus: vi.fn(),
		pairPending: null,
	}),
}));
vi.mock("@/components/mobile/hooks/use-pair-offer-links", () => ({
	usePairOfferLinks: vi.fn(),
}));

vi.mock("@/lib/paper/api", async (importOriginal) => ({
	...(await importOriginal<typeof import("@/lib/paper/api")>()),
	trashPaths: state.trash,
}));
vi.mock("@/lib/core/notify", () => ({ notifyError: state.error }));
vi.mock("@/lib/mobile/import-pdf", () => ({ importMobilePdfs: vi.fn() }));
vi.mock("@/components/mobile/mobile-agent-page", () => ({
	MobileAgentPage: () => createElement("div", null, "remote-agent"),
}));
vi.mock("@/components/agent/agent-panel", () => ({
	AgentPanel: ({
		active,
		backendControl,
		remoteOnly,
	}: {
		active: boolean;
		backendControl: ReactNode;
		remoteOnly: boolean;
	}) => {
		const [draft, setDraft] = useState("");
		useEffect(() => {
			state.remoteMounts += 1;
		}, []);
		return createElement(
			"div",
			{ "data-remote": String(remoteOnly), "data-active": String(active) },
			backendControl,
			"dsh-agent",
			draft,
			createElement(
				"button",
				{ type: "button", onClick: () => setDraft("unsent-dsh-draft") },
				"edit-remote-draft",
			),
		);
	},
}));
vi.mock("@/components/mobile/mobile-gestures", () => ({
	useHorizontalSwipe: () => ({}),
	EdgeSwipeBack: ({ children }: { children: ReactNode }) => children,
}));
vi.mock("@/components/mobile/mobile-header", () => ({
	MobileHeader: ({
		title,
		leading,
		trailing,
		onBrandClick,
	}: {
		title: string;
		leading: ReactNode;
		trailing: ReactNode;
		onBrandClick: () => void;
	}) =>
		createElement(
			"header",
			null,
			createElement(
				"button",
				{ type: "button", onClick: onBrandClick },
				"menu",
			),
			title,
			leading,
			trailing,
		),
}));
vi.mock("@/components/mobile/mobile-header-actions", () => ({
	MobileReaderModeToggle: () => null,
	MobileAgentToolbar: ({
		remoteSelected,
		onSelectBridge,
	}: {
		remoteSelected: boolean;
		onSelectBridge: () => void;
	}) =>
		remoteSelected
			? createElement(
					"button",
					{ type: "button", onClick: onSelectBridge },
					"select-bridge-backend",
				)
			: null,
}));
vi.mock("@/components/mobile/mobile-library-page", () => ({
	MobileLibraryPage: ({
		onSelect,
		onDeletePaper,
	}: {
		onSelect: (paper: PaperMetadata) => void;
		onDeletePaper?: (paper: PaperMetadata) => Promise<void>;
	}) =>
		createElement(
			"div",
			null,
			createElement(
				"button",
				{ type: "button", onClick: () => onSelect(state.paper) },
				"open-local-paper",
			),
			createElement(
				"button",
				{
					type: "button",
					onClick: () => void onDeletePaper?.(state.paper),
				},
				"delete-local-paper",
			),
		),
}));
vi.mock("@/components/mobile/mobile-nav", () => ({
	MobileNav: ({ onTab }: { onTab: (tab: string) => void }) =>
		createElement(
			"nav",
			null,
			createElement(
				"button",
				{ type: "button", onClick: () => onTab("library") },
				"library-tab",
			),
			createElement(
				"button",
				{ type: "button", onClick: () => onTab("agent") },
				"agent-tab",
			),
		),
}));
vi.mock("@/components/mobile/mobile-sidebar", () => ({
	MobileSidebar: ({
		open,
		onPairAnother,
		onSettings,
		onClose,
		navigation,
	}: {
		open: boolean;
		onPairAnother: () => void;
		onSettings: () => void;
		onClose: () => void;
		navigation: ReactNode;
	}) =>
		open
			? createElement(
					"aside",
					null,
					navigation,
					createElement(
						"button",
						{
							type: "button",
							onClick: () => {
								onSettings();
								onClose();
							},
						},
						"open-settings",
					),
					createElement(
						"button",
						{ type: "button", onClick: onPairAnother },
						"pair-desktop",
					),
				)
			: null,
}));
vi.mock("@/components/mobile/mobile-pairing", () => ({
	MobilePairing: ({ onDone }: { onDone: () => void }) =>
		createElement(
			"button",
			{ type: "button", onClick: onDone },
			"cancel-pairing",
		),
}));
vi.mock("@/components/mobile/mobile-reader-page", () => ({
	MobileReaderPage: ({
		onNotesDirtyChange,
	}: {
		onNotesDirtyChange: (dirty: boolean) => void;
	}) => {
		state.dirty = onNotesDirtyChange;
		useEffect(() => {
			state.mounts += 1;
		}, []);
		return createElement("div", { "data-reader": true }, "local-reader");
	},
}));
vi.mock("@/components/ui/dialog", async (importOriginal) => ({
	...(await importOriginal<typeof import("@/components/ui/dialog")>()),
	Dialog: ({ open, children }: { open: boolean; children: ReactNode }) =>
		open ? children : null,
	DialogContent: ({ children }: { children: ReactNode }) =>
		createElement("div", null, children),
	DialogTitle: ({ children }: { children: ReactNode }) =>
		createElement("h2", null, children),
}));

import MobileApp from "@/components/mobile/mobile-app";

let root: ReturnType<typeof createRoot>;
let host: HTMLDivElement;
async function click(text: string) {
	const button = [...host.querySelectorAll("button")].find(
		(element) => element.textContent === text,
	);
	expect(button, text).toBeDefined();
	await act(() => button?.click());
}

beforeEach(async () => {
	vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
	state.status = { paired: false, connected: false };
	state.mounts = 0;
	state.remoteMounts = 0;
	state.dirty = null;
	state.settings = null;
	state.error.mockClear();
	state.refresh.mockClear();
	state.trash.mockClear();
	host = document.createElement("div");
	root = createRoot(host);
	await act(async () => {
		root.render(createElement(MobileApp));
	});
});
afterEach(async () => {
	await act(() => root.unmount());
	vi.unstubAllGlobals();
});

describe("mobile local-first shell", () => {
	it("opens the local library while unpaired and retains its reader through Agent and optional pairing", async () => {
		expect(host.textContent).toContain("open-local-paper");
		expect(host.textContent).not.toContain("cancel-pairing");
		await click("open-local-paper");
		await click("agent-tab");
		expect(
			host.querySelector('[data-remote="true"]')?.getAttribute("data-active"),
		).toBe("true");
		await click("edit-remote-draft");
		await click("library-tab");
		expect(
			host.querySelector('[data-remote="true"]')?.getAttribute("data-active"),
		).toBe("false");
		await click("agent-tab");
		expect(host.textContent).toContain("unsent-dsh-draft");
		expect(state.remoteMounts).toBe(1);
		await click("select-bridge-backend");
		expect(host.textContent).toContain("agent.connectionRequired");
		expect(host.querySelector("[data-reader]")).not.toBeNull();
		await click("connect.title");
		expect(host.textContent).toContain("cancel-pairing");
		await click("cancel-pairing");
		await click("library-tab");
		expect(state.mounts).toBe(1);
		expect(host.textContent).toContain("Offline paper");
	});

	it("opens settings for the local Vault and rescans completed sync without losing the mounted reader", async () => {
		await click("open-local-paper");
		await act(() => state.dirty?.(true));
		await click("menu");
		await click("open-settings");
		expect(state.settings).toEqual({
			open: true,
			section: "appearance",
			vaultPath: "/private/Vault",
		});
		expect(
			host
				.querySelector("[data-settings-vault]")
				?.getAttribute("data-settings-vault"),
		).toBe("/private/Vault");
		await click("settings-synced");
		expect(state.refresh).toHaveBeenCalledExactlyOnceWith(true);
		await click("close-settings");
		expect(state.settings?.open).toBe(false);
		expect(state.mounts).toBe(1);
		expect(host.querySelector("[data-reader]")).not.toBeNull();
	});

	it("does not discard a failed or pending notes save when going back", async () => {
		await click("open-local-paper");
		await act(() => state.dirty?.(true));
		await act(() =>
			host
				.querySelector<HTMLButtonElement>('[aria-label="reader.back"]')
				?.click(),
		);
		expect(state.error).toHaveBeenCalledWith("reader.unsavedChanges");
		expect(host.querySelector("[data-reader]")).not.toBeNull();
		await act(() => state.dirty?.(false));
		await act(() =>
			host
				.querySelector<HTMLButtonElement>('[aria-label="reader.back"]')
				?.click(),
		);
		expect(host.textContent).toContain("open-local-paper");
	});

	it("trashes a paper folder through the native command and rescans the library", async () => {
		await click("delete-local-paper");
		expect(state.trash).toHaveBeenCalledExactlyOnceWith("/private/Vault", [
			"papers/example",
		]);
		expect(state.refresh).toHaveBeenCalledExactlyOnceWith(true);
		expect(state.error).not.toHaveBeenCalled();
	});

	it("refuses to delete the open paper while a draft is unsaved", async () => {
		await click("open-local-paper");
		await act(() => state.dirty?.(true));
		await click("delete-local-paper");
		expect(state.error).toHaveBeenCalledWith("reader.unsavedChanges");
		expect(state.trash).not.toHaveBeenCalled();
	});
});
