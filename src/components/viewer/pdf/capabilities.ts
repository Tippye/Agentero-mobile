import { isMobileApp } from "@/lib/core/tauri";

export type PdfViewerRuntime = "desktop" | "mobile";

/** Host workflows are independent of local annotation and sidecar persistence. */
export type PdfViewerCapabilities = Readonly<{
	ask: boolean;
	addToChat: boolean;
	/** Mobile processing uses remote APIs, including explicit desktop previews. */
	apiOnly?: boolean;
	layoutAnalysis: boolean;
	layoutTranslation: boolean;
	translatedEvidenceSelection: boolean;
	smartHighlights: boolean;
	latex: boolean;
	menuContributions: boolean;
}>;

const DESKTOP_CAPABILITIES: PdfViewerCapabilities = Object.freeze({
	ask: true,
	addToChat: true,
	layoutAnalysis: true,
	layoutTranslation: true,
	translatedEvidenceSelection: true,
	smartHighlights: true,
	latex: true,
	menuContributions: true,
});

const MOBILE_CAPABILITIES: PdfViewerCapabilities = Object.freeze({
	ask: false,
	addToChat: false,
	apiOnly: true,
	layoutAnalysis: true,
	layoutTranslation: true,
	translatedEvidenceSelection: false,
	smartHighlights: false,
	latex: false,
	menuContributions: false,
});

/** Explicit runtime also supports tablet previews in a desktop webview. */
export function pdfViewerCapabilities(
	runtime?: PdfViewerRuntime,
): PdfViewerCapabilities {
	return (runtime ?? (isMobileApp() ? "mobile" : "desktop")) === "mobile"
		? MOBILE_CAPABILITIES
		: DESKTOP_CAPABILITIES;
}
