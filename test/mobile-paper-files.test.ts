import { readFile } from "node:fs/promises";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PaperMetadata } from "@/lib/paper/types";

const media = vi.hoisted(() => ({
	stat: vi.fn(async () => ({ size: 8 })),
	find: vi.fn(
		async () => "/private/Vault/papers/example/example.pdf" as string | null,
	),
	read: vi.fn(async () => new ArrayBuffer(8) as ArrayBuffer | null),
	remote: vi.fn(() => ({
		pdfUrl: "https://example.org/paper.pdf",
		htmlUrl: null,
	})),
}));
vi.mock("@tauri-apps/plugin-fs", () => ({ stat: media.stat }));
vi.mock("@/lib/paper/media", () => ({
	findLocalPdfPath: media.find,
	localFileToArrayBuffer: media.read,
	paperRemoteAssetsFromMetadata: media.remote,
}));

import {
	loadMobilePaperPdf,
	loadMobilePaperPdfAttachment,
	mobilePaperPaths,
} from "@/lib/mobile/paper-files";

const paper = {
	id: "example",
	path: "papers/example",
	title: "Fixture article",
} as PaperMetadata;
const fixture = Uint8Array.from(
	await readFile(new URL("./fixtures/rotate90.pdf", import.meta.url)),
).buffer;

beforeEach(() => {
	vi.clearAllMocks();
	media.find.mockResolvedValue("/private/Vault/papers/example/example.pdf");
	media.read.mockResolvedValue(new ArrayBuffer(8));
});

afterEach(() => vi.unstubAllGlobals());

describe("mobile local paper files", () => {
	it("passes true absolute paper and note paths to shared persistence", () => {
		expect(mobilePaperPaths("/private/Vault/", paper)).toEqual({
			paperAbsPath: "/private/Vault/papers/example",
			paperRelPath: "papers/example",
			notesPath: "/private/Vault/papers/example/NOTES.md",
		});
	});

	it.each([
		undefined,
		"../other",
		"papers/../other",
		"/papers/example",
		"papers/a//b",
		"papers/a/./b",
		"papers/a/../../other",
	])("rejects malformed catalog paths (%s)", (path) => {
		expect(() =>
			mobilePaperPaths("/private/Vault", { ...paper, path }),
		).toThrow("Invalid paper path");
	});

	it("reads local bytes first even when a remote URL exists and the device is offline", async () => {
		const result = await loadMobilePaperPdf("/private/Vault", paper);
		expect(result.bytes?.byteLength).toBe(8);
		expect(result.source).toBeNull();
		expect(media.find).toHaveBeenCalledWith("/private/Vault/papers/example");
		expect(media.remote).not.toHaveBeenCalled();
	});

	it("uses the metadata URL only after no readable local PDF remains", async () => {
		media.read.mockResolvedValue(null);
		expect(await loadMobilePaperPdf("/private/Vault", paper)).toEqual({
			bytes: null,
			source: "https://example.org/paper.pdf",
		});
		media.find.mockResolvedValue(null);
		media.read.mockClear();
		await loadMobilePaperPdf("/private/Vault", paper);
		expect(media.read).not.toHaveBeenCalled();
	});

	it("prepares exact primary PDF bytes, using the deferred download flow as needed", async () => {
		media.find.mockResolvedValueOnce(null);
		media.read.mockResolvedValueOnce(fixture);
		const fetchMissing = vi.fn(async () => true);
		const file = await loadMobilePaperPdfAttachment(
			"/private/Vault",
			paper,
			fixture.byteLength,
			fetchMissing,
		);
		expect(fetchMissing).toHaveBeenCalledOnce();
		expect(file.name).toBe("Fixture article.pdf");
		expect(file.type).toBe("application/pdf");
		expect(await file.arrayBuffer()).toEqual(fixture);
		expect(media.remote).not.toHaveBeenCalled();
	});

	it("rejects an oversized local PDF before reading it", async () => {
		media.stat.mockResolvedValueOnce({ size: 1024 });
		await expect(
			loadMobilePaperPdfAttachment("/private/Vault", paper, 100),
		).rejects.toThrow(/size limit/);
		expect(media.read).not.toHaveBeenCalled();
	});

	it("reads source-only PDF bytes and rejects oversized or non-PDF responses", async () => {
		media.find.mockResolvedValue(null);
		const fetch = vi
			.fn()
			.mockResolvedValueOnce(new Response(fixture))
			.mockResolvedValueOnce(new Response(fixture))
			.mockResolvedValueOnce(new Response("<html>Download</html>"));
		vi.stubGlobal("fetch", fetch);
		const load = (limit: number) =>
			loadMobilePaperPdfAttachment("/private/Vault", paper, limit);
		expect(await (await load(fixture.byteLength)).arrayBuffer()).toEqual(
			fixture,
		);
		expect(fetch).toHaveBeenCalledWith("https://example.org/paper.pdf", {
			credentials: "omit",
		});
		await expect(load(fixture.byteLength - 1)).rejects.toThrow(/size limit/);
		await expect(load(fixture.byteLength)).rejects.toThrow(/Could not read/);
	});
});
