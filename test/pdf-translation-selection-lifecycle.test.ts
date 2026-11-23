// @vitest-environment jsdom

import type { PdfEngine } from "@embedpdf/models";
import { act, createElement, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { pdfViewerCapabilities } from "@/components/viewer/pdf/capabilities";
import {
	type UsePdfTranslationSelectionOptions,
	usePdfTranslationSelection,
} from "@/components/viewer/pdf/hooks/use-pdf-translation-selection";
import type { SelectionMenuState } from "@/components/viewer/pdf/types";
import { clearSelections } from "@/lib/agent/selection-store";
import {
	clearTranslationSelectionBands,
	translationSelectionBandsFor,
} from "@/lib/pdf/layout/translation-selection-bands";
import type { LayoutTranslateItem } from "@/lib/pdf/layout/types";
import {
	linkPreviewDragActive,
	resetLinkPreviewDragForTests,
} from "@/lib/pdf/link-preview-drag";

vi.mock("react-i18next", async (load) => ({
	...(await load<typeof import("react-i18next")>()),
	useTranslation: () => ({ i18n: { language: "en" } }),
}));

const items = new Map<number, readonly LayoutTranslateItem[]>([
	[
		0,
		[
			{
				id: "paragraph",
				pageIndex: 0,
				kind: "text",
				readingOrder: 0,
				bbox: { x: 0.1, y: 0.1, w: 0.5, h: 0.2 },
				source: "Original sentence.",
				translated: "中文译文",
				status: "done",
			},
		],
	],
]);
const docCap = {
	getDocument: () => ({ pages: [{ size: { width: 200, height: 200 } }] }),
} as UsePdfTranslationSelectionOptions["docCapRef"]["current"];
const readers = new Map<string, SelectionMenuState | null>();

function Reader({
	id,
	runtime = "desktop",
	enabled = true,
	engine = null,
}: {
	id: string;
	runtime?: "desktop" | "mobile";
	enabled?: boolean;
	engine?: PdfEngine | null;
}) {
	const hostRef = useRef<HTMLDivElement>(null);
	const zoomRef = useRef(1);
	const [selectionMenu, setSelectionMenu] = useState<SelectionMenuState | null>(
		null,
	);
	readers.set(id, selectionMenu);
	usePdfTranslationSelection({
		enabled:
			enabled && pdfViewerCapabilities(runtime).translatedEvidenceSelection,
		hostRef,
		zoomRef,
		engineRef: useRef(engine),
		docCapRef: useRef(docCap),
		docId: id,
		itemsByPage: items,
		selectionMenu,
		setSelectionMenu,
		paperRelPath: id,
		paperAbsPath: null,
	});
	return createElement(
		"div",
		{ ref: hostRef, "data-reader": id },
		createElement(
			"div",
			{ "data-embed-pdf-page": "0" },
			createElement(
				"div",
				{
					"data-layout-item": "paragraph",
					"data-reflow-selection": "true",
					className: "pdf-translate-selection",
				},
				"中文译文",
			),
		),
	);
}

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
let frames: Map<number, FrameRequestCallback>;
let visibility: Map<Element, (visible: boolean) => void>;

async function flushFrames() {
	await act(async () => {
		while (frames.size) {
			const batch = [...frames.values()];
			frames.clear();
			for (const callback of batch) callback(0);
		}
	});
}

async function pick(id: string, { release = true } = {}) {
	const text = container.querySelector(
		`[data-reader="${id}"] [data-layout-item]`,
	)?.firstChild;
	if (!text) throw new Error(`missing translated text in ${id}`);
	const range = document.createRange();
	range.selectNodeContents(text);
	range.getClientRects = () =>
		[new DOMRect(20, 20, 60, 15)] as unknown as DOMRectList;
	await act(async () => {
		text.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
		window.getSelection()?.removeAllRanges();
		window.getSelection()?.addRange(range);
		document.dispatchEvent(new Event("selectionchange"));
		if (release)
			text.dispatchEvent(new MouseEvent("pointerup", { bubbles: true }));
	});
	await flushFrames();
}

function copySelection() {
	const setData = vi.fn();
	const event = new Event("copy", { bubbles: true, cancelable: true });
	Object.defineProperty(event, "clipboardData", { value: { setData } });
	document.dispatchEvent(event);
	return { event, setData };
}

beforeEach(() => {
	Object.defineProperty(Range.prototype, "getClientRects", {
		configurable: true,
		writable: true,
		value: () => [],
	});

	vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
	frames = new Map();
	let frameId = 0;
	vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
		frames.set(++frameId, callback);
		return frameId;
	});
	vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
	visibility = new Map();
	vi.stubGlobal(
		"IntersectionObserver",
		class {
			constructor(private callback: IntersectionObserverCallback) {}
			observe(element: Element) {
				visibility.set(element, (isIntersecting) =>
					this.callback(
						[
							{ target: element, isIntersecting },
						] as IntersectionObserverEntry[],
						this as unknown as IntersectionObserver,
					),
				);
			}
			disconnect() {}
		},
	);
	vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(
		new DOMRect(0, 0, 200, 200),
	);
	container = document.createElement("div");
	document.body.append(container);
	root = createRoot(container);
});

afterEach(async () => {
	delete (Range.prototype as unknown as Record<string, unknown>).getClientRects;

	await act(() => root.unmount());
	window.getSelection()?.removeAllRanges();
	container.remove();
	for (const id of readers.keys()) clearTranslationSelectionBands(id);
	readers.clear();
	clearSelections();
	resetLinkPreviewDragForTests();
	vi.restoreAllMocks();
	vi.unstubAllGlobals();
});

it("keeps bands, source evidence, and translated copy text scoped to the current reader", async () => {
	const render = (enabledB = true, mountB = true) =>
		act(() =>
			root.render([
				createElement(Reader, { key: "A", id: "A" }),
				mountB &&
					createElement(Reader, {
						key: "B",
						id: "B",
						enabled: enabledB,
					}),
			]),
		);
	await render();
	for (const id of ["A", "B", "A"]) {
		await pick(id);
		const other = id === "A" ? "B" : "A";
		expect(translationSelectionBandsFor(id, 0)).toHaveLength(1);
		expect(translationSelectionBandsFor(other, 0)).toHaveLength(0);
		expect(readers.get(id)).toMatchObject({
			fromTranslation: true,
			copyText: "中文译文",
			anchor: { quote: "Original sentence." },
		});
		const copied = copySelection();
		expect(copied.event.defaultPrevented).toBe(true);
		expect(copied.setData).toHaveBeenCalledWith("text/plain", "中文译文");
	}
	// Disabling and then unmounting an unrelated viewer must not erase A.
	await render(false);
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(1);
	await render(true);
	await pick("A");
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(1);
	await render(true, false);
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(1);
});

it("uses pane visibility to reset a drag in a split view", async () => {
	await act(() =>
		root.render([
			createElement(Reader, { key: "A", id: "A" }),
			createElement(Reader, { key: "B", id: "B" }),
		]),
	);
	await pick("A");
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(1);
	expect(translationSelectionBandsFor("B", 0)).toHaveLength(0);
	await pick("A", { release: false });
	expect(linkPreviewDragActive()).toBe(true);
	const host = container.querySelector('[data-reader="A"]');
	if (!host) throw new Error("missing reader A");
	await act(() => visibility.get(host)?.(false));
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(0);
	expect(linkPreviewDragActive()).toBe(false);
	expect(window.getSelection()?.toString()).toBe("");
	await act(() => visibility.get(host)?.(true));
	await pick("A");
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(1);
	expect(linkPreviewDragActive()).toBe(false);
});

it("discards a pending source lookup when the reader is hidden and can select after returning", async () => {
	let finish!: (glyphs: never[]) => void;
	const lookup = new Promise<never[]>((resolve) => {
		finish = resolve;
	});
	const engine = {
		getPageGlyphs: () => ({ toPromise: () => lookup }),
		getTextSlices: () => ({ toPromise: async () => [] }),
	} as unknown as PdfEngine;
	await act(() => root.render(createElement(Reader, { id: "A", engine })));
	await pick("A");
	expect(readers.get("A")).toBeNull();
	const host = container.querySelector('[data-reader="A"]');
	if (!host) throw new Error("missing reader A");
	await act(() => visibility.get(host)?.(false));
	await act(async () => finish([]));
	expect(readers.get("A")).toBeNull();
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(0);
	expect(linkPreviewDragActive()).toBe(false);
	await act(() => visibility.get(host)?.(true));
	await pick("A");
	expect(readers.get("A")?.copyText).toBe("中文译文");
	expect(translationSelectionBandsFor("A", 0)).toHaveLength(1);
});

it("keeps the existing native range on mobile without publishing desktop evidence", async () => {
	await act(() =>
		root.render(createElement(Reader, { id: "mobile", runtime: "mobile" })),
	);
	await pick("mobile");
	expect(window.getSelection()?.toString()).toBe("中文译文");
	expect(readers.get("mobile")).toBeNull();
	expect(translationSelectionBandsFor("mobile", 0)).toHaveLength(0);
	expect(linkPreviewDragActive()).toBe(false);
	expect(copySelection().event.defaultPrevented).toBe(false);
});

it("double-clicks the laid-out text even when the event lands on its block", async () => {
	await act(() => root.render(createElement(Reader, { id: "A" })));
	const block = container.querySelector('[data-reader="A"] [data-layout-item]');
	if (!block) throw new Error("missing translated text");
	Range.prototype.getClientRects = () =>
		[new DOMRect(20, 20, 60, 15)] as unknown as DOMRectList;
	const press = (clientX: number) =>
		act(() => {
			block.dispatchEvent(
				new MouseEvent("dblclick", {
					bubbles: true,
					cancelable: true,
					clientX,
					clientY: 25,
				}),
			);
		});
	await press(8);
	expect(readers.get("A")).toBeNull();
	await press(30);
	expect(readers.get("A")).toMatchObject({ copyText: "中文译文" });
});
