/**
 * Fast path for a finished translated PDF.
 *
 * Clicking 译文 normally waits for layout analysis before the reflow session can
 * read the final PDF. When that sidecar is missing but a finished PDF for the
 * current target language/service exists, show it read-only immediately and let
 * the background layout/translation job take over once it is ready.
 */

import { useEffect, useRef, useState } from "react";
import { currentLayoutTranslateCacheKey } from "@/lib/pdf/layout";
import {
	type CachedTranslatedPdf,
	findCachedTranslatedPdf,
} from "@/lib/pdf/reflow/build-cache";
import { pdfSha256 } from "@/lib/pdf/reflow/cache-utils";

export function useCachedTranslationPreview(options: {
	/** Entered 译文 and the layout sidecar is not available yet. */
	enabled: boolean;
	paperAbsPath?: string | null;
	paperKey?: string | null;
	/** Bytes of the source PDF, used to reject a final built for an older PDF. */
	pdfBytes?: ArrayBuffer | null;
	/** The normal reflow display document is ready; drop the temporary preview. */
	ready: boolean;
}): {
	preview: CachedTranslatedPdf | null;
	dismiss: () => void;
} {
	const { enabled, paperAbsPath, paperKey, pdfBytes, ready } = options;
	const [preview, setPreview] = useState<CachedTranslatedPdf | null>(null);
	const dismissedKey = useRef<string | null>(null);
	const lastPaper = useRef<string | null>(null);
	const paperIdentity = `${paperAbsPath ?? ""}:${paperKey ?? ""}`;

	useEffect(() => {
		if (lastPaper.current !== paperIdentity) {
			lastPaper.current = paperIdentity;
			dismissedKey.current = null;
		}
		if (!enabled || ready || !paperAbsPath) {
			setPreview(null);
			return;
		}
		if (dismissedKey.current === paperIdentity) return;
		let cancelled = false;
		void (async () => {
			const expectedPdfHash = pdfBytes
				? await pdfSha256(pdfBytes).catch(() => null)
				: null;
			if (cancelled) return;
			const found = await findCachedTranslatedPdf({
				paperAbsPath,
				paperKey,
				identity: currentLayoutTranslateCacheKey(),
				pdfSha256: expectedPdfHash,
			}).catch(() => null);
			if (!cancelled) setPreview(found);
		})();
		return () => {
			cancelled = true;
		};
	}, [enabled, ready, paperAbsPath, paperKey, pdfBytes, paperIdentity]);

	return {
		preview,
		dismiss: () => {
			dismissedKey.current = paperIdentity;
			setPreview(null);
		},
	};
}
