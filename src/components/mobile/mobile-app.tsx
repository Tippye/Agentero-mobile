import { isTauri } from "@tauri-apps/api/core";
import {
	ArrowLeft,
	Info,
	LoaderCircle,
	Menu,
	PanelLeft,
	Plus,
	RefreshCw,
	Search,
	WandSparkles,
} from "lucide-react";
import {
	useCallback,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import agenteroLogo from "@/assets/agentero-logo.svg";
import { AgentPanel } from "@/components/agent/agent-panel";
import { useBridgeStatus } from "@/components/mobile/hooks/use-bridge-status";
import { useMobileAgents } from "@/components/mobile/hooks/use-mobile-agents";
import { useMobileBackLayer } from "@/components/mobile/hooks/use-mobile-back-layer";
import { useMobileExternalPdfs } from "@/components/mobile/hooks/use-mobile-external-pdfs";
import { useMobilePaperSync } from "@/components/mobile/hooks/use-mobile-paper-sync";
import { useMobilePapers } from "@/components/mobile/hooks/use-mobile-papers";
import { useMobileRoute } from "@/components/mobile/hooks/use-mobile-route";
import { useMobileSystemBack } from "@/components/mobile/hooks/use-mobile-system-back";
import { useMobileVault } from "@/components/mobile/hooks/use-mobile-vault";
import { useMobileWorkspaceWidth } from "@/components/mobile/hooks/use-mobile-workspace-width";
import { usePairOfferLinks } from "@/components/mobile/hooks/use-pair-offer-links";
import { MobileAgentPage } from "@/components/mobile/mobile-agent-page";
import { MobileBridgeReset } from "@/components/mobile/mobile-bridge-reset";
import {
	EdgeSwipeBack,
	useHorizontalSwipe,
} from "@/components/mobile/mobile-gestures";
import { MobileHeader } from "@/components/mobile/mobile-header";
import {
	MobileAgentToolbar,
	MobileReaderModeToggle,
} from "@/components/mobile/mobile-header-actions";
import { MobileLibraryNavigation } from "@/components/mobile/mobile-library-navigation";
import { MobileLibraryPage } from "@/components/mobile/mobile-library-page";
import { MobileNav, type MobileTab } from "@/components/mobile/mobile-nav";
import { MobilePairing } from "@/components/mobile/mobile-pairing";
import { MobilePaneGroup } from "@/components/mobile/mobile-pane-group";
import { MobilePaperImport } from "@/components/mobile/mobile-paper-import";
import { MobilePaperInfo } from "@/components/mobile/mobile-paper-info";
import { MobilePlazaControls } from "@/components/mobile/mobile-plaza-controls";
import { MobilePlazaPage } from "@/components/mobile/mobile-plaza-page";
import { MobileReaderPage } from "@/components/mobile/mobile-reader-page";
import { MobileSettingsPage } from "@/components/mobile/mobile-settings-page";
import { MobileSidebar } from "@/components/mobile/mobile-sidebar";
import { MobileTaskIndicator } from "@/components/mobile/mobile-task-indicator";
import type { MobileReaderMode } from "@/components/mobile/types";
import { SyncConflictDialog } from "@/components/settings/panes/sync-conflict-dialog";
import { useSyncConflictReview } from "@/components/settings/panes/use-sync-conflict-review";
import { PaneHeader } from "@/components/shell/pane-header";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger,
} from "@/components/ui/tooltip";
import { errorText } from "@/lib/core/error";
import { notifyError } from "@/lib/core/notify";
import { cn } from "@/lib/core/utils";
import { dismissMobileBackLayer } from "@/lib/mobile/back";
import {
	importMobilePdfs,
	pickAndImportMobilePdfs,
} from "@/lib/mobile/import-pdf";
import {
	filterMobilePapers,
	openMobilePlazaUrl,
} from "@/lib/mobile/navigation";
import type { MobilePaperRegionContext } from "@/lib/mobile/paper-context";
import {
	loadMobilePaperPdfAttachment,
	mobilePaperPaths,
} from "@/lib/mobile/paper-files";
import { useMobilePdfProcessingRuntime } from "@/lib/mobile/pdf-processing";
import type { MobileWindowRoute } from "@/lib/mobile/route";
import {
	listenMobileSettings,
	type MobileSettingsSection,
} from "@/lib/mobile/settings";
import {
	canLeaveMobilePaper,
	type MobileWorkspacePreset,
	mobileWorkspaceLayout,
} from "@/lib/mobile/workspace";
import { deletePaths } from "@/lib/paper/api";
import type { PaperMetadata } from "@/lib/paper/types";
import {
	closeFigureLightbox,
	figureLightboxStore,
} from "@/lib/pdf/figure-lightbox-store";
import type { PdfReadingSnapshot } from "@/lib/pdf/reading-position";
import { PLAZA_SOURCES } from "@/lib/plaza/sources";

export default function MobileApp() {
	useMobilePdfProcessingRuntime();
	const { t } = useTranslation(["mobile", "sidebar", "shortcuts"]);
	const [tab, setTab] = useState<MobileTab>("library");
	const [libraryQuery, setLibraryQuery] = useState("");
	const [selectedTag, setSelectedTag] = useState<string | null>(null);
	const [paperImportOpen, setPaperImportOpen] = useState(false);
	const [importReference, setImportReference] = useState("");
	const [infoPaper, setInfoPaper] = useState<PaperMetadata | null>(null);
	const [paperInfoOpen, setPaperInfoOpen] = useState(false);
	const [sidebarOpen, setSidebarOpen] = useState(false);
	const [paperListCollapsed, setPaperListCollapsed] = useState(false);
	const [plazaPath, setPlazaPath] = useState(PLAZA_SOURCES[0].path);
	const [plazaReloadKey, setPlazaReloadKey] = useState(0);
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [settingsSection, setSettingsSection] =
		useState<MobileSettingsSection>("appearance");
	const [pairingRequested, setPairingRequested] = useState(false);
	const [pendingOffer, setPendingOffer] = useState<string | null>(null);
	const [selectedPaper, setSelectedPaper] = useState<PaperMetadata | null>(
		null,
	);
	const [readerMode, setReaderMode] = useState<MobileReaderMode>("pdf");
	const [readingSnapshot, setReadingSnapshot] = useState<{
		paperId: string;
		value: PdfReadingSnapshot;
	} | null>(null);
	const [workspacePreset, setWorkspacePreset] =
		useState<MobileWorkspacePreset>("read");
	const figureLightboxOpen = useStore(figureLightboxStore, (state) =>
		Boolean(state.request),
	);
	const [inkDirty, setInkDirty] = useState(false);
	const [inkMode, setInkMode] = useState(false);
	const { ref: workspaceRef, widthClass } =
		useMobileWorkspaceWidth<HTMLDivElement>();
	const [agentSessionId, setAgentSessionId] = useState<string | null>(null);
	const [historyOpen, setHistoryOpen] = useState(false);
	const [agentProvider, setAgentProvider] = useState("ds-harness-remote");
	const [regionContext, setRegionContext] =
		useState<MobilePaperRegionContext | null>(null);
	const regionContextRef = useRef<MobilePaperRegionContext | null>(null);
	const [notesDirty, setNotesDirty] = useState(false);
	const [importing, setImporting] = useState(false);
	const fileInput = useRef<HTMLInputElement>(null);
	const layout = mobileWorkspaceLayout({
		width: widthClass,
		preset: workspacePreset,
		tab,
		hasPaper: selectedPaper !== null,
	});
	const {
		vaultPath,
		loading: vaultLoading,
		retry: retryVault,
	} = useMobileVault();

	const latestRegionTarget = useRef({
		vaultPath,
		paperId: selectedPaper?.id,
		agentProvider,
		widthClass,
		inkDirty,
		blocked:
			pairingRequested ||
			settingsOpen ||
			sidebarOpen ||
			paperImportOpen ||
			paperInfoOpen,
	});
	latestRegionTarget.current = {
		vaultPath,
		paperId: selectedPaper?.id,
		agentProvider,
		widthClass,
		inkDirty,
		blocked:
			pairingRequested ||
			settingsOpen ||
			sidebarOpen ||
			paperImportOpen ||
			paperInfoOpen,
	};

	const { status, setStatus, pairPending, disconnect, resume, resetAvailable } =
		useBridgeStatus(agentProvider === "agentero-bridge" || pairingRequested);
	const paired = status.paired;
	const connected = status.connected;
	const {
		papers,
		loading: papersLoading,
		refresh,
	} = useMobilePapers({ vaultPath });
	const importAdmission = useRef(false);
	const importTarget = useRef({ mounted: false, vaultPath, epoch: 0, refresh });
	useLayoutEffect(() => {
		const target = importTarget.current;
		target.mounted = true;
		target.vaultPath = vaultPath;
		target.epoch++;
		// A committed new target owns its busy UI; old operations keep admission only.
		setImporting(false);
		return () => {
			target.mounted = false;
			target.epoch++;
		};
	}, [vaultPath]);
	useLayoutEffect(() => {
		importTarget.current.refresh = refresh;
	}, [refresh]);
	const externalPdfWake = useMobileExternalPdfs({
		vaultPath,
		admission: importAdmission,
		target: importTarget,
		setImporting,
	});
	const visiblePapers = useMemo(
		() => filterMobilePapers(papers, libraryQuery, selectedTag),
		[papers, libraryQuery, selectedTag],
	);
	const showImport = useCallback((reference = "") => {
		setImportReference(reference);
		setPaperImportOpen(true);
	}, []);
	const showPaperInfo = useCallback((paper: PaperMetadata) => {
		setInfoPaper(paper);
		setPaperInfoOpen(true);
	}, []);
	const updatePaperInfo = useCallback(
		async (paper: PaperMetadata) => {
			setInfoPaper(paper);
			setSelectedPaper((current) =>
				current?.id === paper.id ? paper : current,
			);
			await refresh();
		},
		[refresh],
	);
	const restoreRoute = useCallback(
		(route: MobileWindowRoute) => {
			const paper = papers.find((paper) => paper.id === route.paperId) ?? null;
			setSelectedPaper(paper);
			// Restoring a document never opens auxiliary panes without a new user
			// action; a stale notes mode used to leave an empty right pane while the
			// preset toggle still showed Read.
			setWorkspacePreset("read");
			setReaderMode("pdf");
			setTab(paper ? "library" : route.tab);
			setReadingSnapshot(
				paper && route.pdf && typeof route.pdf.zoom === "number"
					? {
							paperId: paper.id,
							value: { page: route.pdf.page, zoom: route.pdf.zoom },
						}
					: null,
			);
		},
		[papers],
	);
	const { markInteraction } = useMobileRoute({
		vaultPath,
		loading: vaultLoading || papersLoading,
		route: {
			paperId: selectedPaper?.id ?? null,
			tab,
			readerMode,
			preset: workspacePreset,
			pdf:
				selectedPaper && readingSnapshot?.paperId === selectedPaper.id
					? readingSnapshot.value
					: null,
		},
		onRestore: restoreRoute,
	});
	const observeReadingSnapshot = useCallback(
		(paperId: string, value: PdfReadingSnapshot) => {
			if (latestRegionTarget.current.paperId !== paperId) return;
			setReadingSnapshot({ paperId, value });
		},
		[],
	);
	const selectReaderMode = useCallback(
		(mode: MobileReaderMode) => {
			markInteraction();
			setReaderMode(mode);
		},
		[markInteraction],
	);
	const rescanAfterSync = useCallback(() => refresh(true), [refresh]);
	const refreshAfterDownload = useCallback(() => refresh(), [refresh]);
	const paperSync = useMobilePaperSync({
		vaultPath,
		papers,
		active: !settingsOpen,
		onChanged: refreshAfterDownload,
	});
	const syncConflictReview = useSyncConflictReview({
		vaultPath,
		pending: paperSync.pendingConflicts,
		refresh: paperSync.refresh,
		onSynced: rescanAfterSync,
	});
	const reviewSyncConflicts = () => {
		syncConflictReview.reopen();
		void paperSync
			.refresh()
			.catch((error) =>
				notifyError(errorText(error), { id: `sync:${vaultPath}` }),
			);
	};
	const {
		agents,
		loading: agentsLoading,
		selectedAgent,
		selectedAgentId,
		selectAgent,
	} = useMobileAgents({
		paired,
		connected,
		agentVisible: layout.agent && agentProvider === "agentero-bridge",
	});

	const acceptPairOffer = useCallback((offerUrl: string) => {
		setAgentProvider("agentero-bridge");
		setPendingOffer(offerUrl);
		setPairingRequested(true);
	}, []);
	usePairOfferLinks(acceptPairOffer);

	useEffect(
		() =>
			listenMobileSettings((section) => {
				setSettingsSection(section);
				setSettingsOpen(true);
			}),
		[],
	);

	useEffect(() => {
		if (paired) return;
		setAgentSessionId(null);
	}, [paired]);

	const canLeaveReader = useCallback(() => {
		if (canLeaveMobilePaper(notesDirty, inkDirty)) return true;
		notifyError(t("reader.unsavedChanges"));
		return false;
	}, [notesDirty, inkDirty, t]);
	const closeReader = useCallback(() => {
		if (!canLeaveReader()) return;
		markInteraction();
		setSelectedPaper(null);
		setReadingSnapshot(null);
		setInkMode(false);
		setReaderMode("pdf");
	}, [canLeaveReader, markInteraction]);
	const selectPaper = useCallback(
		(paper: PaperMetadata) => {
			if (
				selectedPaper?.path === paper.path &&
				selectedPaper?.id === paper.id
			) {
				markInteraction();
				setWorkspacePreset("read");
				setTab("library");
				return;
			}
			if (!canLeaveReader()) return;
			markInteraction();
			setInkMode(false);
			setReadingSnapshot(null);
			setWorkspacePreset("read");
			setReaderMode("pdf");
			setTab("library");
			setSelectedPaper(paper);
		},
		[selectedPaper, canLeaveReader, markInteraction],
	);
	const openPlazaUrl = useCallback(
		(url: string) => {
			void openMobilePlazaUrl(url).catch((error) =>
				notifyError(t("plaza.openFailed"), { description: errorText(error) }),
			);
		},
		[t],
	);
	/** Permanently delete the paper folder, then refresh; never while a draft is unsaved. */
	const deletePaper = useCallback(
		async (paper: PaperMetadata) => {
			if (!vaultPath) return;
			const open =
				selectedPaper?.id === paper.id && selectedPaper?.path === paper.path;
			if (open && !canLeaveReader()) return;
			try {
				const { paperRelPath } = mobilePaperPaths(vaultPath, paper);
				await deletePaths(vaultPath, [paperRelPath]);
				if (open) closeReader();
				await refresh(true);
			} catch (error) {
				notifyError(errorText(error));
				throw error;
			}
		},
		[vaultPath, selectedPaper, canLeaveReader, closeReader, refresh],
	);
	const selectPreset = useCallback(
		(preset: MobileWorkspacePreset) => {
			if (inkDirty && widthClass === "narrow" && preset !== "read") {
				notifyError(t("reader.unsavedChanges"));
				return;
			}
			markInteraction();
			setWorkspacePreset(preset);
			setReaderMode(preset === "compare" ? "notes" : "pdf");
		},
		[inkDirty, widthClass, t, markInteraction],
	);
	const selectTab = useCallback(
		(next: MobileTab) => {
			if (inkDirty && next !== "library") {
				notifyError(t("reader.unsavedChanges"));
				return;
			}
			markInteraction();
			if (next === "library") setWorkspacePreset("read");
			setTab(next);
		},
		[inkDirty, t, markInteraction],
	);

	const onSystemBack = useCallback(() => {
		if (figureLightboxOpen) {
			closeFigureLightbox();
			return;
		}
		markInteraction();
		if (dismissMobileBackLayer()) return;
		if (paperInfoOpen) {
			setPaperInfoOpen(false);
			return;
		}
		if (paperImportOpen) {
			setPaperImportOpen(false);
			return;
		}
		if (settingsOpen) {
			setSettingsOpen(false);
			return;
		}
		if (pairingRequested) {
			setPendingOffer(null);
			setPairingRequested(false);
			return;
		}
		if (sidebarOpen) {
			setSidebarOpen(false);
			return;
		}
		if (historyOpen && layout.agent) {
			setHistoryOpen(false);
			return;
		}
		if (tab !== "library") {
			selectTab("library");
			return;
		}
		if (selectedPaper && workspacePreset !== "read") {
			selectPreset("read");
			return;
		}
		if (selectedPaper) {
			closeReader();
			return;
		}
		if (notesDirty || inkDirty) canLeaveReader();
	}, [
		figureLightboxOpen,
		markInteraction,
		paperInfoOpen,
		paperImportOpen,
		settingsOpen,
		pairingRequested,
		sidebarOpen,
		historyOpen,
		layout.agent,
		tab,
		selectTab,
		selectedPaper,
		workspacePreset,
		selectPreset,
		closeReader,
		notesDirty,
		inkDirty,
		canLeaveReader,
	]);
	const ownsBack =
		paperInfoOpen ||
		paperImportOpen ||
		settingsOpen ||
		pairingRequested ||
		sidebarOpen ||
		(historyOpen && layout.agent) ||
		tab !== "library" ||
		selectedPaper !== null ||
		notesDirty ||
		inkDirty;
	const rootBackLayer = useMobileBackLayer(!ownsBack);
	const androidSystemBack = useMobileSystemBack({
		enabled: ownsBack || rootBackLayer,
		onBack: onSystemBack,
	});

	const acceptRegionPrompt = useCallback(
		(context: MobilePaperRegionContext) => {
			const current = latestRegionTarget.current;
			if (
				!vaultPath ||
				current.vaultPath !== vaultPath ||
				current.paperId !== context.paperId ||
				current.agentProvider !== "ds-harness-remote" ||
				current.inkDirty ||
				current.blocked
			) {
				notifyError(t("paperContext.staleCapture"));
				return;
			}
			if (regionContextRef.current) {
				notifyError(t("paperContext.pendingAlready"));
				return;
			}
			markInteraction();
			regionContextRef.current = context;
			setRegionContext(context);
			setReaderMode("pdf");
			setWorkspacePreset("discuss");
			setTab(current.widthClass === "narrow" ? "agent" : "library");
		},
		[vaultPath, t, markInteraction],
	);
	const handleRegionContext = useCallback((id: string) => {
		if (regionContextRef.current?.id !== id) return;
		regionContextRef.current = null;
		setRegionContext(null);
	}, []);

	const importFiles = async (files?: File[]) => {
		const target = importTarget.current;
		if (
			!target.mounted ||
			!target.vaultPath ||
			importAdmission.current ||
			files?.length === 0
		)
			return;
		const importingVault = target.vaultPath;
		const epoch = target.epoch;
		const current = () =>
			target.mounted &&
			target.epoch === epoch &&
			target.vaultPath === importingVault;
		// React state alone cannot reject a second click in the same event turn.
		importAdmission.current = true;
		setImporting(true);
		let reported = false;
		const report = (message: string) => {
			if (!reported && current()) {
				reported = true;
				notifyError(message);
			}
		};
		try {
			const result = await (files
				? importMobilePdfs(importingVault, files)
				: pickAndImportMobilePdfs(importingVault));
			if (result.errors.length) report(result.errors.join("\n"));
		} catch (error) {
			report(errorText(error));
		} finally {
			try {
				if (current()) await target.refresh();
			} catch (error) {
				report(errorText(error));
			} finally {
				importAdmission.current = false;
				if (current()) setImporting(false);
				externalPdfWake();
			}
		}
	};

	const handleSelectAgent = useCallback(
		(agentId: string) => {
			if (agentId === selectedAgentId) return;
			selectAgent(agentId);
			setAgentSessionId(null);
		},
		[selectedAgentId, selectAgent],
	);

	const swipeHandlers = useHorizontalSwipe(
		({ dx, dy, fromEdge, durationMs }) => {
			if (sidebarOpen || selectedPaper) return;
			if (durationMs > 500 || Math.abs(dy) > 60) return;
			const horizontal = Math.abs(dx) > Math.abs(dy) * 1.25;
			if (!horizontal) return;
			if (dx > 60 && fromEdge) {
				setSidebarOpen(true);
				return;
			}
			if (dx < -60 && tab === "library") {
				selectTab("agent");
				return;
			}
			if (dx > 60 && tab === "agent") {
				selectTab("library");
			}
		},
		androidSystemBack ||
			inkMode ||
			inkDirty ||
			pairingRequested ||
			settingsOpen ||
			paperImportOpen ||
			paperInfoOpen,
	);

	const inReader = tab === "library" && selectedPaper !== null;
	const fetchMissingPaper = paperSync.fetchMissing;
	const remotePaperSource = useMemo(() => {
		if (!vaultPath || !selectedPaper) return null;
		return {
			key: JSON.stringify([vaultPath, selectedPaper.id, selectedPaper.path]),
			paper: selectedPaper,
			loadPdf: (maxBytes: number) =>
				loadMobilePaperPdfAttachment(vaultPath, selectedPaper, maxBytes, () =>
					fetchMissingPaper(selectedPaper),
				),
		};
	}, [vaultPath, selectedPaper, fetchMissingPaper]);

	return (
		<div
			ref={workspaceRef}
			data-workspace-width={widthClass}
			className="mobile-shell mobile-safe-area flex h-dvh min-h-0 overflow-hidden bg-background text-foreground"
			{...swipeHandlers}
		>
			<input
				ref={fileInput}
				type="file"
				accept="application/pdf,.pdf"
				multiple
				hidden
				aria-label={t("library.importPdf")}
				onChange={(event) => {
					const files = Array.from(event.target.files ?? []);
					event.target.value = "";
					void importFiles(files);
				}}
			/>
			<aside
				hidden={widthClass !== "wide"}
				style={{ display: widthClass === "wide" ? undefined : "none" }}
				className="flex w-20 shrink-0 flex-col items-center border-r bg-muted/25 py-6"
			>
				<button
					type="button"
					className="grid size-12 place-items-center rounded-lg focus-visible:ring-2 focus-visible:ring-ring"
					aria-label={t("settings.menu")}
					onClick={() => setSidebarOpen(true)}
				>
					<MobileBrand />
				</button>
				<MobileNav
					tab={tab}
					onTab={selectTab}
					agentTemplate={selectedAgent?.template}
				/>
			</aside>
			<main className="relative flex min-h-0 min-w-0 flex-1 flex-col">
				<MobileHeader
					title={inReader ? selectedPaper.title : t(`tabs.${tab}`)}
					status={status}
					statusLabel={
						tab === "library"
							? t("settings.localVault")
							: connected
								? t("settings.connected")
								: t("settings.offline")
					}
					brand={<MobileBrand />}
					brandButtonLabel={t("settings.menu")}
					onBrandClick={() => setSidebarOpen(true)}
					showBrand={!inReader}
					wide={widthClass === "wide"}
					showStatus={layout.agent && agentProvider === "agentero-bridge"}
					leading={
						inReader ? (
							<div className="flex shrink-0 items-center">
								<Button
									type="button"
									variant="ghost"
									size="icon-sm"
									className="size-11"
									aria-label={t("reader.back")}
									onClick={closeReader}
								>
									<ArrowLeft className="size-4" />
								</Button>
								{widthClass === "wide" ? (
									<TooltipProvider delayDuration={250}>
										<Tooltip>
											<TooltipTrigger asChild>
												<Button
													type="button"
													variant="ghost"
													size="icon-sm"
													className="size-11"
													aria-label={t(
														paperListCollapsed
															? "workspace.showPaperList"
															: "workspace.hidePaperList",
													)}
													aria-controls="mobile-workspace-library"
													aria-expanded={!paperListCollapsed}
													onClick={() => {
														markInteraction();
														setPaperListCollapsed((collapsed) => !collapsed);
													}}
												>
													<PanelLeft className="size-4" />
												</Button>
											</TooltipTrigger>
											<TooltipContent side="bottom">
												{t(
													paperListCollapsed
														? "workspace.showPaperList"
														: "workspace.hidePaperList",
												)}
											</TooltipContent>
										</Tooltip>
									</TooltipProvider>
								) : (
									<Button
										type="button"
										variant="ghost"
										size="icon-sm"
										className="size-11"
										aria-label={t("settings.menu")}
										onClick={() => setSidebarOpen(true)}
									>
										<Menu className="size-4" />
									</Button>
								)}
							</div>
						) : undefined
					}
					trailing={
						inReader ? (
							<div className="flex min-w-0 items-center gap-1">
								<MobileReaderModeToggle
									mode={readerMode}
									onChange={selectReaderMode}
									preset={workspacePreset}
									widthClass={widthClass}
									onPresetChange={selectPreset}
								/>
								<Button
									type="button"
									variant="ghost"
									size="icon-sm"
									className="size-11"
									aria-label={t("sidebar:paperInfo.info")}
									onClick={() => showPaperInfo(selectedPaper)}
								>
									<Info className="size-4" />
								</Button>
							</div>
						) : tab === "plaza" ? (
							<MobilePlazaControls
								path={plazaPath}
								onPathChange={setPlazaPath}
								onReload={() => setPlazaReloadKey((key) => key + 1)}
								onOpenUrl={openPlazaUrl}
							/>
						) : tab === "library" ? (
							<div className="flex shrink-0 items-center gap-1">
								<Button
									type="button"
									variant="ghost"
									size="icon"
									className="size-12"
									disabled={!vaultPath}
									aria-label={t("shortcuts:labels.magicWand")}
									onClick={() => showImport()}
								>
									<WandSparkles className="size-4" />
								</Button>
								<Button
									type="button"
									variant="ghost"
									size="icon"
									className="size-12"
									disabled={!vaultPath || papersLoading}
									aria-label={t("library.refresh")}
									onClick={() => void refresh(true)}
								>
									<RefreshCw
										className={cn("size-4", papersLoading && "animate-spin")}
									/>
								</Button>
								<Button
									type="button"
									variant="ghost"
									size="icon"
									className="size-12"
									disabled={!vaultPath || importing}
									aria-label={t("library.importPdf")}
									onClick={() => {
										if (isTauri()) void importFiles();
										else fileInput.current?.click();
									}}
								>
									{importing ? (
										<LoaderCircle className="size-4 animate-spin" />
									) : (
										<Plus className="size-5" />
									)}
								</Button>
								<div
									className={cn(
										"relative min-w-0",
										widthClass === "narrow" ? "w-24" : "w-40",
									)}
								>
									<Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
									<Input
										value={libraryQuery}
										onChange={(event) => setLibraryQuery(event.target.value)}
										placeholder={t("library.search")}
										aria-label={t("library.search")}
										className={cn(
											"h-12 w-full pl-8",
											widthClass === "narrow" ? "text-base" : "text-sm",
										)}
									/>
								</div>
							</div>
						) : undefined
					}
				/>
				<div className="min-h-0 flex-1 overflow-hidden">
					<MobilePaneGroup
						id="mobile-workspace"
						layoutKey={`${widthClass}:${workspacePreset}:${tab}:${Boolean(selectedPaper)}:${paperListCollapsed}`}
						panes={[
							{
								id: "plaza",
								visible: tab === "plaza",
								weight: 1,
								content: (
									<MobilePlazaPage
										path={plazaPath}
										reloadKey={plazaReloadKey}
										active={
											tab === "plaza" &&
											!sidebarOpen &&
											!paperImportOpen &&
											!paperInfoOpen &&
											!pairingRequested &&
											!settingsOpen
										}
										vaultPath={vaultPath}
										onImportPaper={showImport}
									/>
								),
							},
							{
								id: "library",
								visible:
									layout.library && (!selectedPaper || !paperListCollapsed),
								weight: 1.3,
								separatorLabel: t("workspace.resizeLibrary"),
								content: vaultPath ? (
									<div className="flex h-full min-h-0 flex-col">
										<div
											hidden={!selectedPaper || widthClass !== "wide"}
											className="shrink-0 border-b p-3"
										>
											<Input
												value={libraryQuery}
												onChange={(event) =>
													setLibraryQuery(event.target.value)
												}
												aria-label={t("library.search")}
												placeholder={t("library.search")}
												className="h-11"
											/>
										</div>
										<div className="min-h-0 flex-1">
											<MobileLibraryPage
												onDeletePaper={deletePaper}
												papers={visiblePapers}
												loading={papersLoading}
												selected={selectedPaper}
												onSelect={selectPaper}
												query=""
												syncStates={paperSync.states}
												onPinChange={paperSync.setPinned}
												syncProgress={paperSync.progressText}
											/>
										</div>
									</div>
								) : (
									<div
										className="grid h-full place-items-center p-6"
										role="status"
									>
										{vaultLoading ? (
											<div className="flex items-center gap-2 text-muted-foreground">
												<LoaderCircle className="size-5 animate-spin" />
												{t("local.opening")}
											</div>
										) : (
											<Button
												variant="outline"
												className="min-h-12"
												onClick={retryVault}
											>
												{t("local.retry")}
											</Button>
										)}
									</div>
								),
							},
							{
								id: "reader",
								visible: layout.reader,
								weight: workspacePreset === "discuss" ? 2 : 3.7,
								separatorLabel: t("workspace.resizeDiscussion"),
								content: (
									<EdgeSwipeBack
										onBack={closeReader}
										disabled={
											androidSystemBack ||
											widthClass === "wide" ||
											notesDirty ||
											inkDirty ||
											inkMode ||
											pairingRequested ||
											settingsOpen ||
											sidebarOpen ||
											paperImportOpen ||
											paperInfoOpen
										}
									>
										{vaultPath && selectedPaper ? (
											<MobileReaderPage
												key={selectedPaper.path ?? selectedPaper.id}
												paper={selectedPaper}
												vaultPath={vaultPath}
												mode={readerMode}
												preset={workspacePreset}
												widthClass={widthClass}
												active={
													layout.reader &&
													!pairingRequested &&
													!settingsOpen &&
													!sidebarOpen &&
													!paperImportOpen &&
													!paperInfoOpen
												}
												initialReadingSnapshot={
													readingSnapshot?.paperId === selectedPaper.id
														? readingSnapshot.value
														: null
												}
												onReadingSnapshotChange={observeReadingSnapshot}
												onNotesDirtyChange={setNotesDirty}
												onInkDirtyChange={setInkDirty}
												onInkModeChange={setInkMode}
												fetchMissing={paperSync.fetchMissing}
												onRegionPrompt={
													agentProvider === "ds-harness-remote"
														? acceptRegionPrompt
														: undefined
												}
												onTranslationPaneChange={(open) => {
													setWorkspacePreset(open ? "compare" : "read");
													setReaderMode(open ? "notes" : "pdf");
												}}
											/>
										) : null}
									</EdgeSwipeBack>
								),
							},
							{
								id: "agent",
								visible: layout.agent,
								weight: 1.7,
								content: (
									<div className="flex h-full min-h-0 flex-col">
										<div
											hidden={agentProvider !== "ds-harness-remote"}
											style={{
												display:
													agentProvider !== "ds-harness-remote"
														? "none"
														: undefined,
											}}
											className="min-h-0 flex-1"
										>
											<AgentPanel
												remoteOnly
												vaultPath={vaultPath}
												remotePaperSource={remotePaperSource}
												backendControl={
													<MobileAgentToolbar
														agents={agents}
														loading={agentsLoading}
														selectedAgentId={selectedAgentId}
														remoteSelected
														onSelectRemote={() =>
															setAgentProvider("ds-harness-remote")
														}
														onSelectBridge={() =>
															setAgentProvider("agentero-bridge")
														}
														onSelectAgent={(id) => {
															setAgentProvider("agentero-bridge");
															handleSelectAgent(id);
														}}
														onOpenHistory={() => setHistoryOpen(true)}
													/>
												}
												active={
													layout.agent &&
													agentProvider === "ds-harness-remote" &&
													!pairingRequested &&
													!settingsOpen &&
													!sidebarOpen &&
													!paperImportOpen &&
													!paperInfoOpen
												}
												regionContext={regionContext}
												onRegionContextHandled={handleRegionContext}
											/>
										</div>
										<div
											hidden={agentProvider !== "agentero-bridge"}
											style={{
												display:
													agentProvider !== "agentero-bridge"
														? "none"
														: undefined,
											}}
											className="min-h-0 flex-1"
										>
											<div className="flex h-full min-h-0 flex-col">
												<PaneHeader className="h-12">
													<MobileAgentToolbar
														agents={agents}
														loading={agentsLoading}
														selectedAgentId={selectedAgentId}
														remoteSelected={false}
														onSelectRemote={() => {
															setHistoryOpen(false);
															setAgentProvider("ds-harness-remote");
														}}
														onSelectBridge={() =>
															setAgentProvider("agentero-bridge")
														}
														onSelectAgent={handleSelectAgent}
														onOpenHistory={() => setHistoryOpen(true)}
													/>
												</PaneHeader>
												<div className="min-h-0 flex-1">
													{connected ? (
														<MobileAgentPage
															active={
																layout.agent &&
																agentProvider === "agentero-bridge" &&
																!pairingRequested &&
																!settingsOpen &&
																!sidebarOpen &&
																!paperImportOpen &&
																!paperInfoOpen
															}
															key={selectedAgentId ?? "default"}
															paperPath={selectedPaper?.path ?? undefined}
															selectedAgentId={selectedAgentId}
															sessionId={agentSessionId}
															onSessionId={setAgentSessionId}
															historyOpen={historyOpen}
															onHistoryOpenChange={setHistoryOpen}
														/>
													) : (
														<div className="flex h-full flex-col items-center justify-center gap-4 p-6 text-center">
															<p className="max-w-sm text-muted-foreground text-sm">
																{t("agent.connectionRequired")}
															</p>
															{paired ? (
																<Button
																	variant="outline"
																	className="min-h-12"
																	onClick={() => void resume()}
																>
																	{t("local.retry")}
																</Button>
															) : null}
															{resetAvailable ? (
																<MobileBridgeReset onStatus={setStatus} />
															) : null}
															<Button
																className="min-h-12"
																onClick={() => setPairingRequested(true)}
															>
																{t("connect.title")}
															</Button>
														</div>
													)}
												</div>
											</div>
										</div>
									</div>
								),
							},
						]}
					/>
				</div>
			</main>
			<Dialog
				open={pairingRequested}
				onOpenChange={(open) => {
					setPairingRequested(open);
					if (!open) setPendingOffer(null);
				}}
			>
				<DialogContent
					showCloseButton={false}
					aria-describedby={undefined}
					className="h-dvh w-screen max-w-none rounded-none bg-background p-0 sm:max-w-none"
				>
					<DialogTitle className="sr-only">{t("connect.title")}</DialogTitle>
					<MobilePairing
						status={status}
						pending={pairPending}
						initialOffer={pendingOffer}
						onStatus={setStatus}
						onDone={() => {
							setPendingOffer(null);
							setPairingRequested(false);
						}}
					/>
				</DialogContent>
			</Dialog>
			<MobilePaperImport
				open={paperImportOpen}
				onOpenChange={setPaperImportOpen}
				vaultPath={vaultPath}
				initialText={importReference}
				onImported={refresh}
			/>
			<MobilePaperInfo
				open={paperInfoOpen}
				onOpenChange={setPaperInfoOpen}
				paper={infoPaper}
				vaultPath={vaultPath}
				onUpdated={updatePaperInfo}
			/>
			<MobileTaskIndicator />
			<MobileSettingsPage
				open={settingsOpen}
				section={settingsSection}
				onSectionChange={setSettingsSection}
				onClose={() => setSettingsOpen(false)}
				vaultPath={vaultPath}
				onSynced={rescanAfterSync}
				onReviewSyncConflicts={reviewSyncConflicts}
			/>
			<MobileSidebar
				onSettings={() => {
					setSettingsSection("appearance");
					setSettingsOpen(true);
				}}
				navigation={
					<MobileLibraryNavigation
						active={tab}
						onNavigate={(next) => {
							selectTab(next);
							setSidebarOpen(false);
						}}
						papers={papers}
						selectedPaperId={selectedPaper?.id}
						onSelectPaper={(paper) => {
							selectPaper(paper);
							setSidebarOpen(false);
						}}
						query={libraryQuery}
						onQueryChange={setLibraryQuery}
						selectedTag={selectedTag}
						onTagChange={(tag) => {
							if (!canLeaveReader()) return;
							setSelectedTag(tag);
							closeReader();
							selectTab("library");
							setSidebarOpen(false);
						}}
						loading={papersLoading}
						onImport={() => showImport()}
						onPaperInfo={showPaperInfo}
					/>
				}
				open={sidebarOpen}
				status={status}
				onDisconnect={disconnect}
				onClose={() => setSidebarOpen(false)}
				onStatus={setStatus}
				onPairAnother={() => {
					setAgentProvider("agentero-bridge");
					setPendingOffer(null);
					setPairingRequested(true);
				}}
			/>
			<SyncConflictDialog
				mobile
				pending={paperSync.pendingConflicts}
				open={syncConflictReview.open}
				resolving={syncConflictReview.resolving}
				onResolve={syncConflictReview.apply}
				onDismiss={syncConflictReview.dismiss}
			/>
		</div>
	);
}

function MobileBrand() {
	return <img src={agenteroLogo} alt="Agentero" className="size-8" />;
}
