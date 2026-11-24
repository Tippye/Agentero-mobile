import {
	CITATION_FRAGMENT_KEYS,
	cleanCitationHref,
	isAgentCitationHref,
	splitCitationHref,
} from "@/lib/agent/citation-href";
import { basenameOf, normalizeSlashes } from "@/lib/core/path";
import type { PaperMetadata } from "@/lib/paper/types";

export type MobilePaperCitationFragment =
	| { kind: "page"; page: number }
	| { kind: "native"; source: string };

type CatalogPaper = Pick<PaperMetadata, "id" | "path">;

export type MobilePaperCitation<Paper extends CatalogPaper> = {
	paper: Paper;
	kind: "pdf" | "notes";
	fragment: MobilePaperCitationFragment | null;
};

function vaultPaperPath(path: string | null | undefined): string | null {
	if (!path) return null;
	const relative = normalizeSlashes(path).replace(/^\.\//, "");
	if (
		!relative.startsWith("papers/") ||
		relative
			.split("/")
			.some((part) => !part || part === "." || part === "..") ||
		relative.includes("\0")
	) {
		return null;
	}
	return relative;
}

function decodeComponent(value: string): string | null {
	try {
		return decodeURIComponent(value);
	} catch {
		return null;
	}
}

/** Resolve a raw chat source only against the active Vault's paper catalog. */
export function resolveMobilePaperCitation<Paper extends CatalogPaper>(
	href: string,
	papers: readonly Paper[],
): MobilePaperCitation<Paper> | null {
	const raw = splitCitationHref(
		href
			.trim()
			.replace(/^<|>$/g, "")
			.replace(/[\u200B-\u200D\uFEFF]/g, ""),
	);
	// Split before decoding: the opaque ID can contain escaped slashes or hashes.
	// The Markdown renderer can normalize relative links to a leading slash.
	const stable = /^(?:\.\/|\/)?agentero-paper\/([^/\\?]+)\.pdf$/.exec(raw.path);
	const id = stable ? decodeComponent(stable[1]) : null;
	const path =
		stable || raw.path.includes("?") || /%(?:2f|5c)/i.test(raw.path)
			? null
			: vaultPaperPath(
					cleanCitationHref(raw.path).replace(/^\/(?=papers\/)/, ""),
				);
	if (!id && !path) return null;

	let match: { paper: Paper; kind: "pdf" | "notes"; source: string } | null =
		null;
	for (const paper of papers) {
		if (stable && paper.id !== id) continue;
		const folder = vaultPaperPath(paper.path);
		if (!folder) {
			if (stable) return null;
			continue;
		}
		const pdf = `${folder}/${basenameOf(folder)}.pdf`;
		const body = `${folder}/PAPER.md`;
		const notes = path === `${folder}/NOTES.md`;
		if (!stable && !notes && path !== folder && path !== pdf && path !== body) {
			continue;
		}
		if (match) return null;
		match = {
			paper,
			kind: notes ? "notes" : "pdf",
			source: path === body ? body : pdf,
		};
	}
	if (!match) return null;
	const { paper, kind, source } = match;
	if (!raw.fragment) return { paper, kind, fragment: null };
	if (kind === "notes") return null;

	const fragment = decodeComponent(raw.fragment);
	if (fragment === null) return null;
	const equals = fragment.indexOf("=");
	const key =
		equals === -1 ? null : fragment.slice(0, equals).trim().toLowerCase();
	const value = fragment.slice(equals + 1);
	if (key === "page") {
		const page = Number(value);
		if (!/^\d+$/.test(value) || !Number.isSafeInteger(page) || page < 1) {
			return null;
		}
		return { paper, kind, fragment: { kind: "page", page } };
	}

	// The Host splits its native source on the first hash before parsing the fragment.
	if (source.includes("#")) return null;
	if (key === null) {
		if (!/^\d/.test(value) || !isAgentCitationHref(href)) return null;
	} else if (!CITATION_FRAGMENT_KEYS.has(key) || !value.trim()) {
		return null;
	}
	try {
		const anchor = (key === null ? "" : `${key}=`) + encodeURIComponent(value);
		return {
			paper,
			kind,
			fragment: { kind: "native", source: `${source}#${anchor}` },
		};
	} catch {
		return null;
	}
}
