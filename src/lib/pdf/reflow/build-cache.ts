/**
 * Persistent cache for the finished translated document.
 *
 * The document-level build repeats the same expensive work on every restart:
 * PDFium text redaction, formula fragment extraction, full CJK font embedding
 * (each embedded copy is ~20 MB), and pdf-lib serialization. The bytes depend
 * only on the source geometry IR, the exact translations, the font bytes, and
 * the reflow schema — so the finished PDF is stored and reused verbatim.
 * Local papers keep it in `papers/<id>/translated/` so vault sync can carry the
 * file to other devices; remote/pathless sessions fall back to the app cache.
 * Per-page preview builds are deliberately NOT cached: each embeds the full CJK
 * font pair, so caching them would cost ~20 MB per page for a marginal win over
 * the single final entry.
 *
 * The file is named by a SHA-256 content key and re-read through
 * `pdf-lib.load`, which validates PDF structure. A truncated or corrupt file
 * simply fails the load and the caller rebuilds.
 */

import { appCacheDir, join } from "@tauri-apps/api/path";
import {
	mkdir,
	readDir,
	readFile,
	remove,
	stat,
	writeFile,
} from "@tauri-apps/plugin-fs";
import { errorText } from "@/lib/core/error";
import { logger } from "@/lib/core/logger";
import { isTauri } from "@/lib/core/tauri";
import { isRemoteVaultHandle } from "@/lib/vault/remote/remote-vault";
import { gunzipBytes, paperSlug, pdfSha256, sha256Bytes } from "./cache-utils";
import type { ReflowFontBytes } from "./fonts";
import type { ReflowDocument } from "./types";

/** Fallback for remote/pathless sessions; local papers use `<paper>/translated/`. */
const CACHE_DIR = "reflow-build";
/** Font content pins the parser version too: new font bytes invalidate entries. */
const FONT_ASSET_REVISION = "1eeeec2a63b05fe6fe7a1f45f0955a8fa98793c4";
const REFLOW_BUILD_CACHE_VERSION = 1;
/**
 * Bump when layout/export produce different PDFs for the same IR + translations
 * + fonts (the built bytes are reused verbatim, so layout.ts or export.ts
 * changes must invalidate old entries even when the IR schema is unchanged).
 */
const REFLOW_BUILD_PIPELINE_VERSION = 4;
/**
 * Translation state churns with every page-level hide/restore, so one paper
 * can legitimately produce several distinct finals in a row. Keep the newest
 * few and drop the rest; page previews from the old per-page cache era are
 * always swept.
 */
const FINAL_KEEP_COUNT = 2;

export type TranslationCacheIdentity = {
	sourceLang: string;
	targetLang: string;
	providerId: string;
	serviceKey: string;
	/** Prompt/terminology key accepted alongside `serviceKey` on read. */
	legacyServiceKey?: string;
};

export type ReflowBuildCacheSource = {
	/** Stable paper identifier, usually the vault-relative paper folder. */
	paperKey: string | null | undefined;
	/**
	 * Absolute local paper folder. Finished PDFs live in its `translated/`
	 * directory so vault sync can carry them to other devices. Remote handles,
	 * untitled sessions and local-only translation caches fall back to the app
	 * cache.
	 */
	paperAbsPath?: string | null;
	/** Viewer bytes; keyed by content so a changed PDF never reuses old builds. */
	pdfBytes: ArrayBuffer | Uint8Array | null | undefined;
	/**
	 * Keep the finished PDF in the app cache instead of the paper's
	 * `translated/` directory. Used when the translation-cache sync switch is
	 * off: the file stays local and is never picked up by the sync snapshot.
	 */
	localOnly?: boolean;
	/** Identity written beside each final so a fast preview can match it. */
	identity?: () => TranslationCacheIdentity;
};

/** Cache operations for the finished reflow document; fakes are used by tests. */
export type ReflowBuildCacheStore = {
	/**
	 * See `ReflowIrCacheStore.writesEnabled`; omitted fakes always persist.
	 * Existing files are still read when writes are off.
	 */
	writesEnabled?: () => boolean;
	/** Full source and build identity for reusing the last rendered result. */
	keyForFinal?(
		document: ReflowDocument,
		translations: ReadonlyMap<string, string>,
		fonts: ReflowFontBytes,
	): Promise<string | null>;
	readFinal(
		document: ReflowDocument,
		translations: ReadonlyMap<string, string>,
		fonts: ReflowFontBytes,
	): Promise<Uint8Array | null>;
	writeFinal(
		document: ReflowDocument,
		translations: ReadonlyMap<string, string>,
		fonts: ReflowFontBytes,
		bytes: Uint8Array,
	): Promise<void>;
};

type BuildBase = { dir: string; pdfSha256: string };

function fontFingerprint(fonts: ReflowFontBytes): Promise<string> {
	const keys = Object.keys(fonts).sort() as (keyof ReflowFontBytes)[];
	return sha256Bytes(
		new TextEncoder().encode(
			`${FONT_ASSET_REVISION}:${keys.map((key) => fonts[key].byteLength).join(",")}`,
		),
	);
}

function stableJson(value: unknown): string {
	return JSON.stringify(value, (_key, val) =>
		val instanceof Uint8Array ? undefined : val,
	);
}

function sortedById<T extends { id: string }>(values: readonly T[]): T[] {
	return [...values].sort((a, b) => (a.id < b.id ? -1 : 1));
}

/**
 * Canonical view of the document for keying: paragraph order can jitter
 * between runs (page-level runs re-merge items, and equal-key ties resolve to
 * insertion order), which changes the PDF's draw order by a fragment without
 * changing any visible content. Sort everything addressable by id so such
 * jitter still hits the same cache entry.
 */
function canonicalDocument(document: ReflowDocument): unknown {
	return {
		version: document.version,
		pages: [...document.pages]
			.sort((a, b) => a.index - b.index)
			.map((page) => ({
				index: page.index,
				width: page.width,
				height: page.height,
				paragraphs: sortedById(page.paragraphs).map((paragraph) => ({
					...paragraph,
					characters: [...paragraph.characters].sort(
						(a, b) => a.index - b.index,
					),
					formulas: sortedById(paragraph.formulas),
				})),
				obstacles: [...page.obstacles].sort(
					(a, b) => a.x - b.x || a.y - b.y || a.w - b.w || a.h - b.h,
				),
			})),
	};
}

function translationsFingerprint(
	entries: readonly [string, string][],
): Promise<string> {
	const sorted = [...entries].sort(([a], [b]) => (a < b ? -1 : 1));
	return sha256Bytes(new TextEncoder().encode(stableJson(sorted)));
}

async function documentFingerprint(
	document: ReflowDocument,
	translations: ReadonlyMap<string, string>,
	fonts?: ReflowFontBytes,
): Promise<string> {
	const parts = [
		REFLOW_BUILD_CACHE_VERSION,
		REFLOW_BUILD_PIPELINE_VERSION,
		document.version,
		stableJson(canonicalDocument(document)),
		await translationsFingerprint([...translations]),
	];
	if (fonts) parts.push(await fontFingerprint(fonts));
	return sha256Bytes(new TextEncoder().encode(parts.join("\n")));
}

async function resolveBase(
	source: ReflowBuildCacheSource,
): Promise<BuildBase | null> {
	if (!isTauri() || !source.pdfBytes) return null;
	try {
		const pdfHash = await pdfSha256(source.pdfBytes);
		const paper = source.paperAbsPath;
		if (paper && !source.localOnly && !isRemoteVaultHandle(paper)) {
			return { dir: await join(paper, "translated"), pdfSha256: pdfHash };
		}
		return {
			dir: await join(await appCacheDir(), CACHE_DIR),
			pdfSha256: pdfHash,
		};
	} catch (error) {
		logger.warn("pdf.reflow.buildCache key failed", {
			error: errorText(error),
		});
		return null;
	}
}

/**
 * Reads tolerate both the current raw-PDF format and older gzip-wrapped
 * entries (`gunzipBytes` passes non-gzip bytes through), so upgrading does not
 * miss or corrupt existing caches.
 */
async function readCacheEntry(path: string): Promise<Uint8Array | null> {
	try {
		return gunzipBytes(await readFile(path));
	} catch {
		// Missing cache is a normal miss.
		return null;
	}
}

/**
 * Store the finished PDF as-is. PDFs with embedded fonts are already
 * compressed; gzip only saved ~10% while making the cache file impossible to
 * open outside the app. Best-effort: failures never break a translation.
 */
async function writeCacheEntry(path: string, bytes: Uint8Array): Promise<void> {
	try {
		await mkdir(path.replace(/[/\\][^/\\]+$/, ""), { recursive: true });
		await writeFile(path, bytes);
	} catch (error) {
		logger.warn("pdf.reflow.buildCache write failed", {
			error: errorText(error),
		});
	}
}

/**
 * Bound per-paper disk usage: keep the newest few finals of this PDF and sweep
 * orphaned per-page previews left by the retired per-page cache. Pruning must
 * never fail the write that triggered it.
 */
async function pruneBuildDir(dir: string, prefix: string): Promise<void> {
	try {
		const entries = await readDir(dir);
		const finals = entries.filter(
			(entry) => entry.isFile && entry.name.startsWith(`${prefix}-final-`),
		);
		const stale: string[] = [];
		const pdfs = finals.filter((entry) => entry.name.endsWith(".pdf"));
		if (pdfs.length > FINAL_KEEP_COUNT) {
			const dated = await Promise.all(
				pdfs.map(async (entry) => {
					try {
						const info = await stat(await join(dir, entry.name));
						return { name: entry.name, at: info.mtime?.getTime() ?? 0 };
					} catch {
						return { name: entry.name, at: 0 };
					}
				}),
			);
			const removed = dated
				.sort((a, b) => b.at - a.at)
				.slice(FINAL_KEEP_COUNT)
				.map((entry) => entry.name);
			for (const name of removed) {
				stale.push(name);
				stale.push(name.replace(/\.pdf$/i, ".json"));
			}
		}
		// Metadata without a PDF is an interrupted write; drop it too.
		const pdfNames = new Set(
			pdfs.map((entry) => entry.name.replace(/\.pdf$/i, ".json")),
		);
		for (const entry of finals) {
			if (entry.name.endsWith(".json") && !pdfNames.has(entry.name)) {
				stale.push(entry.name);
			}
		}
		const pagePattern = new RegExp(`^${prefix}-p\\d+-`);
		for (const entry of entries)
			if (entry.isFile && pagePattern.test(entry.name)) stale.push(entry.name);
		await Promise.all(
			[...new Set(stale)].map(async (name) =>
				remove(await join(dir, name)).catch(() => {}),
			),
		);
	} catch {
		// Pruning must never fail a translation.
	}
}

/**
 * Build the real cache store. Key material is computed lazily so a session
 * without a local paper PDF never touches disk.
 */
export function createReflowBuildCache(
	source: ReflowBuildCacheSource,
	options?: { writesEnabled?: () => boolean },
): ReflowBuildCacheStore {
	const canWrite = options?.writesEnabled ?? (() => true);
	let base: Promise<BuildBase | null> | undefined;
	const baseIdentity = () => (base ??= resolveBase(source).catch(() => null));

	const entryPath = async (fingerprint: string): Promise<string | null> => {
		const cacheBase = await baseIdentity();
		if (!cacheBase) return null;
		return join(
			cacheBase.dir,
			`${cacheBase.pdfSha256.slice(0, 16)}-${fingerprint.slice(0, 32)}.pdf`,
		);
	};
	const alternateEntryPath = async (
		fingerprint: string,
	): Promise<string | null> => {
		const cacheBase = await baseIdentity();
		if (!cacheBase) return null;
		let dir: string;
		if (source.localOnly) {
			if (!source.paperAbsPath || isRemoteVaultHandle(source.paperAbsPath)) {
				return null;
			}
			dir = await join(source.paperAbsPath, "translated");
		} else {
			try {
				dir = await join(await appCacheDir(), CACHE_DIR);
			} catch {
				return null;
			}
		}
		return join(
			dir,
			`${cacheBase.pdfSha256.slice(0, 16)}-${fingerprint.slice(0, 32)}.pdf`,
		);
	};
	const writeMetadata = async (
		path: string,
		cacheBase: BuildBase,
	): Promise<void> => {
		const identity = source.identity?.();
		if (!identity) return;
		const metadata = {
			schemaVersion: 1,
			paperKey: source.paperKey ?? null,
			paperAbsPath: source.paperAbsPath ?? null,
			pdfSha256: cacheBase.pdfSha256,
			sourceLang: identity.sourceLang,
			targetLang: identity.targetLang,
			providerId: identity.providerId,
			serviceKey: identity.serviceKey,
			legacyServiceKey: identity.legacyServiceKey ?? null,
			generatedAt: new Date().toISOString(),
		};
		await writeCacheEntry(
			path.replace(/\.pdf$/i, ".json"),
			new TextEncoder().encode(JSON.stringify(metadata)),
		);
	};

	return {
		writesEnabled: canWrite,
		async keyForFinal(document, translations, fonts) {
			if (!source.pdfBytes) return null;
			const [pdf, build] = await Promise.all([
				pdfSha256(source.pdfBytes),
				documentFingerprint(document, translations, fonts),
			]);
			return `${pdf}-${build}`;
		},
		async readFinal(document, translations, fonts) {
			const fingerprint = await documentFingerprint(
				document,
				translations,
				fonts,
			);
			const path = await entryPath(`final-${fingerprint}`);
			if (!path) return null;
			const cacheBase = await baseIdentity();
			const primary = await readCacheEntry(path);
			if (primary) {
				// Legacy finals predate the metadata sidecar; certify the hit so
				// the next click can show it before layout analysis.
				if (cacheBase) await writeMetadata(path, cacheBase);
				return primary;
			}
			// Read the other location so a paper keeps its translated PDF after
			// the sync switch flips (paper folder ↔ app cache).
			const alternate = await alternateEntryPath(`final-${fingerprint}`);
			if (!alternate || alternate === path) return null;
			const bytes = await readCacheEntry(alternate);
			if (!bytes) return null;
			if (cacheBase) await writeMetadata(alternate, cacheBase);
			if (!source.localOnly && cacheBase) {
				// The switch is on: promote a final that was built while sync was
				// off into the paper folder so the next sync publishes it.
				await writeCacheEntry(path, bytes);
				await writeMetadata(path, cacheBase);
				await pruneBuildDir(cacheBase.dir, cacheBase.pdfSha256.slice(0, 16));
			}
			return bytes;
		},
		async writeFinal(document, translations, fonts, bytes) {
			if (!canWrite()) return;
			const fingerprint = await documentFingerprint(
				document,
				translations,
				fonts,
			);
			const path = await entryPath(`final-${fingerprint}`);
			if (!path) return;
			await writeCacheEntry(path, bytes);
			const cacheBase = await baseIdentity();
			if (cacheBase) await writeMetadata(path, cacheBase);
			if (cacheBase)
				await pruneBuildDir(cacheBase.dir, cacheBase.pdfSha256.slice(0, 16));
		},
	};
}

export type CachedTranslatedPdf = {
	bytes: Uint8Array;
	path: string;
	generatedAt: string;
};

type TranslationCacheMetadata = {
	schemaVersion?: number;
	paperKey?: string | null;
	paperAbsPath?: string | null;
	pdfSha256?: string;
	sourceLang?: string;
	targetLang?: string;
	providerId?: string;
	serviceKey?: string;
	legacyServiceKey?: string | null;
	generatedAt?: string;
};

function cacheIdentityMatches(
	metadata: TranslationCacheMetadata,
	identity: TranslationCacheIdentity,
): boolean {
	if (!metadata.targetLang || !metadata.serviceKey) return false;
	if (metadata.targetLang !== identity.targetLang) return false;
	return (
		metadata.serviceKey === identity.serviceKey ||
		metadata.serviceKey === identity.legacyServiceKey ||
		(Boolean(metadata.legacyServiceKey) &&
			metadata.legacyServiceKey === identity.serviceKey)
	);
}

/**
 * Locate one finished translated PDF for the current target language/service.
 *
 * This runs before layout analysis so the reader can show the cached result
 * immediately. It deliberately requires the metadata sidecar written next to
 * each final: the content hash alone cannot tell two target languages apart.
 * Paper-local finals are checked first, then the app cache used when
 * translation-cache sync is off.
 */
export async function findCachedTranslatedPdf(opts: {
	paperAbsPath?: string | null;
	paperKey?: string | null;
	identity: TranslationCacheIdentity;
	/** Current source PDF hash; rejects a final built for an older PDF. */
	pdfSha256?: string | null;
}): Promise<CachedTranslatedPdf | null> {
	if (!isTauri()) return null;
	const dirs: string[] = [];
	if (opts.paperAbsPath && !isRemoteVaultHandle(opts.paperAbsPath)) {
		try {
			dirs.push(await join(opts.paperAbsPath, "translated"));
		} catch {
			// Ignore path failures; the app cache is still worth checking.
		}
	}
	try {
		dirs.push(await join(await appCacheDir(), CACHE_DIR));
	} catch {
		// No app cache path.
	}
	type Candidate = { pdfPath: string; generatedAt: string };
	const candidates: Candidate[] = [];
	for (const dir of dirs) {
		let entries: Awaited<ReturnType<typeof readDir>>;
		try {
			entries = await readDir(dir);
		} catch {
			continue;
		}
		for (const entry of entries) {
			if (!entry.isFile || !entry.name.endsWith(".json")) continue;
			if (!entry.name.includes("-final-")) continue;
			const metadataPath = await join(dir, entry.name);
			let metadata: TranslationCacheMetadata;
			try {
				metadata = JSON.parse(
					new TextDecoder().decode(await readFile(metadataPath)),
				) as TranslationCacheMetadata;
			} catch {
				continue;
			}
			if (!cacheIdentityMatches(metadata, opts.identity)) {
				continue;
			}
			if (
				opts.pdfSha256 &&
				metadata.pdfSha256 &&
				metadata.pdfSha256 !== opts.pdfSha256
			) {
				continue;
			}
			if (
				opts.paperKey &&
				metadata.paperKey &&
				metadata.paperKey !== opts.paperKey
			) {
				continue;
			}
			candidates.push({
				pdfPath: metadataPath.replace(/\.json$/i, ".pdf"),
				generatedAt: metadata.generatedAt ?? "",
			});
		}
	}
	candidates.sort((a, b) => b.generatedAt.localeCompare(a.generatedAt));
	for (const candidate of candidates) {
		try {
			const bytes = await gunzipBytes(await readFile(candidate.pdfPath));
			if (bytes.byteLength === 0) continue;
			// A PDF header check rejects truncated or foreign files before the
			// viewer tries to parse them.
			if (
				bytes.byteLength < 5 ||
				bytes[0] !== 0x25 ||
				bytes[1] !== 0x50 ||
				bytes[2] !== 0x44 ||
				bytes[3] !== 0x46
			) {
				continue;
			}
			return {
				bytes,
				path: candidate.pdfPath,
				generatedAt: candidate.generatedAt,
			};
		} catch {
			// Try the next candidate.
		}
	}
	return null;
}

/**
 * Remove the app-cache side of a deleted paper: local-only final PDFs and their
 * metadata, plus the reflow IR pages. Paper-local files are deleted with the
 * paper itself; this keeps the cache from outliving it when sync is off.
 */
export async function clearLocalTranslationCaches(
	rels: readonly string[],
): Promise<void> {
	if (!isTauri() || rels.length === 0) return;
	let cacheRoot: string;
	try {
		cacheRoot = await appCacheDir();
	} catch {
		return;
	}
	const removed = new Set(rels.map((rel) => rel.replace(/\\/g, "/")));
	const matches = (paperKey: string | null | undefined) =>
		Boolean(
			paperKey &&
				[...removed].some(
					(rel) => paperKey === rel || paperKey.startsWith(`${rel}/`),
				),
		);
	const buildDir = await join(cacheRoot, CACHE_DIR);
	try {
		const entries = await readDir(buildDir);
		for (const entry of entries) {
			if (!entry.isFile || !entry.name.endsWith(".json")) continue;
			if (!entry.name.includes("-final-")) continue;
			const metadataPath = await join(buildDir, entry.name);
			let metadata: TranslationCacheMetadata;
			try {
				metadata = JSON.parse(
					new TextDecoder().decode(await readFile(metadataPath)),
				) as TranslationCacheMetadata;
			} catch {
				continue;
			}
			if (!matches(metadata.paperKey)) continue;
			await remove(metadataPath).catch(() => {});
			await remove(metadataPath.replace(/\.json$/i, ".pdf")).catch(() => {});
		}
	} catch {
		// Missing build cache is normal.
	}
	const irDir = await join(cacheRoot, "reflow-ir");
	try {
		const slugs = new Set(rels.map((rel) => paperSlug(rel)));
		for (const entry of await readDir(irDir)) {
			if (!entry.isDirectory) continue;
			if (![...slugs].some((slug) => entry.name.startsWith(`${slug}-`))) {
				continue;
			}
			await remove(await join(irDir, entry.name), { recursive: true }).catch(
				() => {},
			);
		}
	} catch {
		// Missing IR cache is normal.
	}
}

/**
 * Sweep local-only finals whose paper no longer exists. Recycle-bin purges do
 * not carry the original vault paths, so this runs after a purge and only
 * removes entries that cannot belong to a live paper.
 */
export async function clearOrphanedTranslationCaches(
	vaultPath: string,
): Promise<void> {
	if (!isTauri() || isRemoteVaultHandle(vaultPath)) return;
	let buildDir: string;
	try {
		buildDir = await join(await appCacheDir(), CACHE_DIR);
	} catch {
		return;
	}
	try {
		for (const entry of await readDir(buildDir)) {
			if (!entry.isFile || !entry.name.endsWith(".json")) continue;
			if (!entry.name.includes("-final-")) continue;
			const metadataPath = await join(buildDir, entry.name);
			let metadata: TranslationCacheMetadata;
			try {
				metadata = JSON.parse(
					new TextDecoder().decode(await readFile(metadataPath)),
				) as TranslationCacheMetadata;
			} catch {
				continue;
			}
			if (!metadata.paperKey) continue;
			try {
				await stat(await join(vaultPath, metadata.paperKey));
			} catch {
				await remove(metadataPath).catch(() => {});
				await remove(metadataPath.replace(/\.json$/i, ".pdf")).catch(() => {});
			}
		}
	} catch {
		// Missing build cache is normal.
	}
}
