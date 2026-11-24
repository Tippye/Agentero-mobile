import { LoaderCircle, X } from "lucide-react";
import {
	lazy,
	Suspense,
	useCallback,
	useEffect,
	useMemo,
	useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useMobileWorkspaceWidth } from "@/components/mobile/hooks/use-mobile-workspace-width";
import { MobileNotesEditor } from "@/components/mobile/mobile-notes-editor";
import { MobilePaneGroup } from "@/components/mobile/mobile-pane-group";
import type { MobileReaderMode } from "@/components/mobile/types";
import { Button } from "@/components/ui/button";
import type { PdfRegionPromptDraft } from "@/components/viewer/pdf/types";
import { notifyError } from "@/lib/core/notify";
import { exportMobileInkPdf } from "@/lib/mobile/export-pdf";
import {
	createPaperRegionContext,
	type MobilePaperRegionContext,
} from "@/lib/mobile/paper-context";
import { loadMobilePaperPdf, mobilePaperPaths } from "@/lib/mobile/paper-files";
import type {
	MobileWorkspacePreset,
	MobileWorkspaceWidth,
} from "@/lib/mobile/workspace";
import type { PaperMetadata } from "@/lib/paper/types";
import type { PdfReadingSnapshot } from "@/lib/pdf/reading-position";

const MobilePdfViewer = lazy(() =>
	import("@/components/viewer/pdf/pdf-viewer").then((module) => ({
		default: module.PdfViewer,
	})),
);

export type MobileReaderPageProps = {
	paper: PaperMetadata;
	vaultPath: string;
	mode: MobileReaderMode;
	active: boolean;
	preset?: MobileWorkspacePreset;
	widthClass?: MobileWorkspaceWidth;
	initialReadingSnapshot?: PdfReadingSnapshot | null;
	onReadingSnapshotChange?: (
		paperId: string,
		snapshot: PdfReadingSnapshot,
	) => void;
	onNotesDirtyChange: (dirty: boolean) => void;
	onInkDirtyChange?: (dirty: boolean) => void;
	onInkModeChange?: (active: boolean) => void;
	onInkExport?: (bytes: Uint8Array, suggestedName: string) => Promise<void>;
	onRegionPrompt?: (context: MobilePaperRegionContext) => void;
	fetchMissing?: (paper: PaperMetadata) => Promise<boolean>;
	/** Keep the shell preset in sync while the translation companion is open. */
	onTranslationPaneChange?: (open: boolean) => void;
};

export function MobileReaderPage({
	paper,
	vaultPath,
	mode,
	active,
	preset = "read",
	widthClass: suppliedWidth,
	initialReadingSnapshot,
	onReadingSnapshotChange,
	onNotesDirtyChange,
	onInkDirtyChange,
	onInkModeChange,
	onInkExport,
	onRegionPrompt,
	fetchMissing,
	onTranslationPaneChange,
}: MobileReaderPageProps) {
	const { t } = useTranslation("mobile");
	const measurement = useMobileWorkspaceWidth<HTMLElement>(
		suppliedWidth === undefined,
	);
	const widthClass = suppliedWidth ?? measurement.widthClass;
	const compare = widthClass !== "narrow" && preset === "compare";
	const [translationPaneOpen, setTranslationPaneOpen] = useState(false);
	const translationVisible = translationPaneOpen && preset === "compare";
	useEffect(() => {
		// The shell has already chosen a new preset, such as Discuss for a citation.
		if (preset !== "compare" && translationPaneOpen) {
			setTranslationPaneOpen(false);
		}
	}, [preset, translationPaneOpen]);
	// Narrow screens show the translation alone. Discuss can show cited notes
	// alongside Agent, while Read always shows the PDF.
	const notesVisible =
		!translationVisible && (compare || (mode === "notes" && preset !== "read"));
	const pdfVisible = translationVisible
		? widthClass !== "narrow"
		: compare || !notesVisible;
	const [pdf, setPdf] = useState<{
		paperId: string;
		paperPath: string;
		vaultPath: string;
		source: string | null;
		bytes: ArrayBuffer | null;
	} | null>(null);
	const [pdfError, setPdfError] = useState<string | null>(null);
	const [attempt, setAttempt] = useState(0);
	const paths = useMemo(() => {
		try {
			return mobilePaperPaths(vaultPath, paper);
		} catch {
			return null;
		}
	}, [vaultPath, paper]);

	const paperDirectory = paths?.paperAbsPath;
	const exportInk = useCallback(
		(bytes: Uint8Array, name: string) =>
			exportMobileInkPdf(bytes, name, paperDirectory),
		[paperDirectory],
	);

	const captureRegion = useCallback(
		(draft: PdfRegionPromptDraft) => {
			if (!onRegionPrompt) return;
			try {
				onRegionPrompt(createPaperRegionContext(paper, draft));
			} catch {
				notifyError(t("paperContext.errors.invalidImage"));
			}
		},
		[paper, onRegionPrompt, t],
	);

	useEffect(() => {
		void attempt;
		let mounted = true;
		setPdf(null);
		setPdfError(null);
		void loadMobilePaperPdf(
			vaultPath,
			paper,
			fetchMissing ? () => fetchMissing(paper) : undefined,
		)
			.then((loaded) => {
				if (!mounted) return;
				if (!loaded.source && !loaded.bytes)
					setPdfError(t("reader.pdfUnavailable"));
				else
					setPdf({
						...loaded,
						paperId: paper.id,
						paperPath: paper.path,
						vaultPath,
					});
			})
			.catch((error) => {
				if (mounted) {
					const message =
						error instanceof Error ? error.message : t("reader.pdfUnavailable");
					setPdfError(message);
					notifyError(message);
				}
			});
		return () => {
			mounted = false;
		};
	}, [vaultPath, paper, t, fetchMissing, attempt]);

	return (
		<section
			ref={measurement.ref}
			data-reader-width={widthClass}
			className="h-full min-h-0 min-w-0"
		>
			<MobilePaneGroup
				id="mobile-document"
				layoutKey={`${widthClass}:${preset}:${mode}:translation${translationPaneOpen ? 1 : 0}`}
				panes={[
					{
						id: "pdf",
						visible: pdfVisible,
						weight: 1,
						separatorLabel: t("workspace.resizeNotes"),
						content: pdfError ? (
							<div className="flex h-full flex-col items-center justify-center gap-4 p-6 text-center text-muted-foreground text-sm">
								{pdfError}
								<Button
									type="button"
									variant="outline"
									className="min-h-11"
									onClick={() => setAttempt((value) => value + 1)}
								>
									{t("local.retry")}
								</Button>
							</div>
						) : (
							<Suspense fallback={<PdfLoading />}>
								{pdf &&
								paths &&
								pdf.paperId === paper.id &&
								pdf.paperPath === paper.path &&
								pdf.vaultPath === vaultPath ? (
									<MobilePdfViewer
										source={pdf.source}
										sourceBytes={pdf.bytes}
										docId={paths.paperAbsPath}
										paperRelPath={paths.paperRelPath}
										paperAbsPath={paths.paperAbsPath}
										vaultPath={vaultPath}
										insetCommentCards={false}
										onOpenTranslationPane={() => {
											setTranslationPaneOpen(true);
											onTranslationPaneChange?.(true);
										}}
										onCloseTranslationPane={() => {
											setTranslationPaneOpen(false);
											onTranslationPaneChange?.(false);
										}}
										translationCompanionOpen={translationVisible}
										isActive={active && pdfVisible}
										initialReadingSnapshot={initialReadingSnapshot}
										onReadingSnapshotChange={(snapshot, documentId) => {
											if (documentId === paths.paperAbsPath)
												onReadingSnapshotChange?.(paper.id, snapshot);
										}}
										onInkDirtyChange={onInkDirtyChange}
										onInkModeChange={onInkModeChange}
										onInkExport={onInkExport ?? exportInk}
										onRegionPrompt={onRegionPrompt ? captureRegion : undefined}
										className="h-full"
									/>
								) : (
									<PdfLoading />
								)}
							</Suspense>
						),
					},
					{
						id: "notes",
						visible: notesVisible,
						weight: 1,
						content: paths ? (
							<MobileNotesEditor
								key={paths.notesPath}
								path={paths.notesPath}
								vaultPath={vaultPath}
								active={active && notesVisible}
								onDirtyChange={onNotesDirtyChange}
							/>
						) : (
							<div className="p-6 text-muted-foreground text-sm">
								{t("reader.pdfUnavailable")}
							</div>
						),
					},
					{
						id: "translation",
						visible: translationVisible,
						weight: 1,
						separatorLabel: t("workspace.resizeNotes"),
						content:
							translationVisible && pdf && paths ? (
								<div className="relative h-full min-h-0">
									<MobilePdfViewer
										source={pdf.source}
										sourceBytes={pdf.bytes}
										docId={`${paths.paperAbsPath}::translation`}
										paperRelPath={paths.paperRelPath}
										paperAbsPath={paths.paperAbsPath}
										vaultPath={vaultPath}
										translationPane
										translationOnly
										isActive={active && translationVisible}
										className="h-full"
									/>
									<Button
										type="button"
										variant="secondary"
										size="icon"
										className="absolute top-2 left-2 z-40 size-11 rounded-full shadow"
										aria-label={t("reader.closeTranslation")}
										onClick={() => {
											setTranslationPaneOpen(false);
											onTranslationPaneChange?.(false);
										}}
									>
										<X className="size-5" />
									</Button>
								</div>
							) : null,
					},
				]}
			/>
		</section>
	);
}

function PdfLoading() {
	const { t } = useTranslation("mobile");
	return (
		<div
			className="grid h-full place-items-center text-muted-foreground text-sm"
			role="status"
		>
			<div className="flex items-center gap-2">
				<LoaderCircle className="size-5 animate-spin" />
				{t("reader.loadingPdf")}
			</div>
		</div>
	);
}
