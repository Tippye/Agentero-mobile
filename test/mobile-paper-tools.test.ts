// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JobSnapshot, Json } from "@/lib/core/bindings";
import type { MobilePaperImportResult } from "@/lib/mobile/paper-import";
import type { PaperMetadata } from "@/lib/paper/types";

const io = vi.hoisted(() => ({
	list: vi.fn(),
	enqueue: vi.fn(),
	settled: vi.fn(),
	cancel: vi.fn(),
	progress: vi.fn(),
	stop: vi.fn(),
	extract: vi.fn(),
	get: vi.fn(),
	publication: vi.fn(),
	tags: vi.fn(),
	probe: vi.fn(),
	open: vi.fn(),
	error: vi.fn(),
	warning: vi.fn(),
	imported: vi.fn(),
	updated: vi.fn(),
	changed: vi.fn(),
	load: vi.fn(() => ({
		easyScholarKey: "",
		easyScholarRanks: ["if"],
		batchImportConcurrency: 2,
		locale: "en",
	})),
	save: vi.fn(),
	subscribe: vi.fn(),
	unsubscribe: vi.fn(),
}));
vi.mock("react-i18next", async (importOriginal) => ({
	...(await importOriginal<typeof import("react-i18next")>()),
	useTranslation: () => ({ t: (key: string) => key, i18n: { language: "en" } }),
}));
vi.mock("@/lib/core/notify", () => ({
	notifyError: io.error,
	notifyWarning: io.warning,
	notifySuccess: vi.fn(),
}));
vi.mock("@/lib/core/tauri", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: io.open }));
vi.mock("@/lib/core/tasks", () => ({
	enqueueTask: io.enqueue,
	awaitTaskSettled: io.settled,
	cancelTask: io.cancel,
}));
vi.mock("@/lib/core/bindings", () => ({
	commands: {
		jobList: io.list,
		chatImportExtract: io.extract,
		paperGet: io.get,
		paperSyncPublication: io.publication,
		paperSetTags: io.tags,
		easyScholarProbe: io.probe,
	},
	events: { jobProgress: { listen: io.progress } },
}));
vi.mock("@/lib/settings", () => ({
	loadSettings: io.load,
	saveSettingsAsync: io.save,
	subscribeSettings: io.subscribe,
	DEFAULT_TRANSLATOR_BASE_URL: "https://translator.example",
}));
vi.mock("@/lib/paper/library-store", () => ({ setEditMetaDraft: vi.fn() }));
vi.mock("@/lib/vault/remote/remote-vault", () => ({
	isRemoteVaultHandle: () => false,
	remoteSessionIdFromHandle: () => null,
}));
vi.mock("@/lib/vault/store", () => ({
	getVaultPath: () => "/desktop-unrelated-vault",
}));
vi.mock("@/components/ui/math-text", () => ({
	MathText: ({ text }: { text: string }) => createElement("span", null, text),
}));

import { MobilePaperImport } from "@/components/mobile/mobile-paper-import";
import { MobilePaperInfo } from "@/components/mobile/mobile-paper-info";
import {
	importMobilePaperTexts,
	openMobilePaperLink,
	parsePaperImportTexts,
	readMobilePaper,
} from "@/lib/mobile/paper-import";

const vault = "/private/mobile-vault";
function paper(path = "papers/a", title = "Native paper A"): PaperMetadata {
	return {
		path,
		id: path.slice(7),
		type: "arxiv",
		title,
		authors: ["Ada"],
		tags: [],
		status: "completed",
		is_read: false,
		added_at: "2026-01-01T00:00:00Z",
		updated_at: "2026-01-01T00:00:00Z",
		arxiv_id: "2401.01234",
		doi: "10.1234/paper",
		code_repo: "github.com/research/paper",
	};
}
function result(
	overrides: Partial<MobilePaperImportResult> = {},
): MobilePaperImportResult {
	return {
		imported: [],
		skills: [],
		skillCandidates: [],
		searchCandidates: [],
		skipped: [],
		errors: [],
		...overrides,
	};
}
function importedResult(pdf = true): MobilePaperImportResult {
	return result({
		imported: [
			{
				paperDir: `${vault}/papers/a`,
				path: "papers/a",
				id: "a",
				title: "Committed paper",
				usedTranslator: true,
				translatorBaseUrl: "https://translator.example",
				pdf,
				tex: false,
				paperMd: false,
				status: "created",
				assetMessages: pdf ? [] : ["PDF download unavailable"],
			},
		],
	});
}
function job(overrides: Partial<JobSnapshot> = {}): JobSnapshot {
	return {
		id: "native-job-a",
		kind: "import",
		lane: "normal",
		state: "queued",
		vaultId: vault,
		vaultPath: vault,
		paperPath: "papers",
		fingerprint: "native-lookup",
		dependsOn: [],
		depPolicy: "allSucceeded",
		progress: null,
		phase: null,
		error: null,
		force: false,
		params: { native: true, mode: "lookup", texts: ["2401.01234"] },
		host: "host",
		...overrides,
	};
}
function complete(value: MobilePaperImportResult): JobSnapshot {
	return job({
		state: "succeeded",
		params: { native: true, mode: "lookup", result: value as unknown as Json },
	});
}
function deferred<T>() {
	let resolve: (value: T) => void = () => {};
	let reject: (error: unknown) => void = () => {};
	const promise = new Promise<T>((yes, no) => {
		resolve = yes;
		reject = no;
	});
	return { promise, resolve, reject };
}
let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
async function renderImport(
	open = true,
	path = vault,
	initialText = "2401.01234",
) {
	await act(async () =>
		root.render(
			createElement(MobilePaperImport, {
				open,
				onOpenChange: io.changed,
				vaultPath: path,
				onImported: io.imported,
				initialText,
			}),
		),
	);
}
async function renderInfo(meta = paper(), open = true, path = vault) {
	await act(async () =>
		root.render(
			createElement(MobilePaperInfo, {
				open,
				onOpenChange: io.changed,
				vaultPath: path,
				paper: meta,
				onUpdated: io.updated,
			}),
		),
	);
}
function required<T extends Element>(selector: string): T {
	const element = document.querySelector<T>(selector);
	if (!element) throw new Error(`Missing element ${selector}`);
	return element;
}
function button(label: string): HTMLButtonElement {
	const found = [
		...document.querySelectorAll<HTMLButtonElement>("button"),
	].find(
		(item) =>
			item.textContent === label || item.getAttribute("aria-label") === label,
	);
	if (!found) throw new Error(`Missing button ${label}`);
	return found;
}
async function click(label: string) {
	await act(async () => button(label).click());
}
async function edit(
	element: HTMLInputElement | HTMLTextAreaElement,
	value: string,
) {
	await act(async () => {
		const prototype =
			element instanceof HTMLTextAreaElement
				? HTMLTextAreaElement.prototype
				: HTMLInputElement.prototype;
		Object.getOwnPropertyDescriptor(prototype, "value")?.set?.call(
			element,
			value,
		);
		element.dispatchEvent(new Event("input", { bubbles: true }));
	});
}

beforeEach(() => {
	Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
	vi.clearAllMocks();
	io.open.mockResolvedValue(undefined);
	io.progress.mockResolvedValue(io.stop);
	io.enqueue.mockResolvedValue(job());
	io.settled.mockResolvedValue(complete(importedResult()));
	io.extract.mockResolvedValue({
		status: "ok",
		data: {
			ok: true,
			data: { arxiv: [], dois: [], pmids: [], fetched_from: null },
		},
	});
	io.probe.mockResolvedValue({ ok: true, data: true });
	io.get.mockResolvedValue({ ok: true, data: paper() });
	io.publication.mockResolvedValue({
		ok: true,
		data: { updated: 1, metricsUpdated: 1, metricsFailed: 0 },
	});
	io.tags.mockResolvedValue({ ok: true, data: paper() });
	io.load.mockReturnValue({
		easyScholarKey: "******",
		easyScholarRanks: ["if"],
		batchImportConcurrency: 2,
		locale: "en",
		unrelated: "preserved",
	});
	io.save.mockResolvedValue({
		easyScholarKey: "********",
		easyScholarRanks: ["if"],
		batchImportConcurrency: 2,
		locale: "en",
		unrelated: "preserved",
	});
	io.subscribe.mockReturnValue(io.unsubscribe);
	container = document.createElement("div");
	document.body.append(container);
	root = createRoot(container);
});
afterEach(async () => {
	await act(async () => root.unmount());
	container.remove();
	vi.useRealTimers();
	vi.restoreAllMocks();
});

describe("native mobile paper jobs", () => {
	it("preserves title spaces and identifier lists while deduplicating repeated lines", () => {
		expect(
			parsePaperImportTexts(
				"Attention Is All You Need\n2401.01234 2402.12345；10.1234/a,2401.01234 2402.12345",
			),
		).toEqual([
			"Attention Is All You Need",
			"2401.01234 2402.12345",
			"10.1234/a",
		]);
	});
	it("uses explicit native dispatch and waits for committed terminal result, then releases progress listener", async () => {
		const pending = deferred<JobSnapshot>();
		io.settled.mockReturnValueOnce(pending.promise);
		const progress = vi.fn();
		const running = importMobilePaperTexts({
			vaultPath: vault,
			texts: ["2401.01234", "2401.01234"],
			onProgress: progress,
		});
		await Promise.resolve();
		await Promise.resolve();
		expect(io.enqueue).toHaveBeenCalledWith({
			kind: "import",
			vaultPath: vault,
			path: "papers",
			params: {
				native: true,
				mode: "lookup",
				texts: ["2401.01234"],
				translatorBaseUrl: "https://translator.example",
				concurrency: 2,
			},
		});
		const listener = io.progress.mock.calls[0][0];
		listener({ payload: { taskId: "another-job", progress: 95 } });
		listener({ payload: { taskId: "native-job-a", progress: 42 } });
		expect(progress.mock.calls).toEqual([[42]]);
		expect(io.stop).not.toHaveBeenCalled();
		pending.resolve(complete(importedResult(false)));
		expect((await running).imported[0].pdf).toBe(false);
		expect(io.stop).toHaveBeenCalledOnce();
	});
	it("rejects wrong resources, renderer routing, and missing committed result", async () => {
		for (const wrong of [
			job({ vaultId: "/other-vault" }),
			job({ vaultPath: "/other-canonical-vault" }),
			job({ host: "renderer" }),
		]) {
			io.enqueue.mockResolvedValueOnce(wrong);
			await expect(
				importMobilePaperTexts({ vaultPath: vault, texts: ["2401.01234"] }),
			).rejects.toThrow();
		}
		expect(io.settled).not.toHaveBeenCalled();
		io.settled
			.mockResolvedValueOnce(complete(result()))
			.mockResolvedValueOnce(job({ state: "succeeded" }));
		await expect(
			importMobilePaperTexts({ vaultPath: vault, texts: ["2401.01234"] }),
		).resolves.toEqual(result());
		await expect(
			importMobilePaperTexts({ vaultPath: vault, texts: ["2401.01234"] }),
		).rejects.toThrow();
	});
	it("cancels only explicitly, never turns an abort request into fake completion", async () => {
		const pending = deferred<JobSnapshot>();
		io.settled.mockReturnValueOnce(pending.promise);
		const controller = new AbortController();
		const running = importMobilePaperTexts({
			vaultPath: vault,
			texts: ["2401.01234"],
			signal: controller.signal,
		});
		await Promise.resolve();
		await Promise.resolve();
		controller.abort();
		expect(io.cancel).toHaveBeenCalledExactlyOnceWith("native-job-a");
		pending.reject(new Error("native task cancelled"));
		await expect(running).rejects.toThrow("cancelled");
		expect(io.stop).toHaveBeenCalledOnce();
	});
	it("reads the exact failed native job's stored batch errors without replay", async () => {
		io.settled.mockRejectedValueOnce(new Error("Native import failed"));
		io.list.mockResolvedValueOnce({
			status: "ok",
			data: {
				ok: true,
				data: [
					job({
						id: "another-job",
						state: "failed",
						error: "Unrelated failure",
					}),
					job({
						state: "failed",
						error: "Native import failed",
						params: {
							result: result({
								errors: ["Metadata service unavailable"],
							}) as unknown as Json,
						},
					}),
				],
			},
		});
		const batch = await importMobilePaperTexts({
			vaultPath: vault,
			texts: ["2401.01234"],
		});
		expect(batch.errors).toEqual([
			"Metadata service unavailable",
			"Native import failed",
		]);
		expect(batch.imported).toEqual([]);
		expect(io.list).toHaveBeenCalledExactlyOnceWith({
			vaultPath: vault,
			path: null,
		});
		expect(io.enqueue).toHaveBeenCalledOnce();
	});
	it("does not enqueue if explicit cancellation arrives while progress registration is pending", async () => {
		const subscription = deferred<() => void>();
		io.progress.mockReturnValueOnce(subscription.promise);
		const controller = new AbortController();
		const running = importMobilePaperTexts({
			vaultPath: vault,
			texts: ["2401.01234"],
			signal: controller.signal,
		});
		controller.abort();
		subscription.resolve(io.stop);
		await expect(running).rejects.toMatchObject({ name: "AbortError" });
		expect(io.enqueue).not.toHaveBeenCalled();
		expect(io.stop).toHaveBeenCalledOnce();
	});
	it("refuses skill installation input before native enqueue", async () => {
		await expect(
			importMobilePaperTexts({
				vaultPath: vault,
				texts: ["npx skills add owner/repo"],
			}),
		).rejects.toThrow();
		expect(io.enqueue).not.toHaveBeenCalled();
	});
});

describe("mobile paper dialog resource lifetimes", () => {
	it("does no IPC, probes, settings subscriptions or task initialization while initially closed", async () => {
		await renderImport(false);
		await renderInfo(paper(), false);
		expect(io.progress).not.toHaveBeenCalled();
		expect(io.extract).not.toHaveBeenCalled();
		expect(io.enqueue).not.toHaveBeenCalled();
		expect(io.get).not.toHaveBeenCalled();
		expect(io.probe).not.toHaveBeenCalled();
		expect(io.subscribe).not.toHaveBeenCalled();
	});
	it("does not refresh before commit; closing the dialog leaves native execution alone", async () => {
		const pending = deferred<JobSnapshot>();
		io.settled.mockReturnValueOnce(pending.promise);
		await renderImport();
		await click("lookup.add");
		expect(io.imported).not.toHaveBeenCalled();
		await renderImport(false);
		expect(io.cancel).not.toHaveBeenCalled();
		await act(async () => pending.resolve(complete(importedResult())));
		expect(io.imported).toHaveBeenCalledOnce();
	});
	it("keeps an old vault completion out of the new vault and labels missing PDFs truthfully", async () => {
		const pending = deferred<JobSnapshot>();
		io.settled.mockReturnValueOnce(pending.promise);
		await renderImport();
		await click("lookup.add");
		await renderImport(true, "/other-vault");
		await act(async () => pending.resolve(complete(importedResult())));
		expect(io.imported).not.toHaveBeenCalled();
		await renderImport(true, vault);
		io.settled.mockResolvedValueOnce(complete(importedResult(false)));
		await click("lookup.add");
		expect(document.body.textContent).toContain(
			"mobile:paperImport.pdfMissing",
		);
		expect(document.body.textContent).toContain("PDF download unavailable");
	});
	it("deduplicates rapid submit and imports a title hit through its returned identifier", async () => {
		const hit = {
			title: "Attention Is All You Need",
			authors: ["A. Author"],
			year: 2017,
			venue: "NeurIPS",
			doi: null,
			arxivId: "1706.03762",
			citationCount: 1,
			url: null,
			identifier: "arXiv:1706.03762",
			source: "arxiv",
		};
		io.settled.mockResolvedValueOnce(
			complete(
				result({ searchCandidates: [{ query: hit.title, candidates: [hit] }] }),
			),
		);
		await renderImport(true, vault, hit.title);
		await act(async () => {
			button("lookup.add").click();
			button("lookup.add").click();
		});
		expect(io.enqueue).toHaveBeenCalledOnce();
		expect(io.imported).not.toHaveBeenCalled();
		await click(`lookup.searchConfirm: ${hit.title}`);
		expect(io.enqueue.mock.calls[1][0].params.texts).toEqual([hit.identifier]);
		expect(io.imported).toHaveBeenCalledOnce();
	});
	it("keeps stale debounced extraction out of a replacement input", async () => {
		vi.useFakeTimers();
		const old = deferred<unknown>();
		io.extract.mockReturnValueOnce(old.promise);
		await renderImport(true, vault, "old chat");
		await act(async () => {
			vi.advanceTimersByTime(350);
		});
		const input = required<HTMLTextAreaElement>("textarea");
		await edit(input, "New paper title");
		await act(async () =>
			old.resolve({
				status: "ok",
				data: {
					ok: true,
					data: {
						arxiv: ["2401.00001", "2401.00002"],
						dois: [],
						pmids: [],
						fetched_from: null,
					},
				},
			}),
		);
		expect(document.body.textContent).not.toContain("2401.00001");
		expect(io.enqueue).not.toHaveBeenCalled();
	});
	it("shows partial native errors without claiming any import or refreshing", async () => {
		io.settled.mockResolvedValueOnce(
			complete(result({ errors: ["Metadata lookup unavailable"] })),
		);
		await renderImport();
		await click("lookup.add");
		expect(document.querySelector('[role="alert"]')?.textContent).toBe(
			"Metadata lookup unavailable",
		);
		expect(io.imported).not.toHaveBeenCalled();
	});
});

describe("mobile paper info reuses catalog and business details", () => {
	it("refreshes metrics from the selected native vault only and publishes after catalog reread", async () => {
		const fresh = deferred<unknown>();
		io.get.mockReturnValueOnce(fresh.promise);
		await renderInfo();
		await click("papersLibrary.syncPublication");
		expect(io.publication).toHaveBeenCalledExactlyOnceWith({
			vaultPath: vault,
			paths: ["papers/a"],
		});
		expect(io.get).toHaveBeenCalledWith({ vaultPath: vault, path: "papers/a" });
		expect(io.updated).not.toHaveBeenCalled();
		const next = {
			...paper(),
			publication: "Verified venue",
			tags: [{ name: "#easyscholar:if=4.2", color: null }],
		};
		await act(async () => fresh.resolve({ ok: true, data: next }));
		expect(io.updated).toHaveBeenCalledOnce();
		expect(document.body.textContent).toContain("IF 4.2");
		expect(document.body.textContent).not.toContain("paperInfo.zhihu");
		expect(document.body.textContent).not.toContain("paperInfo.editMeta.title");
	});
	it("never applies an old paper's metadata response to another paper", async () => {
		const fresh = deferred<unknown>();
		io.get.mockReturnValueOnce(fresh.promise);
		await renderInfo();
		await click("papersLibrary.syncPublication");
		await renderInfo(paper("papers/b", "Different paper B"));
		await act(async () =>
			fresh.resolve({ ok: true, data: { ...paper(), title: "Stale A" } }),
		);
		expect(io.updated).not.toHaveBeenCalled();
		expect(document.body.textContent).not.toContain("Stale A");
		expect(document.body.textContent).toContain("Different paper B");
	});
	it("persists tags through the selected catalog and retains hidden EasyScholar metadata", async () => {
		const initial = {
			...paper(),
			tags: [{ name: "#easyscholar:if=4.2", color: null }],
		};
		const pending = deferred<unknown>();
		io.tags.mockReturnValueOnce(pending.promise);
		await renderInfo(initial);
		await edit(
			required<HTMLInputElement>('input[placeholder="paperInfo.addTag"]'),
			"method",
		);
		await act(async () =>
			required<HTMLInputElement>(
				'input[placeholder="paperInfo.addTag"]',
			).dispatchEvent(
				new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
			),
		);
		expect(io.tags).toHaveBeenCalledWith({
			vaultPath: vault,
			path: "papers/a",
			tags: [
				{ name: "#easyscholar:if=4.2", color: null },
				{ name: "method", color: null },
			],
		});
		expect(io.updated).not.toHaveBeenCalled();
		await act(async () =>
			pending.resolve({
				ok: true,
				data: {
					...initial,
					tags: [...initial.tags, { name: "method", color: null }],
				},
			}),
		);
		expect(io.updated).toHaveBeenCalledOnce();
		expect(
			io.updated.mock.calls[0][0].tags.map((tag: string | { name: string }) =>
				typeof tag === "string" ? tag : tag.name,
			),
		).toEqual(["#easyscholar:if=4.2", "method"]);
		expect(document.body.textContent).toContain("method");
	});
	it("opens shared-panel links in the native browser, rejects unsafe URLs and never falls back to WebView", async () => {
		const fallback = vi.spyOn(window, "open").mockImplementation(() => null);
		await renderInfo({
			...paper(),
			pdf_url: "javascript:alert(1)",
			html_url: "file:///private/secret",
		});
		expect(
			[...document.querySelectorAll<HTMLAnchorElement>("a")].every((link) =>
				/^https?:/.test(link.href),
			),
		).toBe(true);
		const repository = required<HTMLAnchorElement>(
			'a[aria-label="paperInfo.codeRepo"]',
		);
		await act(async () => repository.click());
		expect(io.open).toHaveBeenCalledWith("https://github.com/research/paper");
		io.open.mockRejectedValueOnce(new Error("Native opener denied"));
		await act(async () => repository.click());
		expect(io.error).toHaveBeenCalledWith("Native opener denied");
		expect(fallback).not.toHaveBeenCalled();
		expect(await openMobilePaperLink("javascript:alert(1)")).toBe(false);
		expect(await openMobilePaperLink("file:///private/secret")).toBe(false);
	});
	it("rejects a paper_get resource mismatch rather than publishing another record", async () => {
		io.get.mockResolvedValueOnce({ ok: true, data: paper("papers/wrong") });
		await expect(readMobilePaper(vault, "papers/a")).rejects.toThrow();
	});
});
