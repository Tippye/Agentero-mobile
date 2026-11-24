import { stat } from "@tauri-apps/plugin-fs";
import i18n from "@/i18n";
import {
	findLocalPdfPath,
	localFileToArrayBuffer,
	paperRemoteAssetsFromMetadata,
} from "@/lib/paper/media";
import type { PaperMetadata } from "@/lib/paper/types";
import { joinVaultPath } from "@/lib/vault/path";

/** Catalog paths are Vault-relative. Never give a malformed row FS access. */
export function mobilePaperPaths(vaultPath: string, paper: PaperMetadata) {
	const relative = paper.path?.replace(/\\/g, "/").replace(/^\.\//, "");
	if (
		!relative?.startsWith("papers/") ||
		relative.split("/").some((part) => !part || part === "." || part === "..")
	) {
		throw new Error("Invalid paper path");
	}
	const paperAbsPath = joinVaultPath(vaultPath, relative);
	return {
		paperRelPath: relative,
		paperAbsPath,
		notesPath: joinVaultPath(paperAbsPath, "NOTES.md"),
	};
}

/** Offline bytes always win over a network candidate, without a HEAD probe. */
export async function loadMobilePaperPdf(
	vaultPath: string,
	paper: PaperMetadata,
	fetchMissing?: () => Promise<boolean>,
	maxBytes?: number,
) {
	const { paperAbsPath } = mobilePaperPaths(vaultPath, paper);
	const readLocal = async () => {
		const path = await findLocalPdfPath(paperAbsPath);
		if (!path) return null;
		if (maxBytes !== undefined) {
			const info = await stat(path).catch(() => null);
			if (info && info.size > maxBytes) throw paperPdfTooLarge();
		}
		return localFileToArrayBuffer(path);
	};
	const local = await readLocal();
	if (local) return { bytes: local, source: null };
	if (fetchMissing && (await fetchMissing())) {
		const downloaded = await readLocal();
		if (downloaded) return { bytes: downloaded, source: null };
	}
	return { bytes: null, source: paperRemoteAssetsFromMetadata(paper).pdfUrl };
}

/** Materialize the reader's primary PDF for an explicitly submitted Agent turn. */
export async function loadMobilePaperPdfAttachment(
	vaultPath: string,
	paper: PaperMetadata,
	maxBytes: number,
	fetchMissing?: () => Promise<boolean>,
): Promise<File> {
	const unavailable = () =>
		new Error(i18n.t("agent:composer.remotePaperPdfUnavailable"));
	const { bytes, source } = await loadMobilePaperPdf(
		vaultPath,
		paper,
		fetchMissing,
		maxBytes,
	);
	const chunks: BlobPart[] = [];
	if (bytes) {
		if (bytes.byteLength > maxBytes) throw paperPdfTooLarge();
		chunks.push(bytes);
	} else if (source) {
		const response = await fetch(source, { credentials: "omit" }).catch(
			() => null,
		);
		if (!response?.ok || !response.body) throw unavailable();
		if (Number(response.headers.get("content-length")) > maxBytes) {
			await response.body.cancel().catch(() => undefined);
			throw paperPdfTooLarge();
		}
		const reader = response.body.getReader();
		let size = 0;
		try {
			for (;;) {
				const chunk = await reader.read().catch(() => {
					throw unavailable();
				});
				if (chunk.done) break;
				size += chunk.value.byteLength;
				if (size > maxBytes) {
					await reader.cancel().catch(() => undefined);
					throw paperPdfTooLarge();
				}
				chunks.push(chunk.value);
			}
		} finally {
			reader.releaseLock();
		}
	}
	const title = paper.title?.trim() || "paper";
	const file = new File(chunks, `${title}.pdf`, { type: "application/pdf" });
	const header = new TextDecoder().decode(
		await file.slice(0, 1024).arrayBuffer(),
	);
	if (!header.includes("%PDF-")) throw unavailable();
	return file;
}

function paperPdfTooLarge() {
	return new Error(i18n.t("agent:composer.remotePaperPdfTooLarge"));
}
