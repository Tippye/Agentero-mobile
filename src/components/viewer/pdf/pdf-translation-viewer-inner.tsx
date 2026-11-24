/**
 * Dual-pane translation companion with its own find, citation and crossref chrome.
 */

import type { PdfLinkAnnoObject } from "@embedpdf/models";
import { useAnnotationCapability } from "@embedpdf/plugin-annotation/react";
import { useDocumentManagerCapability } from "@embedpdf/plugin-document-manager/react";
import {
	GlobalPointerProvider,
	useInteractionManagerCapability,
} from "@embedpdf/plugin-interaction-manager/react";
import { type PageLayout, useScroll } from "@embedpdf/plugin-scroll/react";
import { useSearch } from "@embedpdf/plugin-search/react";
import { useZoom, ZoomGestureWrapper } from "@embedpdf/plugin-zoom/react";
import {
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
	useSyncExternalStore,
} from "react";
import { createPortal } from "react-dom";
import { useStore } from "zustand";
import {
	createSelectionAskThread,
	useSelectionAsk,
} from "@/components/selection/use-selection-ask";
import { pdfViewerCapabilities } from "@/components/viewer/pdf/capabilities";
import { AskPopover } from "@/components/viewer/pdf/cards/ask-popover";
import { PdfCitationPreview } from "@/components/viewer/pdf/cards/citation-preview";
import { PdfCrossrefPreview } from "@/components/viewer/pdf/cards/crossref-preview";
import { PdfLinkPreviewCard } from "@/components/viewer/pdf/cards/link-preview-card";
import { SelectionMenu } from "@/components/viewer/pdf/cards/selection-menu";
import { PdfFindBar } from "@/components/viewer/pdf/chrome/pdf-find-bar";
import { PdfTranslateProgressChip } from "@/components/viewer/pdf/chrome/pdf-translate-progress";
import { EMPTY_LAYOUT_REGIONS_BY_PAGE } from "@/components/viewer/pdf/constants";
import { usePdfEngineContext } from "@/components/viewer/pdf/engine-provider";
import { EPHEMERAL_PREVIEW_OPEN_MS } from "@/components/viewer/pdf/floating-hover";
import { useHoverIntent } from "@/components/viewer/pdf/hooks/use-hover-intent";
import { usePdfCitations } from "@/components/viewer/pdf/hooks/use-pdf-citations";
import { usePdfTranslatedEvidence } from "@/components/viewer/pdf/hooks/use-pdf-companion-selection";
import { usePdfCrossrefPreview } from "@/components/viewer/pdf/hooks/use-pdf-crossref-preview";
import { usePdfFind } from "@/components/viewer/pdf/hooks/use-pdf-find";
import { usePdfHighlights } from "@/components/viewer/pdf/hooks/use-pdf-highlights";
import { usePdfInk } from "@/components/viewer/pdf/hooks/use-pdf-ink";
import { usePdfLayoutRegions } from "@/components/viewer/pdf/hooks/use-pdf-layout-regions";
import { usePdfLayoutTranslate } from "@/components/viewer/pdf/hooks/use-pdf-layout-translate";
import { usePdfLinkUrlTip } from "@/components/viewer/pdf/hooks/use-pdf-link-url-tip";
import { usePdfPaperTone } from "@/components/viewer/pdf/hooks/use-pdf-paper-tone";
import { usePdfReadingMode } from "@/components/viewer/pdf/hooks/use-pdf-reading-mode";
import { usePdfScrollSync } from "@/components/viewer/pdf/hooks/use-pdf-scroll-sync";
import { usePdfSelectionActions } from "@/components/viewer/pdf/hooks/use-pdf-selection-actions";
import { usePdfZoomControls } from "@/components/viewer/pdf/hooks/use-pdf-zoom-controls";
import { useReflowDisplayDocument } from "@/components/viewer/pdf/hooks/use-reflow-display-document";
import { getLinkDestination } from "@/components/viewer/pdf/layers/citation-links";
import {
	type PdfPageHandlers,
	PdfPageLayers,
	type PdfPageLayoutSlice,
	type PdfPageMarksSlice,
	type PdfPageModeSlice,
} from "@/components/viewer/pdf/layers/page-layers";
import {
	publishLayoutSentenceCatalog,
	publishLayoutSentencePageAnalysis,
} from "@/components/viewer/pdf/layout-sentence-hover";
import { usePdfSentenceInteraction } from "@/components/viewer/pdf/layout-sentence-pairing";
import type {
	PdfViewerInnerProps,
	ScreenPoint,
	SelectionCommentDraft,
} from "@/components/viewer/pdf/types";
import { ActiveCardScrollSync } from "@/components/viewer/pdf/viewport/active-card-scroll-sync";
import { DockviewViewport } from "@/components/viewer/pdf/viewport/dockview-viewport";
import { InkInputBridge } from "@/components/viewer/pdf/viewport/ink-input-bridge";
import { KeyboardScrollHandler } from "@/components/viewer/pdf/viewport/keyboard-scroll-handler";
import { PanDragHandler } from "@/components/viewer/pdf/viewport/pan-handler";
import { PdfHistoryKeyHandler } from "@/components/viewer/pdf/viewport/pdf-history-key-handler";
import { PdfScroller } from "@/components/viewer/pdf/viewport/pdf-scroller";
import { PdfVimKeyHandler } from "@/components/viewer/pdf/viewport/pdf-vim-key-handler";
import { WheelZoomHandler } from "@/components/viewer/pdf/viewport/wheel-zoom-handler";
import { pdfAskContextRequest } from "@/lib/agent/context-request";
import { backgroundTasksStore } from "@/lib/core/background-tasks";
import { openExternalUrl } from "@/lib/core/open-external";
import { sameRelPaperPath } from "@/lib/core/path";
import {
	buildPdfAskPrompt,
	rememberPairedAskTranslation,
} from "@/lib/pdf/ask/prompt";
import { destinationInPageBox } from "@/lib/pdf/citation-dest-keys";
import { partitionHighlightPaint } from "@/lib/pdf/highlight/translated-geometry";
import {
	mirrorLayoutResultToTranslation,
	translationSourceKey,
} from "@/lib/pdf/layout";
import {
	splitLayoutSentences,
	withHoverSentences,
} from "@/lib/pdf/layout/layout-sentences";
import {
	isInsideVisualLayoutRegion,
	sentenceHoverTextRegions,
} from "@/lib/pdf/layout/translatable";
import {
	getTranslatedLinkSource,
	subscribeTranslatedLinkSource,
} from "@/lib/pdf/layout/translated-link-source";
import {
	linkPreviewDragActive,
	subscribeLinkPreviewDrag,
} from "@/lib/pdf/link-preview-drag";
import { pickPdfSearchDocumentId } from "@/lib/pdf/search-target";
import { getTabs } from "@/lib/workspace/store";
import { findTranslationSourceId } from "@/lib/workspace/tabs";

const EMPTY_PAGE_MAP = new Map();

const EMPTY_MARKS: PdfPageMarksSlice = {
	activeAskAnchor: null,
	activeTranslateAnchor: null,
	activeVisualTrace: null,
	focusedLayoutRegion: null,
	focusedLayoutFlash: false,
	focusedLayoutFlashToken: 0,
	pinsByPage: EMPTY_PAGE_MAP,
	commentsByPage: EMPTY_PAGE_MAP,
	editingCommentId: null,
	commentWikiTarget: null,
	citationLinks: EMPTY_PAGE_MAP,
	textLinks: EMPTY_PAGE_MAP,
	activeCardId: null,
	hoveredCommentId: null,
	selectionCommentDraft: null,
	translateHighlightsByPage: EMPTY_PAGE_MAP,
	highlightQuotesByPage: EMPTY_PAGE_MAP,
	translatedHighlightsByPage: EMPTY_PAGE_MAP,
};

const NOOP = () => undefined;

const EMPTY_HANDLERS: PdfPageHandlers = {
	onOpenPin: NOOP,
	onCardHoverEnter: NOOP,
	onCardHoverLeave: NOOP,
	onCitationActivate: NOOP,
	onTextLinkActivate: NOOP,
	onCitationHover: NOOP,
	onRegionSelect: NOOP,
	onTogglePageLayoutTranslate: NOOP,
	onRetryPageLayoutTranslate: NOOP,
	onDeleteHighlightAnnotation: NOOP,
	onEditHighlightAnnotation: NOOP,
	onChangeHighlightColor: NOOP,
	onOpenComment: NOOP,
	onSaveComment: NOOP,
	onCancelComment: NOOP,
	onDeleteComment: NOOP,
	onCopyCommentLink: NOOP,
	onCopyCommentEmbed: NOOP,
	onAddCommentToChat: NOOP,
	onHoverComment: NOOP,
	onLeaveComment: NOOP,
	onLeavePin: NOOP,
	onCommitSelectionComment: NOOP,
	onSelectionCommentActiveChange: NOOP,
	onDismissSelectionComment: NOOP,
};

const PAGE_MODE: PdfPageModeSlice = {
	regionSelecting: false,
	visualCropPending: false,
	sentencePairing: true,
	translationOnly: true,
};

export function PdfTranslationViewerInner({
	runtime,
	docId,
	paperAbsPath = null,
	paperRelPath = null,
	vaultPath = null,
	sourceBytes = null,
	pdfCacheBytes = null,
	isRemotePaper = false,
	importIdentifier,
	isActive = true,
	onHandle,
	onHighlightsChange,
}: PdfViewerInnerProps) {
	const capabilities = pdfViewerCapabilities(runtime);
	usePdfScrollSync(docId);
	usePdfReadingMode(docId);
	const { provides: scroll } = useScroll(docId);
	const scrollRef = useRef(scroll);
	scrollRef.current = scroll;
	const { provides: docCap } = useDocumentManagerCapability();
	const docCapRef = useRef(docCap);
	docCapRef.current = docCap;
	const inkEditable = Boolean(paperRelPath && vaultPath && !isRemotePaper);
	// Ink is page-relative and shared with the original pane (sharedEditor keyed
	// by paper): the companion paints and edits the same strokes over its reflow
	// surface. The original pane's toolbar owns the ink controls.
	const { editor: inkEditor } = usePdfInk({
		vaultPath,
		paperRelPath,
		enabled: inkEditable,
		editable: inkEditable,
		isActive,
		sharedEditor: inkEditable,
		controlsMode: false,
	});
	// Interaction manager is registered; keep the capability subscribed so
	// GlobalPointerProvider / pan-zoom gestures stay wired.
	useInteractionManagerCapability();

	const { state: zoomState } = useZoom(docId);
	const zoomLevel = zoomState.currentZoomLevel || 1;
	const { zoomRef } = usePdfZoomControls(zoomLevel);
	const hostRef = useRef<HTMLDivElement>(null);
	const { engine } = usePdfEngineContext();
	const engineRef = useRef(engine);
	engineRef.current = engine;
	const { pdfTone } = usePdfPaperTone();
	const paperKey = paperRelPath || paperAbsPath || null;

	const { layoutRawRegions, rawRegionsByPage } = usePdfLayoutRegions(docId);
	const {
		layoutTranslateItemsByPage,
		layoutTranslatePageStateByPage,
		layoutTranslateActive,
		layoutTranslateRunning,
		layoutTranslatePreparing,
		layoutTranslateWaiting,
		layoutTranslateQueued,
		layoutTranslateProgress,
		layoutTranslatePageCounts,
		layoutTranslatePartial,
		reflowReady,
		reflowOutput,
		exportTranslatedPdf,
		ensureLayoutTranslate,
		cancelLayoutTranslate,
		layoutTranslateStatus,
		toggleLayoutTranslate,
		togglePageLayoutTranslate,
		retryPageLayoutTranslate,
	} = usePdfLayoutTranslate({
		enabled: capabilities.layoutTranslation,
		apiOnly: capabilities.apiOnly,
		reflowContext: () => {
			const engine = engineRef.current,
				document = docCapRef.current?.getDocument(docId);
			return engine && document ? { engine, document } : null;
		},
		docId,
		layoutRawRegions,
		translationPane: true,
		paperAbsPath,
		paperRelPath,
		paperKey,
		vaultPath,
		pdfBytes: pdfCacheBytes ?? sourceBytes,
	});

	useEffect(() => {
		const rawRegions = layoutRawRegions ?? [];
		publishLayoutSentencePageAnalysis(
			docId,
			rawRegions.map((region) => region.pageIndex),
		);
		const translated = [...layoutTranslateItemsByPage.values()]
			.flat()
			.filter((item) => !isInsideVisualLayoutRegion(item, rawRegions))
			.map((item) => withHoverSentences(item));
		const translatedIds = new Set(translated.map((item) => item.id));
		// Blocks that are not translated yet still occupy the source PDF on
		// this pane. Publish their sentences so a hover can paint there.
		const source = sentenceHoverTextRegions(rawRegions)
			.filter((region) => !translatedIds.has(region.id))
			.map((region) => ({
				id: region.id,
				pageIndex: region.pageIndex,
				bbox: region.bbox,
				source: region.text ?? "",
				sentences: splitLayoutSentences(region.text ?? "").map((quote) => ({
					quote,
					translated: "",
				})),
			}));
		publishLayoutSentenceCatalog(docId, [...source, ...translated]);
	}, [docId, layoutTranslateItemsByPage, layoutRawRegions]);
	useEffect(() => {
		return () => {
			publishLayoutSentenceCatalog(docId, []);
			publishLayoutSentencePageAnalysis(docId, []);
		};
	}, [docId]);
	usePdfSentenceInteraction(docId, hostRef, undefined, {
		translatedTextHitTest: capabilities.translatedEvidenceSelection,
	});
	const { provides: annotationCap } = useAnnotationCapability();
	const onHighlightsChangeRef = useRef(onHighlightsChange);
	onHighlightsChangeRef.current = onHighlightsChange;
	const highlights = usePdfHighlights({
		annotationCap,
		docCap,
		docId,
		paperAbsPath,
		paperKey,
		totalPages: docCap?.getDocument(docId)?.pages.length ?? 0,
		onHighlightsChangeRef,
	});
	const translatedEvidenceEnabled =
		capabilities.translatedEvidenceSelection &&
		capabilities.layoutTranslation &&
		layoutTranslatePageStateByPage.size > 0;
	const [noteDraft, setNoteDraft] = useState<SelectionCommentDraft | null>(
		null,
	);
	const activeSessionRef = useRef<string | null>(null);
	const ask = useSelectionAsk<ScreenPoint>({
		activeSessionRef,
		buildPrompt: buildPdfAskPrompt,
		context: pdfAskContextRequest,
	});
	const { handleCommitSelectionNote } = usePdfSelectionActions(highlights);
	const dismissNote = useCallback(() => setNoteDraft(null), []);
	const commitNote = useCallback(
		(comment: string) => {
			if (noteDraft) handleCommitSelectionNote(noteDraft, comment);
			dismissNote();
		},
		[noteDraft, handleCommitSelectionNote, dismissNote],
	);
	const translatedEvidence = usePdfTranslatedEvidence({
		enabled: translatedEvidenceEnabled,
		docId,
		hostRef,
		zoomRef,
		zoom: zoomLevel,
		engineRef,
		docCapRef,
		itemsByPage: layoutTranslateItemsByPage,
		paperRelPath,
		paperAbsPath,
		createHighlights: highlights.createHighlights,
		canMark: !isRemotePaper,
		onAsk: (anchor, translation, screen) => {
			const thread = createSelectionAskThread(
				paperRelPath ?? paperAbsPath ?? "PDF",
				anchor.quote ?? "",
			);
			thread.anchor = anchor;
			if (translation) rememberPairedAskTranslation(thread.id, translation);
			ask.setAsk({ thread, screen });
		},
		onNote: setNoteDraft,
	});
	const evidenceMenuOpen = Boolean(translatedEvidence.menu);
	const { hideAsk } = ask;
	useEffect(() => {
		if (translatedEvidenceEnabled && isActive) return;
		hideAsk();
		dismissNote();
	}, [translatedEvidenceEnabled, isActive, hideAsk, dismissNote]);
	const highlightPaint = useMemo(
		() => partitionHighlightPaint(highlights.highlights),
		[highlights.highlights],
	);

	const reflowDisplayDocument = useReflowDisplayDocument(
		docCap,
		reflowOutput,
		docId,
	);
	const searchDocumentId = pickPdfSearchDocumentId({
		sourceDocumentId: docId,
		translatedDocumentId: reflowDisplayDocument?.id,
		translationVisible: layoutTranslatePageStateByPage.size > 0,
	});
	const { state: searchState, provides: search } = useSearch(searchDocumentId);
	const searchScale = searchDocumentId === docId ? undefined : zoomLevel;
	const {
		findOpen,
		findQuery,
		setFindQuery,
		findInputRef,
		findTotal,
		findActiveIndex,
		findNext,
		findPrev,
		closeFind,
	} = usePdfFind({ hostRef, search, searchState, scroll });
	const findNextRef = useRef(findNext);
	findNextRef.current = findNext;
	const handleFindNext = useCallback(() => {
		findNextRef.current();
	}, []);
	const findPrevRef = useRef(findPrev);
	findPrevRef.current = findPrev;
	const handleFindPrev = useCallback(() => {
		findPrevRef.current();
	}, []);
	const closeFindRef = useRef(closeFind);
	closeFindRef.current = closeFind;
	const handleCloseFind = useCallback(() => {
		closeFindRef.current();
	}, []);

	// `openTranslationTab` seeds a snapshot at open time; if layout analysis was
	// still running then, mirror the source result when it lands so the
	// auto-start below can fire.
	useEffect(() => {
		const sourceKey =
			findTranslationSourceId(getTabs(), docId) ?? translationSourceKey(docId);
		if (!sourceKey) return;
		return mirrorLayoutResultToTranslation(docId, sourceKey);
	}, [docId]);

	const translationAutoStartedRef = useRef(false);
	const hadLayoutRegionsRef = useRef(false);
	const layoutTranslateActiveRef = useRef(layoutTranslateActive);
	layoutTranslateActiveRef.current = layoutTranslateActive;
	const layoutTranslateRunningRef = useRef(layoutTranslateRunning);
	layoutTranslateRunningRef.current = layoutTranslateRunning;
	const ensureLayoutTranslateRef = useRef(ensureLayoutTranslate);
	ensureLayoutTranslateRef.current = ensureLayoutTranslate;
	/** Stop also ends local page generation, not just a Host job. */
	const handleStopTranslate = useCallback(() => {
		cancelLayoutTranslate({ stopTypeset: true });
	}, [cancelLayoutTranslate]);
	// A shared job for this paper can outlive a 译文 run: switching to 对照 while
	// 重译整篇 is still working mounts this pane on top of its cached sidecar.
	const liveTranslateJobId = useStore(backgroundTasksStore, (store) => {
		const task = store.tasks.find(
			(task) =>
				task.kind === "layoutTranslate" &&
				(task.status === "running" || task.status === "queued") &&
				sameRelPaperPath(task.paperPath ?? null, paperRelPath),
		);
		return task?.id ?? null;
	});
	/** Job this pane already followed; one ensure() per job, never a retry loop. */
	const followedTranslateJobRef = useRef<string | null>(null);

	useEffect(() => {
		if (!capabilities.layoutTranslation) return;
		const hasRegions = (layoutRawRegions?.length ?? 0) > 0;
		if (hasRegions) {
			hadLayoutRegionsRef.current = true;
		} else if (hadLayoutRegionsRef.current) {
			translationAutoStartedRef.current = false;
			hadLayoutRegionsRef.current = false;
		}
		// 对照 puts every progress/stop surface in this pane, so a live job must
		// be followed even when cached blocks are already painted. Only once per
		// job: clicking stop flips the local status to "cancelled" before the Host
		// row leaves "running", and another ensure() would set the status back to
		// running and undo that stop. `ensureLayoutTranslate` attaches to the
		// existing job without stopping or hiding anything.
		if (
			liveTranslateJobId !== null &&
			liveTranslateJobId !== followedTranslateJobRef.current &&
			!layoutTranslateRunning &&
			layoutTranslateStatus !== "cancelled"
		) {
			followedTranslateJobRef.current = liveTranslateJobId;
			ensureLayoutTranslateRef.current();
			translationAutoStartedRef.current = true;
			return;
		}
		if (translationAutoStartedRef.current) return;
		if (!hasRegions) {
			// Layout is not ready yet. Park the job on the shared analysis queue
			// so the user sees the "queued 闁?waiting for layout analysis" toast
			// (with a cancel action) instead of a silent empty pane. The hook
			// starts the job once regions land; the mirror above delivers them
			// even when the source pane finishes after this pane opened.
			translationAutoStartedRef.current = true;
			toggleLayoutTranslate();
			return;
		}
		if (layoutTranslateActive || layoutTranslateRunning) {
			translationAutoStartedRef.current = true;
			return;
		}
		const timer = window.setTimeout(() => {
			if (translationAutoStartedRef.current) return;
			if (
				layoutTranslateActiveRef.current ||
				layoutTranslateRunningRef.current
			) {
				translationAutoStartedRef.current = true;
				return;
			}
			translationAutoStartedRef.current = true;
			toggleLayoutTranslate();
		}, 250);
		return () => window.clearTimeout(timer);
	}, [
		capabilities.layoutTranslation,
		layoutRawRegions,
		layoutTranslateActive,
		layoutTranslateRunning,
		layoutTranslateStatus,
		liveTranslateJobId,
		toggleLayoutTranslate,
	]);

	useEffect(() => {
		onHandle?.(null);
		return () => onHandle?.(null);
	}, [onHandle]);

	const sourceKey =
		findTranslationSourceId(getTabs(), docId) ?? translationSourceKey(docId);
	const linkSource = useSyncExternalStore(
		subscribeTranslatedLinkSource,
		() => getTranslatedLinkSource(sourceKey),
		() => getTranslatedLinkSource(sourceKey),
	);
	const linkInkByKey = linkSource.inkByKey;
	const glyphRunsByPage = linkSource.glyphRunsByPage;

	const pageMarks = useMemo<PdfPageMarksSlice>(
		() => ({
			...EMPTY_MARKS,
			citationLinks: linkSource.citationLinks,
			textLinks: linkSource.textLinks,
			translatedHighlightsByPage: highlightPaint.translatedByPage,
			highlightQuotesByPage: highlightPaint.quotesByPage,
			selectionCommentDraft: noteDraft,
		}),
		[linkSource, highlightPaint, noteDraft],
	);

	const pageLayout = useMemo<PdfPageLayoutSlice>(
		() => ({
			selectableRegionsByPage: EMPTY_LAYOUT_REGIONS_BY_PAGE,
			rawRegionsByPage,
			layoutOverlayVisible: false,
			layoutTranslateItemsByPage,
			layoutTranslatePageStateByPage,
			linkInkByKey,
			glyphRunsByPage,
		}),
		[
			rawRegionsByPage,
			layoutTranslateItemsByPage,
			layoutTranslatePageStateByPage,
			linkInkByKey,
			glyphRunsByPage,
		],
	);

	const activateCitation = useCallback(
		(link: PdfLinkAnnoObject) => {
			const raw = getLinkDestination(link.target);
			if (!raw) return;
			// PDFium reports user space. EmbedPDF page size is the visible box,
			// so a CropBox that does not start at (0, 0) has to be subtracted
			// before the top-left conversion.
			const destination = destinationInPageBox(raw, linkSource.pageOrigins);
			const page =
				docCapRef.current?.getDocument(docId)?.pages[destination.pageIndex];
			const height = page?.size.height ?? 0;
			const width = page?.size.width ?? 0;
			const anchored = destination.pdfY > 0 || destination.pdfX != null;
			scrollRef.current?.scrollToPage({
				pageNumber: destination.pageIndex + 1,
				behavior: "instant",
				...(anchored && height > 0
					? {
							pageCoordinates: {
								x:
									destination.pdfX != null && width > 0
										? Math.max(0, destination.pdfX)
										: 0,
								y: Math.max(0, height - destination.pdfY),
							},
							alignX: 0,
							alignY: 18,
						}
					: {}),
			});
		},
		[docId, linkSource.pageOrigins],
	);

	// Sibling clears so citation, crossref, and URL tips dismiss each other.
	const clearCrossrefPreviewRef = useRef<() => void>(() => {});
	const clearCitationPreviewRef = useRef<() => void>(() => {});
	const clearLinkUrlTipRef = useRef<() => void>(() => {});

	const {
		citationPreview,
		scheduleCitationHide,
		markCitationHoverEnter,
		clearCitationPreview,
		handleCitationLinkHover,
		hasCitationMatch,
		citationImport,
	} = usePdfCitations({
		engineRef,
		docCapRef,
		docId,
		annotationCap: null,
		hostRef,
		zoomRef,
		vaultPath,
		paperPath: paperRelPath,
		paperAbsPath,
		sourceBytes,
		isRemotePaper,
		importIdentifier,
		onPreviewShow: () => {
			clearCrossrefPreviewRef.current();
			clearLinkUrlTipRef.current();
		},
	});

	const {
		crossrefPreview,
		scheduleCrossrefHide,
		markCrossrefHoverEnter,
		clearCrossrefPreview,
		handleCrossrefLinkHover,
	} = usePdfCrossrefPreview({
		docId,
		hostRef,
		zoomRef,
		paperAbsPath,
		sourceBytes,
		engineRef,
		docCapRef,
		onPreviewShow: () => {
			clearCitationPreviewRef.current();
			clearLinkUrlTipRef.current();
		},
		hasCitationMatch,
	});

	const {
		linkUrlTip,
		scheduleLinkUrlHide,
		markLinkUrlHoverEnter,
		clearLinkUrlTip,
		handleLinkUrlHover: hoverLinkUrl,
	} = usePdfLinkUrlTip();

	clearCitationPreviewRef.current = clearCitationPreview;
	clearCrossrefPreviewRef.current = clearCrossrefPreview;
	clearLinkUrlTipRef.current = clearLinkUrlTip;

	// Hover intent (#1213): cards open only after the pointer rests on a link;
	// crossing link text on the way somewhere else must not flash a card.
	const showLinkPreview = useCallback(
		(payload: {
			link: PdfLinkAnnoObject;
			clientPoint?: ScreenPoint | null;
		}) => {
			handleCitationLinkHover(payload.link, payload.clientPoint);
			handleCrossrefLinkHover(payload.link, payload.clientPoint);
		},
		[handleCitationLinkHover, handleCrossrefLinkHover],
	);
	const hideLinkPreview = useCallback(() => {
		handleCitationLinkHover(null);
		handleCrossrefLinkHover(null);
	}, [handleCitationLinkHover, handleCrossrefLinkHover]);
	const linkHoverIntent = useHoverIntent({
		delayMs: EPHEMERAL_PREVIEW_OPEN_MS,
		onOpen: showLinkPreview,
		onLeave: hideLinkPreview,
	});
	const showUrlTip = useCallback(
		(payload: { url: string; clientPoint: ScreenPoint }) => {
			hoverLinkUrl(payload.url, payload.clientPoint);
		},
		[hoverLinkUrl],
	);
	const hideUrlTip = useCallback(() => {
		hoverLinkUrl(null);
	}, [hoverLinkUrl]);
	const urlHoverIntent = useHoverIntent({
		delayMs: EPHEMERAL_PREVIEW_OPEN_MS,
		onOpen: showUrlTip,
		onLeave: hideUrlTip,
	});

	useEffect(() => {
		return subscribeLinkPreviewDrag(() => {
			linkHoverIntent.cancel();
			urlHoverIntent.cancel();
			clearCitationPreviewRef.current();
			clearCrossrefPreviewRef.current();
			clearLinkUrlTipRef.current();
		});
	}, [linkHoverIntent, urlHoverIntent]);

	// A pending open must not outlive the document or land on the selection menu.
	// biome-ignore lint/correctness/useExhaustiveDependencies: docId is the effect trigger.
	useEffect(() => {
		linkHoverIntent.cancel();
		urlHoverIntent.cancel();
	}, [docId]);

	useEffect(() => {
		if (!evidenceMenuOpen) return;
		linkHoverIntent.cancel();
		urlHoverIntent.cancel();
		clearCitationPreview();
		clearCrossrefPreview();
		clearLinkUrlTip();
	}, [
		evidenceMenuOpen,
		linkHoverIntent,
		urlHoverIntent,
		clearCitationPreview,
		clearCrossrefPreview,
		clearLinkUrlTip,
	]);

	const handleLinkHover = useCallback(
		(link: PdfLinkAnnoObject | null, point?: ScreenPoint | null) => {
			if (evidenceMenuOpen || linkPreviewDragActive()) {
				linkHoverIntent.cancel();
				urlHoverIntent.cancel();
				clearCitationPreview();
				clearCrossrefPreview();
				clearLinkUrlTip();
				return;
			}
			if (!link) {
				linkHoverIntent.leave();
				return;
			}
			linkHoverIntent.enter({ link, clientPoint: point });
		},
		[
			evidenceMenuOpen,
			clearCitationPreview,
			clearCrossrefPreview,
			clearLinkUrlTip,
			linkHoverIntent,
			urlHoverIntent,
		],
	);

	const handleLinkUrlHover = useCallback(
		(url: string | null, point?: ScreenPoint | null) => {
			if (evidenceMenuOpen || linkPreviewDragActive()) {
				urlHoverIntent.cancel();
				clearLinkUrlTip();
				return;
			}
			const trimmed = url?.trim() ?? "";
			if (!trimmed || !point) {
				urlHoverIntent.leave();
				return;
			}
			urlHoverIntent.enter({ url: trimmed, clientPoint: point });
		},
		[evidenceMenuOpen, clearLinkUrlTip, urlHoverIntent],
	);

	const handleCitationImportOpenChange = useCallback(
		(open: boolean) => {
			if (open) markCitationHoverEnter();
			else scheduleCitationHide();
		},
		[markCitationHoverEnter, scheduleCitationHide],
	);
	const citationImportMenu = useMemo(
		() =>
			citationImport
				? {
						folders: citationImport.folders,
						lastImportParentDir: citationImport.lastImportParentDir,
						importingId: citationImport.importingId,
						onImport: citationImport.importCitation,
						onOpenChange: handleCitationImportOpenChange,
						remotePaper: isRemotePaper,
					}
				: undefined,
		[citationImport, handleCitationImportOpenChange, isRemotePaper],
	);

	const pageMode = useMemo<PdfPageModeSlice>(
		() =>
			translatedEvidenceEnabled
				? { ...PAGE_MODE, translatedEvidenceSelection: true }
				: PAGE_MODE,
		[translatedEvidenceEnabled],
	);
	const pageHandlers = useMemo<PdfPageHandlers>(
		() => ({
			...EMPTY_HANDLERS,
			onTogglePageLayoutTranslate: togglePageLayoutTranslate,
			onRetryPageLayoutTranslate: retryPageLayoutTranslate,
			onCitationActivate: activateCitation,
			onTextLinkActivate: openExternalUrl,
			onCitationHover: handleLinkHover,
			onLinkUrlHover: handleLinkUrlHover,
			onCommitSelectionComment: commitNote,
			onDismissSelectionComment: dismissNote,
		}),
		[
			togglePageLayoutTranslate,
			retryPageLayoutTranslate,
			activateCitation,
			handleLinkHover,
			handleLinkUrlHover,
			commitNote,
			dismissNote,
		],
	);

	const renderPage = useCallback(
		({ pageIndex, width, height, rotatedWidth, rotatedHeight }: PageLayout) => (
			<PdfPageLayers
				docId={docId}
				pageIndex={pageIndex}
				reflowDocumentId={reflowDisplayDocument?.id}
				reflowDocumentVersion={reflowDisplayDocument?.version}
				searchDocumentId={searchDocumentId}
				searchScale={searchScale}
				width={rotatedWidth || width}
				height={rotatedHeight || height}
				tone={pdfTone}
				zoomRef={zoomRef}
				annotationCap={null}
				inkEditor={inkEditor}
				inkOverReflow
				marks={pageMarks}
				layout={pageLayout}
				mode={pageMode}
				handlers={pageHandlers}
			/>
		),
		[
			docId,
			pdfTone,
			zoomRef,
			inkEditor,
			pageMarks,
			pageLayout,
			pageHandlers,
			pageMode,
			reflowDisplayDocument,
			searchDocumentId,
			searchScale,
		],
	);

	return (
		<div
			ref={hostRef}
			data-pdf-root
			className="content-arrive relative flex h-full min-h-0 w-full select-none flex-col"
		>
			<DockviewViewport
				documentId={docId}
				hostRef={hostRef}
				className="min-h-0 min-w-0 flex-1"
			>
				<ActiveCardScrollSync
					active={evidenceMenuOpen}
					onScroll={translatedEvidence.rePlaceSelectionMenu}
				/>
				<PdfHistoryKeyHandler
					active={isActive}
					hostRef={hostRef}
					onReplay={highlights.replayAnnotationHistory}
				/>
				<WheelZoomHandler docId={docId} />
				<InkInputBridge
					editor={inkEditor}
					documentId={docId}
					enabled={inkEditable && isActive}
					docCapRef={docCapRef}
				/>
				<KeyboardScrollHandler
					documentId={docId}
					active={isActive}
					hostRef={hostRef}
				/>
				<PdfVimKeyHandler
					documentId={docId}
					active={isActive}
					hostRef={hostRef}
				/>
				<PanDragHandler
					active={isActive}
					hostRef={hostRef}
					allowLeftDrag={false}
				/>
				<ZoomGestureWrapper documentId={docId} enableWheel={false}>
					<GlobalPointerProvider documentId={docId}>
						<PdfScroller documentId={docId} renderPage={renderPage} />
					</GlobalPointerProvider>
				</ZoomGestureWrapper>
			</DockviewViewport>
			<PdfFindBar
				open={findOpen}
				inputRef={findInputRef}
				query={findQuery}
				onQueryChange={setFindQuery}
				total={findTotal}
				activeResultIndex={findActiveIndex}
				onFindNext={handleFindNext}
				onFindPrev={handleFindPrev}
				onClose={handleCloseFind}
			/>
			<PdfTranslateProgressChip
				layoutTranslateRunning={layoutTranslateRunning}
				layoutTranslatePreparing={layoutTranslatePreparing}
				layoutTranslateWaiting={layoutTranslateWaiting}
				layoutTranslateQueued={layoutTranslateQueued}
				layoutTranslateProgress={layoutTranslateProgress}
				layoutTranslatePageCounts={layoutTranslatePageCounts}
				layoutTranslatePartial={layoutTranslatePartial}
				// 对照 mode hands the export control to this pane: it owns the
				// generated pages the reader toolbar would otherwise export.
				onExport={reflowReady ? exportTranslatedPdf : undefined}
				// Cancel the paper's job outright, like the reader toolbar's stop:
				// this pane may not have attached yet when the click lands. A settled
				// job can still be generating pages here, so stop that build too.
				onStop={handleStopTranslate}
			/>
			{typeof document !== "undefined"
				? createPortal(
						<>
							{translatedEvidence.menu && (
								<SelectionMenu {...translatedEvidence.menu} />
							)}
							{translatedEvidenceEnabled && ask.ask && (
								<AskPopover
									thread={ask.ask.thread}
									screen={ask.ask.screen}
									streaming={ask.streaming}
									error={ask.askError}
									onSend={ask.sendAskQuestion}
									onResend={ask.resendAskQuestion}
									onHide={ask.hideAsk}
									onDelete={ask.deleteAsk}
									onStop={ask.stopAskStreaming}
								/>
							)}
							{citationPreview ? (
								<PdfCitationPreview
									screen={citationPreview.screen}
									matched={citationPreview.matched}
									importMenu={citationImportMenu}
									onPointerEnter={markCitationHoverEnter}
									onPointerLeave={scheduleCitationHide}
								/>
							) : null}
							{crossrefPreview ? (
								<PdfCrossrefPreview
									screen={crossrefPreview.screen}
									kind={crossrefPreview.kind}
									page={crossrefPreview.page}
									image={crossrefPreview.image}
									targetSize={crossrefPreview.targetSize}
									onPointerEnter={markCrossrefHoverEnter}
									onPointerLeave={scheduleCrossrefHide}
								/>
							) : null}
							{linkUrlTip ? (
								<PdfLinkPreviewCard
									key={linkUrlTip.url}
									url={linkUrlTip.url}
									screen={linkUrlTip.screen}
									onPointerEnter={markLinkUrlHoverEnter}
									onPointerLeave={scheduleLinkUrlHide}
								/>
							) : null}
						</>,
						document.body,
					)
				: null}
		</div>
	);
}
