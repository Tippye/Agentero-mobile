import {
	BookOpen,
	ChevronRight,
	CircleCheck,
	CloudDownload,
	LoaderCircle,
	Pin,
	Trash2,
} from "lucide-react";
import { type TouchEvent, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { MobilePaperSyncState } from "@/components/mobile/hooks/use-mobile-paper-sync";
import { PaperTitle } from "@/components/paper/paper-title";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogTitle,
} from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/core/utils";
import type { PaperMetadata } from "@/lib/paper/types";

/** Width of one revealed row action (delete or pin). */
const ACTION_WIDTH_PX = 72;
/** Delete plus pin; rows without a known sync state only reveal delete. */
const PINNED_ACTION_WIDTH_PX = ACTION_WIDTH_PX * 2;
/** A release past this offset keeps the row open. */
const OPEN_THRESHOLD_PX = 36;
/** Horizontal slop before a touch counts as a row swipe. */
const SWIPE_SLOP_PX = 8;
/** A click right after a committed swipe is the swipe's own click, not a tap. */
const CLICK_SUPPRESS_MS = 400;
/** Keeps vertical scrolling native: the browser owns pan-y, we own the rest. */
const ROW_TOUCH_ACTION = { touchAction: "pan-y" } as const;

type RowSwipe = {
	key: string;
	offset: number;
	dragging: boolean;
};

type SwipeGesture = {
	key: string;
	startX: number;
	startY: number;
	startOffset: number;
	offset: number;
	startTime: number;
	lastX: number;
	decided: boolean;
	otherOpen: boolean;
};

function rowKeyOf(paper: PaperMetadata) {
	return paper.path ?? paper.id;
}

export function MobileLibraryPage({
	papers,
	loading,
	selected,
	onSelect,
	query,
	syncStates = {},
	onPinChange,
	syncProgress,
	onDeletePaper,
}: {
	papers: PaperMetadata[];
	loading: boolean;
	selected: PaperMetadata | null;
	onSelect: (paper: PaperMetadata) => void;
	query: string;
	syncStates?: Record<string, MobilePaperSyncState>;
	onPinChange?: (paper: PaperMetadata, pinned: boolean) => void;
	syncProgress?: string | null;
	onDeletePaper?: (paper: PaperMetadata) => Promise<void>;
}) {
	const { t } = useTranslation(["mobile", "common"]);
	const [openKey, setOpenKey] = useState<string | null>(null);
	const [drag, setDrag] = useState<RowSwipe | null>(null);
	const [pendingDelete, setPendingDelete] = useState<PaperMetadata | null>(
		null,
	);
	const [busy, setBusy] = useState(false);
	const gesture = useRef<SwipeGesture | null>(null);
	const suppressClick = useRef<{ key: string; until: number } | null>(null);
	const busyRef = useRef(false);
	const filtered = useMemo(() => {
		const normalized = query.trim().toLowerCase();
		if (!normalized) return papers;
		return papers.filter((paper) =>
			`${paper.title} ${paper.authors.join(" ")}`
				.toLowerCase()
				.includes(normalized),
		);
	}, [papers, query]);

	const hasPinAction = (paper: PaperMetadata) =>
		Boolean(onPinChange && syncStates[paper.id]);

	const actionWidthFor = (rowKey: string) => {
		const paper = filtered.find((candidate) => rowKeyOf(candidate) === rowKey);
		return paper && hasPinAction(paper)
			? PINNED_ACTION_WIDTH_PX
			: ACTION_WIDTH_PX;
	};

	const shouldSuppressClick = (rowKey: string) => {
		const suppressed = suppressClick.current;
		return (
			suppressed !== null &&
			suppressed.key === rowKey &&
			Date.now() < suppressed.until
		);
	};

	const handleTouchStart = (rowKey: string, event: TouchEvent<HTMLElement>) => {
		if (!onDeletePaper && !onPinChange) return;
		const touch = event.touches[0];
		if (!touch) return;
		// A fresh touch is a deliberate tap, so it ends the post-swipe click guard.
		suppressClick.current = null;
		const startOffset = openKey === rowKey ? actionWidthFor(rowKey) : 0;
		gesture.current = {
			key: rowKey,
			startX: touch.clientX,
			startY: touch.clientY,
			startOffset,
			offset: startOffset,
			startTime: Date.now(),
			lastX: touch.clientX,
			decided: false,
			otherOpen: openKey !== null && openKey !== rowKey,
		};
	};

	const handleTouchMove = (rowKey: string, event: TouchEvent<HTMLElement>) => {
		const current = gesture.current;
		if (!current || current.key !== rowKey) return;
		const touch = event.touches[0];
		if (!touch) return;
		const dx = touch.clientX - current.startX;
		const dy = touch.clientY - current.startY;
		if (!current.decided) {
			if (
				Math.abs(dx) <= SWIPE_SLOP_PX ||
				Math.abs(dx) <= Math.abs(dy) * 1.25
			) {
				return;
			}
			current.decided = true;
			// Starting a swipe on another row closes the previously open one.
			if (current.otherOpen) {
				setOpenKey(null);
				current.startOffset = 0;
				current.offset = 0;
			}
		}
		const offset = Math.min(
			actionWidthFor(rowKey),
			Math.max(0, current.startOffset - dx),
		);
		current.offset = offset;
		current.lastX = touch.clientX;
		setDrag({ key: rowKey, offset, dragging: true });
	};

	const handleTouchEnd = (rowKey: string, event: TouchEvent<HTMLElement>) => {
		const current = gesture.current;
		if (!current || current.key !== rowKey) return;
		gesture.current = null;
		if (!current.decided) {
			setDrag(null);
			return;
		}
		const touch = event.changedTouches[0] ?? event.touches[0];
		const endX = touch ? touch.clientX : current.lastX;
		const elapsed = Math.max(1, Date.now() - current.startTime);
		const velocity = (endX - current.startX) / elapsed;
		const shouldOpen =
			current.offset > OPEN_THRESHOLD_PX ||
			(current.offset > 12 && elapsed >= 16 && velocity < -0.5);
		suppressClick.current = {
			key: rowKey,
			until: Date.now() + CLICK_SUPPRESS_MS,
		};
		setDrag(null);
		setOpenKey(shouldOpen ? rowKey : null);
	};

	const handleTouchCancel = (rowKey: string) => {
		if (gesture.current?.key !== rowKey) return;
		gesture.current = null;
		setDrag(null);
	};

	const handleSelect = (paper: PaperMetadata, rowKey: string) => {
		if (shouldSuppressClick(rowKey)) return;
		if (openKey === rowKey) {
			setOpenKey(null);
			return;
		}
		setOpenKey(null);
		onSelect(paper);
	};

	const handlePin = (paper: PaperMetadata, pinned: boolean) => {
		onPinChange?.(paper, pinned);
	};

	const handleDialogOpenChange = (open: boolean) => {
		if (!open && !busyRef.current) setPendingDelete(null);
	};

	const confirmDelete = async () => {
		const paper = pendingDelete;
		if (!paper || !onDeletePaper || busyRef.current) return;
		busyRef.current = true;
		setBusy(true);
		try {
			await onDeletePaper(paper);
			setPendingDelete(null);
			const rowKey = rowKeyOf(paper);
			setOpenKey((current) => (current === rowKey ? null : current));
		} catch {
			// The owner reports the failure; keep the dialog and row retryable.
		} finally {
			busyRef.current = false;
			setBusy(false);
		}
	};

	return (
		<section className="flex h-full min-h-0 select-none flex-col">
			{syncProgress ? (
				<p
					role="status"
					aria-live="polite"
					className="border-b px-4 py-2 text-muted-foreground text-sm"
				>
					{syncProgress}
				</p>
			) : null}
			<div className="agentero-scroll flex-1">
				{loading && papers.length === 0 ? (
					<ul className="divide-y" aria-busy="true">
						<li className="sr-only" role="status">
							{t("library.loading")}
						</li>
						{["one", "two", "three", "four", "five", "six"].map((key) => (
							<li
								key={key}
								aria-hidden="true"
								className="flex items-center gap-3 px-4 py-4.5 md:px-6"
							>
								<Skeleton className="size-11 shrink-0 rounded-lg" />
								<span className="min-w-0 flex-1 space-y-2">
									<Skeleton className="h-4 w-4/5" />
									<Skeleton className="h-3 w-1/2" />
								</span>
							</li>
						))}
					</ul>
				) : (
					<>
						<ul className="content-arrive divide-y">
							{filtered.map((paper) => {
								const rowKey = rowKeyOf(paper);
								const isOpen = openKey === rowKey;
								const activeDrag = drag?.key === rowKey ? drag : null;
								const hasPin = hasPinAction(paper);
								const actionWidth = hasPin
									? PINNED_ACTION_WIDTH_PX
									: ACTION_WIDTH_PX;
								const offset = activeDrag
									? activeDrag.offset
									: isOpen
										? actionWidth
										: 0;
								const dragging = activeDrag?.dragging === true;
								return (
									<li key={rowKey} className="relative overflow-hidden">
										<div
											className="relative overflow-hidden"
											style={ROW_TOUCH_ACTION}
											data-swipe-container={rowKey}
											onTouchStart={(event) => handleTouchStart(rowKey, event)}
											onTouchMove={(event) => handleTouchMove(rowKey, event)}
											onTouchEnd={(event) => handleTouchEnd(rowKey, event)}
											onTouchCancel={() => handleTouchCancel(rowKey)}
										>
											{onDeletePaper || hasPin ? (
												<div
													className="absolute inset-y-0 right-0 flex"
													style={{ width: actionWidth }}
												>
													{hasPin ? (
														<Button
															type="button"
															variant={
																syncStates[paper.id].pinned
																	? "secondary"
																	: "outline"
															}
															className="h-full min-h-11 w-[72px] touch-manipulation rounded-none"
															tabIndex={isOpen ? 0 : -1}
															aria-pressed={syncStates[paper.id].pinned}
															aria-label={t(
																syncStates[paper.id].pinned
																	? "sync.unpin"
																	: "sync.pin",
																{ title: paper.title },
															)}
															title={t(
																syncStates[paper.id].pinned
																	? "sync.unpin"
																	: "sync.pin",
																{ title: paper.title },
															)}
															disabled={syncStates[paper.id].busy}
															onClick={() =>
																handlePin(paper, !syncStates[paper.id].pinned)
															}
														>
															{syncStates[paper.id].busy ? (
																<LoaderCircle className="size-5 animate-spin" />
															) : (
																<Pin
																	className={cn(
																		"size-5",
																		syncStates[paper.id].pinned &&
																			"fill-current text-primary",
																	)}
																/>
															)}
														</Button>
													) : null}
													{onDeletePaper ? (
														<Button
															type="button"
															variant="destructive"
															className="h-full min-h-11 w-[72px] touch-manipulation rounded-none"
															tabIndex={isOpen ? 0 : -1}
															aria-label={t("library.deletePaper", {
																title: paper.title,
															})}
															disabled={busy}
															onClick={() => setPendingDelete(paper)}
														>
															<Trash2 className="size-5" />
														</Button>
													) : null}
												</div>
											) : null}
											<div
												className="relative flex items-center bg-background"
												data-swipe-content={rowKey}
												style={{
													transform:
														offset === 0
															? "translateX(0px)"
															: `translateX(-${offset}px)`,
													transition: dragging
														? "none"
														: "transform 150ms ease-out",
												}}
											>
												<button
													type="button"
													onClick={() => handleSelect(paper, rowKey)}
													className={cn(
														"flex min-w-0 flex-1 items-center gap-3 px-4 py-4.5 text-left md:px-6",
														selected?.id === paper.id && "bg-muted/60",
													)}
												>
													<div className="grid size-11 shrink-0 place-items-center rounded-lg border bg-muted text-muted-foreground">
														{syncStates[paper.id] ? (
															syncStates[paper.id].offline ? (
																<CircleCheck
																	className="size-5"
																	aria-label={t("sync.offline")}
																/>
															) : (
																<CloudDownload
																	className="size-5"
																	aria-label={t("sync.deferred")}
																/>
															)
														) : (
															<BookOpen className="size-5" />
														)}
													</div>
													<span className="min-w-0 flex-1">
														<span className="line-clamp-2 block font-medium text-base leading-6">
															<PaperTitle text={paper.title} />
														</span>
														<span className="mt-1 block truncate text-muted-foreground text-sm">
															{paper.authors.join(", ")}
															{paper.year ? ` · ${paper.year}` : ""}
														</span>
													</span>
													<ChevronRight className="size-5 shrink-0 text-muted-foreground" />
												</button>
											</div>
										</div>
									</li>
								);
							})}
						</ul>
						{filtered.length === 0 ? (
							<div className="grid h-full place-items-center text-muted-foreground text-sm">
								{t("library.empty")}
							</div>
						) : null}
					</>
				)}
			</div>
			<Dialog
				open={pendingDelete !== null}
				onOpenChange={handleDialogOpenChange}
			>
				<DialogContent showCloseButton={false}>
					<DialogTitle>{t("library.deletePaperTitle")}</DialogTitle>
					<DialogDescription>
						{t("library.deletePaperDescription", {
							title: pendingDelete?.title ?? "",
						})}
					</DialogDescription>
					<div className="flex justify-end gap-2">
						<Button
							type="button"
							variant="outline"
							className="min-h-11"
							disabled={busy}
							onClick={() => setPendingDelete(null)}
						>
							{t("common:cancel")}
						</Button>
						<Button
							type="button"
							variant="destructive"
							className="min-h-11"
							disabled={busy}
							onClick={() => void confirmDelete()}
						>
							{t("library.deletePaperConfirm")}
						</Button>
					</div>
				</DialogContent>
			</Dialog>
		</section>
	);
}
