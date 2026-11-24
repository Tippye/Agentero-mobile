// @vitest-environment happy-dom

import type { ReactNode } from "react";
import { act, createElement, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import type { PdfViewerProps } from "@/components/viewer/pdf/types";
import type { MobilePaperRegionContext } from "@/lib/mobile/paper-context";
import type { MobileWindowRoute } from "@/lib/mobile/route";
import type { PaperMetadata } from "@/lib/paper/types";

const state = vi.hoisted(() => ({
	t: (key: string) => key,
	papers: [] as PaperMetadata[],
	bytes: new ArrayBuffer(8),
	pdfMounts: 0,
	notesMounts: 0,
	agentMounts: 0,
	pdf: null as PdfViewerProps | null,
	restoreRoute: null as ((route: MobileWindowRoute) => void) | null,
	systemBack: null as (() => void) | null,
	systemBackEnabled: false,
	android: false,
	regionContext: null as MobilePaperRegionContext | null,
	handleRegionContext: null as ((id: string) => void) | null,
	load: vi.fn(),
	error: vi.fn(),
	refresh: vi.fn(async () => {}),
	fetchMissing: vi.fn(async () => false),
	importPdfs: vi.fn(async (_vault: string, _files: File[]) => ({
		imported: 0,
		errors: [] as string[],
	})),
	settings: null as { open: boolean; section: string } | null,
	plaza: null as { path: string; reloadKey: number; active: boolean } | null,
}));
// Keep the real plugin bridge: `@/i18n` (pulled in by the Plaza sources) needs
// `initReactI18next`; only the translation hook is pinned to the key echo.
vi.mock("react-i18next", async (importOriginal) => ({
	...(await importOriginal<typeof import("react-i18next")>()),
	useTranslation: () => ({ t: state.t }),
}));
vi.mock("@/components/mobile/hooks/use-mobile-route", () => ({
	useMobileRoute: ({
		onRestore,
	}: {
		onRestore: (route: MobileWindowRoute) => void;
	}) => {
		state.restoreRoute = onRestore;
		return { markInteraction: vi.fn() };
	},
}));
vi.mock("@/components/mobile/hooks/use-mobile-system-back", () => ({
	useMobileSystemBack: ({
		enabled,
		onBack,
	}: {
		enabled: boolean;
		onBack: () => void;
	}) => {
		state.systemBack = onBack;
		state.systemBackEnabled = enabled;
		return state.android;
	},
}));
vi.mock("@/components/mobile/mobile-paper-import", () => ({
	MobilePaperImport: () => null,
}));
vi.mock("@/components/mobile/mobile-paper-info", () => ({
	MobilePaperInfo: () => null,
}));
vi.mock("@/components/mobile/mobile-plaza-page", () => ({
	MobilePlazaPage: (props: {
		path: string;
		reloadKey: number;
		active: boolean;
	}) => {
		state.plaza = props;
		return null;
	},
}));
// The settings surface has its own focused integration suite; here it is only a
// probe so workspace tests can assert the page opens without remounting panes.
vi.mock("@/components/mobile/mobile-settings-page", () => ({
	MobileSettingsPage: ({
		open,
		section,
		onClose,
	}: {
		open: boolean;
		section: string;
		onClose: () => void;
	}) => {
		state.settings = { open, section };
		return open
			? createElement(
					"section",
					{ "data-settings-page": section },
					createElement(
						"button",
						{
							type: "button",
							"aria-label": "close-settings",
							onClick: onClose,
						},
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
		papers: state.papers,
		loading: false,
		refresh: state.refresh,
	}),
}));
vi.mock("@/components/mobile/hooks/use-mobile-paper-sync", () => ({
	useMobilePaperSync: () => ({
		states: {},
		setPinned: vi.fn(),
		fetchMissing: state.fetchMissing,
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
		status: { paired: false, connected: false },
		setStatus: vi.fn(),
		pairPending: null,
		disconnect: vi.fn(),
		resume: vi.fn(),
		resetAvailable: false,
	}),
}));
vi.mock("@/components/mobile/hooks/use-pair-offer-links", () => ({
	usePairOfferLinks: vi.fn(),
}));
vi.mock("@/lib/core/notify", () => ({ notifyError: state.error }));
vi.mock("@/lib/mobile/import-pdf", () => ({
	importMobilePdfs: state.importPdfs,
	takeAndImportOpenedMobilePdf: vi.fn(async () => null),
}));
vi.mock("@/lib/mobile/paper-files", () => ({
	loadMobilePaperPdf: state.load,
	mobilePaperPaths: (_vault: string, paper: PaperMetadata) => ({
		paperAbsPath: `/private/Vault/${paper.path}`,
		paperRelPath: paper.path,
		notesPath: `/private/Vault/${paper.path}/NOTES.md`,
	}),
}));
vi.mock("@/components/viewer/pdf/pdf-viewer", () => ({
	PdfViewer: (props: PdfViewerProps) => {
		state.pdf = props;
		const [page, setPage] = useState(1);
		const [zoom, setZoom] = useState(100);
		useEffect(() => {
			state.pdfMounts += 1;
		}, []);
		return createElement(
			"article",
			{ "data-pdf-document": props.docId },
			createElement("output", { "data-position": true }, `${page}:${zoom}`),
			createElement(
				"button",
				{
					type: "button",
					onClick: () => {
						setPage(7);
						setZoom(175);
					},
				},
				"change-reading-position",
			),
			createElement(
				"button",
				{ type: "button", onClick: () => props.onInkDirtyChange?.(true) },
				"unsaved-ink",
			),
			createElement(
				"button",
				{ type: "button", onClick: () => props.onInkDirtyChange?.(false) },
				"saved-ink",
			),
			createElement(
				"button",
				{ type: "button", onClick: () => props.onInkModeChange?.(true) },
				"arm-pen",
			),
			createElement(
				"button",
				{ type: "button", onClick: () => props.onInkModeChange?.(false) },
				"disarm-pen",
			),
		);
	},
}));
vi.mock("@/components/mobile/mobile-notes-editor", () => ({
	MobileNotesEditor: ({
		onDirtyChange,
	}: {
		onDirtyChange: (dirty: boolean) => void;
	}) => {
		const [draft, setDraft] = useState("");
		useEffect(() => {
			state.notesMounts += 1;
		}, []);
		return createElement("textarea", {
			"aria-label": "test-notes",
			value: draft,
			onChange: (event: { target: { value: string } }) => {
				setDraft(event.target.value);
				onDirtyChange(true);
			},
		});
	},
}));
vi.mock("@/components/agent/agent-panel", () => ({
	AgentPanel: (props: {
		backendControl?: ReactNode;
		regionContext?: MobilePaperRegionContext | null;
		onRegionContextHandled?: (id: string) => void;
	}) => {
		state.regionContext = props.regionContext ?? null;
		state.handleRegionContext = props.onRegionContextHandled ?? null;
		const [draft, setDraft] = useState("");
		const [session, setSession] = useState("session-1");
		useEffect(() => {
			state.agentMounts += 1;
		}, []);
		return createElement(
			"section",
			{ "data-agent-document": true },
			props.backendControl,
			createElement("textarea", {
				"aria-label": "test-agent",
				value: draft,
				onChange: (event: { target: { value: string } }) =>
					setDraft(event.target.value),
			}),
			createElement("output", { "data-agent-session": true }, session),
			createElement(
				"button",
				{ type: "button", onClick: () => setSession("session-2") },
				"choose-session",
			),
		);
	},
}));
vi.mock("@/components/mobile/mobile-agent-page", () => ({
	MobileAgentPage: () => null,
}));
vi.mock("@/components/mobile/mobile-sidebar", () => ({
	MobileSidebar: ({
		open,
		navigation,
		onSettings,
		onClose,
	}: {
		open: boolean;
		navigation: ReactNode;
		onSettings: () => void;
		onClose: () => void;
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
vi.mock("@/components/mobile/mobile-bridge-reset", () => ({
	MobileBridgeReset: () => null,
}));
vi.mock("@/components/paper/paper-title", () => ({
	PaperTitle: ({ text }: { text: string }) => text,
}));

import MobileApp from "@/components/mobile/mobile-app";

let width = 599;
let root: ReturnType<typeof createRoot>;
let container: HTMLDivElement;
const observers = new Set<() => void>();
let ariaDisabledDescriptor: PropertyDescriptor | undefined;
const paper = (id: string): PaperMetadata => ({
	id,
	path: `papers/${id}`,
	type: "pdf",
	title: `Paper ${id}`,
	authors: [],
	tags: [],
	status: "completed",
	is_read: false,
	added_at: "2026-11-01",
	updated_at: "2026-11-01",
});
const groupsWidth = (group: HTMLElement): number => {
	const parentPanel = group.parentElement?.closest<HTMLElement>("[data-panel]");
	return parentPanel
		? panelWidth(parentPanel)
		: width - (width >= 840 ? 80 : 0);
};
const panelWidth = (panel: HTMLElement): number => {
	const group = panel.parentElement;
	if (!group) return width;
	const siblings = [...group.children].filter((element) =>
		element.hasAttribute("data-panel"),
	);
	const total = siblings.reduce(
		(sum, element) =>
			sum + Number((element as HTMLElement).style.flexGrow || 1),
		0,
	);
	const handles = [...group.children].filter(
		(element) =>
			element.hasAttribute("data-separator") &&
			!(element as HTMLElement).hidden,
	).length;
	return (
		(Math.max(0, groupsWidth(group) - handles) *
			Number(panel.style.flexGrow || 1)) /
		(total || 1)
	);
};
const rect = (element: HTMLElement): DOMRect => {
	const panel = element.hasAttribute("data-panel");
	const separator = element.hasAttribute("data-separator");
	const measuredWidth = panel
		? panelWidth(element)
		: separator
			? element.hidden
				? 0
				: 1
			: element.hasAttribute("data-group")
				? groupsWidth(element)
				: width;
	const parent = element.parentElement;
	const left = parent?.hasAttribute("data-group")
		? [...parent.children]
				.slice(0, [...parent.children].indexOf(element))
				.reduce(
					(sum, sibling) =>
						sum +
						(sibling.hasAttribute("data-panel")
							? panelWidth(sibling as HTMLElement)
							: (sibling as HTMLElement).hidden
								? 0
								: 1),
					0,
				)
		: 0;
	return {
		x: left,
		y: 0,
		left,
		top: 0,
		right: left + measuredWidth,
		bottom: 800,
		width: measuredWidth,
		height: 800,
		toJSON: () => ({}),
	};
};
async function resize(next: number) {
	width = next;
	await act(async () => {
		for (const notify of [...observers]) notify();
	});
	// Container classification changes panel geometry in the next layout delivery.
	await act(async () => {
		for (const notify of [...observers]) notify();
	});
}
async function click(label: string) {
	const element = [
		...document.querySelectorAll<HTMLButtonElement>("button"),
	].find(
		(button) =>
			!button.closest("[hidden]") &&
			(button.getAttribute("aria-label") === label ||
				button.textContent === label),
	);
	expect(element, label).toBeDefined();
	await act(async () => {
		element?.click();
	});
	await act(async () => {
		for (const notify of [...observers]) notify();
	});
}
async function selectBridgeBackend() {
	const trigger = [
		...document.querySelectorAll<HTMLButtonElement>("button"),
	].find(
		(button) =>
			!button.closest("[hidden]") && button.textContent === "dsh-remote",
	);
	expect(trigger).toBeDefined();
	await act(() =>
		trigger?.dispatchEvent(
			new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
		),
	);
	const item = [
		...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]'),
	].find((row) => row.textContent === "remote.bridgeProvider");
	expect(item).toBeDefined();
	await act(() => item?.click());
}
async function fill(label: string, value: string) {
	const field = container.querySelector<HTMLTextAreaElement>(
		`[aria-label="${label}"]`,
	);
	expect(field).not.toBeNull();
	await act(() => {
		Object.getOwnPropertyDescriptor(
			HTMLTextAreaElement.prototype,
			"value",
		)?.set?.call(field, value);
		field?.dispatchEvent(new Event("input", { bubbles: true }));
	});
}
function pane(id: string) {
	return container.querySelector<HTMLElement>(`[data-workspace-pane="${id}"]`);
}
async function ready() {
	await vi.waitFor(async () => {
		await act(async () => {});
		expect(state.pdf).not.toBeNull();
	});
}

beforeEach(async () => {
	vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
	// happy-dom omits the standard ARIA reflection used by the real resize library.
	ariaDisabledDescriptor = Object.getOwnPropertyDescriptor(
		HTMLElement.prototype,
		"ariaDisabled",
	);
	Object.defineProperty(HTMLElement.prototype, "ariaDisabled", {
		configurable: true,
		get() {
			return this.getAttribute("aria-disabled");
		},
	});
	vi.stubGlobal(
		"ResizeObserver",
		class {
			callback: () => void;
			elements = new Set<HTMLElement>();
			constructor(callback: ResizeObserverCallback) {
				this.callback = () =>
					callback(
						[...this.elements].map((element) => ({
							target: element,
							contentRect: rect(element),
							borderBoxSize: [
								{ inlineSize: rect(element).width, blockSize: 800 },
							],
							contentBoxSize: [
								{ inlineSize: rect(element).width, blockSize: 800 },
							],
							devicePixelContentBoxSize: [
								{ inlineSize: rect(element).width, blockSize: 800 },
							],
						})),
						this,
					);
				observers.add(this.callback);
			}
			observe(element: Element) {
				if (element instanceof HTMLElement) this.elements.add(element);
			}
			unobserve(element: Element) {
				if (element instanceof HTMLElement) this.elements.delete(element);
			}
			disconnect() {
				this.elements.clear();
				observers.delete(this.callback);
			}
		},
	);
	vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
		function () {
			return rect(this);
		},
	);
	vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockImplementation(
		function () {
			return rect(this).width;
		},
	);
	vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
	vi.spyOn(HTMLElement.prototype, "offsetLeft", "get").mockImplementation(
		function () {
			return rect(this).left;
		},
	);
	vi.spyOn(HTMLElement.prototype, "offsetTop", "get").mockReturnValue(0);
	width = 599;
	observers.clear();
	vi.clearAllMocks();
	state.papers = [paper("first"), paper("second")];
	state.android = false;
	state.systemBack = null;
	state.systemBackEnabled = false;
	state.pdf = null;
	state.regionContext = null;
	state.handleRegionContext = null;
	state.pdfMounts = 0;
	state.notesMounts = 0;
	state.agentMounts = 0;
	state.load.mockResolvedValue({ source: null, bytes: state.bytes });
	container = document.createElement("div");
	document.body.appendChild(container);
	root = createRoot(container);
	await act(async () => {
		root.render(createElement(MobileApp));
	});
});
afterEach(async () => {
	await act(() => root.unmount());
	container.remove();
	vi.restoreAllMocks();
	if (ariaDisabledDescriptor)
		Object.defineProperty(
			HTMLElement.prototype,
			"ariaDisabled",
			ariaDisabledDescriptor,
		);
	else Reflect.deleteProperty(HTMLElement.prototype, "ariaDisabled");
	vi.unstubAllGlobals();
	observers.clear();
});

function LateRootLayer() {
	const [open, setOpen] = useState(false);
	return createElement(
		"div",
		null,
		createElement(
			"button",
			{ type: "button", onClick: () => setOpen(true) },
			"late-root-layer",
		),
		createElement(
			Dialog,
			{ open, onOpenChange: setOpen },
			createElement(
				DialogContent,
				{ "aria-describedby": undefined },
				createElement(DialogTitle, null, "Late root request"),
			),
		),
	);
}

describe("stable tablet workspace DOM", () => {
	it("collapses the wide paper list without replacing dirty reader surfaces or opening Agent", async () => {
		await resize(1000);
		await click("Paper first");
		await ready();
		const pdf = container.querySelector("[data-pdf-document]");
		const notes = container.querySelector<HTMLTextAreaElement>(
			'[aria-label="test-notes"]',
		);
		await click("change-reading-position");
		await click("workspace.compare");
		await fill("test-notes", "keep my unsaved draft");
		await act(() => state.pdf?.onInkDirtyChange?.(true));
		const separator = container.querySelector<HTMLElement>(
			"#mobile-workspace-library-separator",
		);
		const readerWidth = pane("reader")
			?.closest("[data-panel]")
			?.getBoundingClientRect().width;
		await click("workspace.hidePaperList");
		expect(pane("library")?.hidden).toBe(true);
		expect(pane("library")?.inert).toBe(true);
		expect(separator?.hidden).toBe(true);
		expect(
			pane("reader")?.closest("[data-panel]")?.getBoundingClientRect().width,
		).toBeGreaterThan(readerWidth ?? 0);
		const expand = container.querySelector<HTMLButtonElement>(
			'[aria-label="workspace.showPaperList"]',
		);
		expect(expand?.closest("[hidden]")).toBeNull();
		expect(expand?.getAttribute("aria-expanded")).toBe("false");
		expect(
			document.getElementById(expand?.getAttribute("aria-controls") ?? ""),
		).not.toBeNull();
		await click("workspace.showPaperList");
		expect(pane("library")?.hidden).toBe(false);
		expect(separator?.hidden).toBe(false);
		expect(
			container
				.querySelector('[aria-label="workspace.hidePaperList"]')
				?.getAttribute("aria-expanded"),
		).toBe("true");
		expect(container.querySelector("[data-pdf-document]")).toBe(pdf);
		expect(container.querySelector('[aria-label="test-notes"]')).toBe(notes);
		expect(notes?.value).toBe("keep my unsaved draft");
		expect(container.querySelector("[data-position]")?.textContent).toBe(
			"7:175",
		);
		expect(pane("agent")?.hidden).toBe(true);
		expect(state.error).not.toHaveBeenCalled();
		await click("reader.back");
		expect(state.error).toHaveBeenCalledWith("reader.unsavedChanges");
		expect(container.querySelector("[data-pdf-document]")).toBe(pdf);
		expect(state.load).toHaveBeenCalledTimes(1);
		expect([state.pdfMounts, state.notesMounts]).toEqual([1, 1]);
	});

	it("keeps the list reachable across width bands and visible after leaving a collapsed reader", async () => {
		await resize(840);
		await click("Paper first");
		await ready();
		const pdf = container.querySelector("[data-pdf-document]");
		await click("workspace.hidePaperList");
		for (const size of [839, 600, 599]) {
			await resize(size);
			expect(pane("library")?.hidden).toBe(true);
			await click("settings.menu");
			await click("Paper first");
			expect(pane("reader")?.hidden).toBe(false);
			expect(pane("agent")?.hidden).toBe(true);
			expect(container.querySelector("[data-pdf-document]")).toBe(pdf);
		}
		await resize(840);
		expect(pane("library")?.hidden).toBe(true);
		await click("workspace.showPaperList");
		expect(pane("library")?.hidden).toBe(false);
		await click("workspace.hidePaperList");
		await click("reader.back");
		expect(pane("library")?.hidden).toBe(false);
		expect(pane("reader")?.hidden).toBe(true);
		await click("Paper first");
		await ready();
		expect(pane("library")?.hidden).toBe(true);
		await click("workspace.showPaperList");
		expect(pane("library")?.hidden).toBe(false);
	});
	it("leaves root system Back alone while a file import is pending", async () => {
		let finish: (value: { imported: number; errors: string[] }) => void =
			() => {};
		state.importPdfs.mockImplementationOnce(
			() =>
				new Promise((resolve) => {
					finish = resolve;
				}),
		);
		state.android = true;
		await act(() => root.render(createElement(MobileApp)));
		const input =
			container.querySelector<HTMLInputElement>('input[type="file"]');
		expect(input).not.toBeNull();
		Object.defineProperty(input, "files", {
			configurable: true,
			value: [new File(["fixture"], "paper.pdf", { type: "application/pdf" })],
		});
		await act(() =>
			input?.dispatchEvent(new Event("change", { bubbles: true })),
		);
		expect(state.importPdfs).toHaveBeenCalledOnce();
		expect(state.systemBackEnabled).toBe(false);
		await act(async () => finish({ imported: 1, errors: [] }));
		expect(state.refresh).toHaveBeenCalled();
		expect(state.systemBackEnabled).toBe(false);
	});
	it("owns Back for a late portalled dialog even at an otherwise clean library root", async () => {
		state.android = true;
		await act(() =>
			root.render(
				createElement(
					"div",
					null,
					createElement(MobileApp),
					createElement(LateRootLayer),
				),
			),
		);
		expect(state.systemBackEnabled).toBe(false);
		await click("late-root-layer");
		await vi.waitFor(async () => {
			await act(async () => {});
			expect(state.systemBackEnabled).toBe(true);
		});
		await act(() => state.systemBack?.());
		await vi.waitFor(async () => {
			await act(async () => {});
			expect(state.systemBackEnabled).toBe(false);
		});
		expect(
			document.querySelector('[role="dialog"][data-state="open"]'),
		).toBeNull();
		expect(pane("library")?.hidden).toBe(false);
	});
	it("leaves Android touch edges to the system without cancelling native touch scrolling", async () => {
		state.android = true;
		await act(() => root.render(createElement(MobileApp)));
		await click("Paper first");
		await ready();
		const reader = container.querySelector("[data-pdf-document]");
		const edge = pane("reader")?.firstElementChild as HTMLElement;
		const send = (type: string, x: number, held: boolean) => {
			const event = new Event(type, { bubbles: true, cancelable: true });
			Object.defineProperties(event, {
				touches: { value: held ? [{ clientX: x, clientY: 20 }] : [] },
				changedTouches: { value: [{ clientX: x, clientY: 20 }] },
			});
			edge.dispatchEvent(event);
			expect(event.defaultPrevented).toBe(false);
		};
		await act(() => {
			send("touchstart", 2, true);
			send("touchmove", 130, true);
			send("touchend", 130, false);
		});
		expect(container.querySelector("[data-pdf-document]")).toBe(reader);
		expect(pane("reader")?.hidden).toBe(false);
	});
	it("handles Android Back through owned surfaces and dirty ink before restoring the clean root default", async () => {
		state.android = true;
		await act(() => root.render(createElement(MobileApp)));
		await resize(840);
		expect(state.systemBackEnabled).toBe(false);
		const back = async () => {
			await act(() => state.systemBack?.());
			await resize(width);
		};
		await click("Paper first");
		await ready();
		const reader = container.querySelector("[data-pdf-document]");
		expect(state.systemBackEnabled).toBe(true);
		await click("workspace.discuss");
		await back();
		expect(pane("agent")?.hidden).toBe(true);
		expect(pane("reader")?.hidden).toBe(false);
		await click("settings.menu");
		expect(state.pdf?.isActive).toBe(false);
		await back();
		expect(state.pdf?.isActive).toBe(true);
		await act(() => state.pdf?.onInkDirtyChange?.(true));
		await back();
		expect(state.error).toHaveBeenCalledWith("reader.unsavedChanges");
		expect(container.querySelector("[data-pdf-document]")).toBe(reader);
		expect(state.systemBackEnabled).toBe(true);
		await act(() => state.pdf?.onInkDirtyChange?.(false));
		await back();
		expect(pane("reader")?.hidden).toBe(true);
		expect(state.systemBackEnabled).toBe(false);
		await click("tabs.plaza");
		expect(state.systemBackEnabled).toBe(true);
		await back();
		expect(pane("library")?.hidden).toBe(false);
		expect(state.systemBackEnabled).toBe(false);
	});
	it("keeps a failed or pending notes draft mounted when system Back is refused", async () => {
		await click("Paper first");
		await ready();
		await fill("test-notes", "pending native back draft");
		const reader = container.querySelector("[data-pdf-document]");
		await act(() => state.systemBack?.());
		await resize(width);
		expect(state.error).toHaveBeenCalledWith("reader.unsavedChanges");
		expect(container.querySelector("[data-pdf-document]")).toBe(reader);
		expect(
			container.querySelector<HTMLTextAreaElement>('[aria-label="test-notes"]')
				?.value,
		).toBe("pending native back draft");
		expect(state.systemBackEnabled).toBe(true);
	});
	it("opens the left navigation while reading on a narrow screen", async () => {
		await click("Paper first");
		await ready();
		const reader = container.querySelector("[data-pdf-document]");
		await click("settings.menu");
		expect(state.pdf?.isActive).toBe(false);
		await click("Paper first");
		expect(pane("reader")?.hidden).toBe(false);
		expect(state.pdf?.isActive).toBe(true);
		expect(container.querySelector("[data-pdf-document]")).toBe(reader);
	});
	it("opens the current paper from Plaza without replacing its reader", async () => {
		await resize(1000);
		await click("Paper first");
		await ready();
		const reader = container.querySelector("[data-pdf-document]");
		await click("tabs.plaza");
		expect(pane("plaza")?.hidden).toBe(false);
		// The header owns the source picker and icon actions on the same row.
		expect(
			container.querySelector('[aria-label="plaza.source"]'),
		).not.toBeNull();
		expect(
			container.querySelector('[aria-label="plaza.refresh"]'),
		).not.toBeNull();
		expect(
			container.querySelector('[aria-label="plaza.openBrowser"]'),
		).not.toBeNull();
		expect(
			container.querySelector('[aria-label="shortcuts:labels.magicWand"]'),
		).toBeNull();
		const firstPath = state.plaza?.path;
		expect(firstPath).toBe("agentero:plaza/cool-papers");
		expect(state.plaza?.reloadKey).toBe(0);
		await click("plaza.refresh");
		expect(state.plaza?.reloadKey).toBe(1);
		expect(state.plaza?.path).toBe(firstPath);
		await click("settings.menu");
		await click("Paper first");
		expect(pane("reader")?.hidden).toBe(false);
		expect(pane("plaza")?.hidden).toBe(true);
		expect(pane("agent")?.hidden).toBe(true);
		expect(container.querySelector("[data-pdf-document]")).toBe(reader);
	});
	it("restores and opens papers without reopening auxiliary panes", async () => {
		await resize(1000);
		await act(() =>
			state.restoreRoute?.({
				paperId: "first",
				tab: "agent",
				readerMode: "pdf",
				preset: "discuss",
				pdf: { page: 7, zoom: 1.75 },
			}),
		);
		await resize(1000);
		await ready();
		expect(pane("reader")?.hidden).toBe(false);
		expect(pane("agent")?.hidden).toBe(true);
		expect(state.pdf?.initialReadingSnapshot).toEqual({ page: 7, zoom: 1.75 });
		await click("workspace.discuss");
		expect(pane("agent")?.hidden).toBe(false);
		await click("Paper second");
		await ready();
		expect(pane("reader")?.hidden).toBe(false);
		expect(pane("agent")?.hidden).toBe(true);
		await click("workspace.discuss");
		expect(pane("agent")?.hidden).toBe(false);
		await click("tabs.agent");
		await click("tabs.library");
		expect(pane("agent")?.hidden).toBe(true);
		expect(pane("reader")?.hidden).toBe(false);
	});
	it("hands a captured source to the existing composer without replacing drafts, and fences pending or stale captures", async () => {
		await resize(840);
		await click("Paper first");
		await ready();
		await click("workspace.discuss");
		await fill("test-agent", "existing unsent question");
		const pdfNode = container.querySelector("[data-pdf-document]");
		const originalCapture = state.pdf?.onRegionPrompt;
		expect(originalCapture).toBeDefined();
		const draft = {
			page: 3,
			region: { x: 0.1, y: 0.2, w: 0.3, h: 0.4 },
			image: {
				mimeType: "image/png",
				data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j94sAAAAASUVORK5CYII=",
			},
		};
		await act(() => originalCapture?.(draft));
		const first = state.regionContext;
		expect(first).toMatchObject({
			paperId: "first",
			title: "Paper first",
			page: 3,
			region: draft.region,
		});
		expect(container.querySelector("[data-pdf-document]")).toBe(pdfNode);
		expect(
			container.querySelector<HTMLTextAreaElement>('[aria-label="test-agent"]')
				?.value,
		).toBe("existing unsent question");
		expect(pane("reader")?.hidden).toBe(false);
		expect(pane("agent")?.hidden).toBe(false);
		await act(() => originalCapture?.({ ...draft, page: 4 }));
		expect(state.regionContext).toBe(first);
		expect(state.error).toHaveBeenCalledWith("paperContext.pendingAlready");
		await act(() => state.handleRegionContext?.("stale-consumption-id"));
		expect(state.regionContext).toBe(first);
		await act(() => state.handleRegionContext?.(first?.id ?? ""));
		expect(state.regionContext).toBeNull();
		await click("Paper second");
		await ready();
		await act(() => originalCapture?.(draft));
		expect(state.regionContext).toBeNull();
		expect(state.error).toHaveBeenCalledWith("paperContext.staleCapture");
	});

	it("only exposes region handoff to the native remote provider", async () => {
		await click("Paper first");
		await ready();
		expect(state.pdf?.onRegionPrompt).toBeDefined();
		await click("tabs.agent");
		await selectBridgeBackend();
		expect(state.pdf?.onRegionPrompt).toBeUndefined();
	});

	it("adapts 599/600/839/840 and presets without replacing PDF bytes, position, notes draft, or Agent session", async () => {
		await click("Paper first");
		await ready();
		const pdf = container.querySelector("[data-pdf-document]");
		const notes = container.querySelector('[aria-label="test-notes"]');
		const agent = container.querySelector("[data-agent-document]");
		await click("change-reading-position");
		await click("reader.notes");
		await fill("test-notes", "unsaved analysis");
		await resize(600);
		expect(
			container
				.querySelector("[data-workspace-width]")
				?.getAttribute("data-workspace-width"),
		).toBe("medium");
		expect(pane("pdf")?.hidden).toBe(false);
		expect(pane("notes")?.hidden).toBe(false);
		await click("workspace.discuss");
		await fill("test-agent", "unsent question");
		await click("choose-session");
		expect(pane("reader")?.hidden).toBe(false);
		expect(pane("agent")?.hidden).toBe(false);
		expect(pane("notes")?.hidden).toBe(true);
		await resize(839);
		expect(pane("library")?.hidden).toBe(true);
		await resize(840);
		expect(pane("library")?.hidden).toBe(false);
		await click("workspace.compare");
		expect(pane("agent")?.hidden).toBe(true);
		await resize(599);
		expect(pane("pdf")?.hidden).toBe(true);
		expect(pane("notes")?.hidden).toBe(false);
		await click("tabs.agent");
		expect(pane("agent")?.hidden).toBe(false);
		expect(pane("reader")?.hidden).toBe(true);
		await resize(840);
		await click("workspace.read");
		expect(container.querySelector("[data-pdf-document]")).toBe(pdf);
		expect(container.querySelector('[aria-label="test-notes"]')).toBe(notes);
		expect(container.querySelector("[data-agent-document]")).toBe(agent);
		expect(container.querySelector("[data-position]")?.textContent).toBe(
			"7:175",
		);
		expect((notes as HTMLTextAreaElement).value).toBe("unsaved analysis");
		expect(
			container.querySelector<HTMLTextAreaElement>('[aria-label="test-agent"]')
				?.value,
		).toBe("unsent question");
		expect(container.querySelector("[data-agent-session]")?.textContent).toBe(
			"session-2",
		);
		expect(state.pdf?.sourceBytes).toBe(state.bytes);
		expect(state.load).toHaveBeenCalledTimes(1);
		expect([state.pdfMounts, state.notesMounts, state.agentMounts]).toEqual([
			1, 1, 1,
		]);
	});

	it("keeps the reader mounted for Agent navigation, settings and pairing while guarding dirty replacement/back", async () => {
		await resize(840);
		await click("Paper first");
		await ready();
		const pdf = container.querySelector("[data-pdf-document]");
		await click("workspace.compare");
		await fill("test-notes", "keep this draft");
		await click("change-reading-position");
		const position = () =>
			container.querySelector("[data-position]")?.textContent;
		expect(position()).toBe("7:175");
		await click("Paper second");
		expect(state.pdf?.paperRelPath).toBe("papers/first");
		await click("reader.back");
		expect(state.error).toHaveBeenCalledWith("reader.unsavedChanges");
		await click("tabs.agent");
		await click("tabs.library");
		await click("settings.menu");
		await click("open-settings");
		expect(container.querySelector("[data-settings-page]")).not.toBeNull();
		expect(state.settings).toEqual({ open: true, section: "appearance" });
		expect(container.querySelector("[data-pdf-document]")).toBe(pdf);
		expect(
			container.querySelector<HTMLTextAreaElement>('[aria-label="test-notes"]')
				?.value,
		).toBe("keep this draft");
		expect(position()).toBe("7:175");
		await click("close-settings");
		expect(container.querySelector("[data-settings-page]")).toBeNull();
		expect(state.settings?.open).toBe(false);
		await click("workspace.discuss");
		await selectBridgeBackend();
		await click("connect.title");
		await click("cancel-pairing");
		expect(container.querySelector("[data-pdf-document]")).toBe(pdf);
		expect(
			container.querySelector<HTMLTextAreaElement>('[aria-label="test-notes"]')
				?.value,
		).toBe("keep this draft");
		expect(position()).toBe("7:175");
		expect(state.pdfMounts).toBe(1);
		expect(state.notesMounts).toBe(1);
	});

	it("resizes through the public separator and restores the user's split after switching presets", async () => {
		await resize(600);
		await click("Paper first");
		await ready();
		await click("workspace.compare");
		const separator = container.querySelector<HTMLElement>(
			"#mobile-document-pdf-separator",
		);
		expect(separator).not.toBeNull();
		const before = Number(separator?.getAttribute("aria-valuenow"));
		await act(() => {
			separator?.dispatchEvent(
				new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }),
			);
		});
		const keyed = Number(separator?.getAttribute("aria-valuenow"));
		expect(keyed).toBeGreaterThan(before);
		const x = separator?.getBoundingClientRect().left ?? 0;
		await act(() => {
			separator?.dispatchEvent(
				new PointerEvent("pointerdown", {
					pointerType: "mouse",
					isPrimary: true,
					pointerId: 1,
					button: 0,
					buttons: 1,
					clientX: x,
					clientY: 300,
					bubbles: true,
				}),
			);
			document.dispatchEvent(
				new PointerEvent("pointermove", {
					pointerType: "mouse",
					isPrimary: true,
					pointerId: 1,
					button: 0,
					buttons: 1,
					clientX: x + 35,
					clientY: 300,
					bubbles: true,
				}),
			);
			document.dispatchEvent(
				new PointerEvent("pointerup", {
					pointerType: "mouse",
					isPrimary: true,
					pointerId: 1,
					button: 0,
					clientX: x + 35,
					clientY: 300,
					bubbles: true,
				}),
			);
		});
		const dragged = Number(separator?.getAttribute("aria-valuenow"));
		expect(dragged).toBeGreaterThan(keyed);
		await click("workspace.read");
		await click("workspace.compare");
		expect(Number(separator?.getAttribute("aria-valuenow"))).toBeCloseTo(
			dragged,
			1,
		);
		expect(state.pdfMounts).toBe(1);
		expect(state.notesMounts).toBe(1);
	});

	it("treats unsaved ink independently from clean notes and keeps armed tools out of edge navigation", async () => {
		await click("Paper first");
		await ready();
		await click("unsaved-ink");
		await click("reader.back");
		expect(state.error).toHaveBeenCalledWith("reader.unsavedChanges");
		expect(container.querySelector("[data-pdf-document]")).not.toBeNull();
		await click("saved-ink");
		await click("arm-pen");
		const edge = pane("reader")?.firstElementChild as HTMLElement;
		const send = (type: string, x: number, touches: boolean) => {
			const event = new Event(type, { bubbles: true, cancelable: true });
			Object.defineProperties(event, {
				touches: { value: touches ? [{ clientX: x, clientY: 20 }] : [] },
				changedTouches: { value: [{ clientX: x, clientY: 20 }] },
			});
			edge.dispatchEvent(event);
			return event;
		};
		await act(() => {
			send("touchstart", 2, true);
			send("touchmove", 130, true);
			send("touchend", 130, false);
		});
		expect(container.querySelector("[data-pdf-document]")).not.toBeNull();
		await click("disarm-pen");
		await click("reader.back");
		expect(container.querySelector("[data-pdf-document]")).toBeNull();
	});
});
